//! Frame plans for the GPU compositor.
//!
//! [`plan_frame`] resolves what is visible at a time into a list of layers the GPU can draw
//! directly: a decoded source frame (YUV planes or RGBA), the matrix from source pixels to output
//! pixels, an opacity and a blend mode (all 27 of [`Blend`] are composited by the GPU). A media
//! clip whose enabled standard effects all have a GPU implementation ([`crate::gpufx`]) carries
//! them as a [`LayerFx`]: their parameters evaluated at the frame's time, run by the GPU on the
//! clip's working image before Motion places it, in the CPU's order (effects → Motion → Opacity /
//! blend). Lumetri runs as a baked LUT ([`crate::gpufx::gpu_ops`]). An adjustment layer of the
//! top sequence whose effects the GPU covers (unmasked, covering the whole frame, not inside a
//! transition) is an [`adjust`](PlanLayer::adjust) layer: its effects run on everything composited
//! below it and the result is drawn over it with the layer's opacity and blend mode, as the CPU
//! does. Anything the shaders don't cover yet — other standard effects, masks, other adjustment
//! layers, nested sequences, non-dissolve transitions — is rendered on the CPU for that layer (or
//! the whole frame) and handed over as a pre-composited image, so the GPU path matches the CPU
//! reference (exactly, or within the LUT's interpolation for Lumetri).

use std::sync::Arc;

use filmcraft_frame::VideoFrame;
use filmcraft_geom::Affine;
use filmcraft_media::FrameRequest;
use filmcraft_project::{ItemId, ItemKind, Project, Sequence, TrackItem};
use filmcraft_time::Tick;

use crate::gpufx::FxOp;
use crate::{Blend, RenderOptions, SourceProvider, motion_matrix, output_size};

/// One layer for the GPU, bottom to top.
#[derive(Clone)]
pub struct PlanLayer {
    pub frame: Arc<VideoFrame>,
    /// Maps frame pixels (0..w, 0..h) to output pixels — or, with [`fx`](Self::fx), working-image
    /// pixels (0..fx.size).
    pub matrix: Affine,
    pub opacity: f32,
    /// How the layer combines with what is under it ([`crate::blend::composite`]).
    pub blend: Blend,
    /// Standard effects to run on the frame before it is placed (None: draw the frame directly).
    pub fx: Option<Arc<LayerFx>>,
    /// An adjustment layer: `fx` runs on the picture composited so far (sized `fx.size`, the
    /// output) instead of on `frame`, and the result is drawn over it at the identity matrix.
    pub adjust: bool,
}

/// The GPU effect stage of a layer: the frame is decoded into a working image of `size` (the
/// frame box-decimated by `decimation`, as the CPU decodes it), `ops` run on it in order, and the
/// result is placed with the layer's matrix, opacity and blend mode.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerFx {
    pub size: (u32, u32),
    pub decimation: u32,
    pub ops: Vec<FxOp>,
}

impl PlanLayer {
    /// A layer drawing `frame` directly.
    pub fn new(frame: Arc<VideoFrame>, matrix: Affine, opacity: f32, blend: Blend) -> Self {
        Self { frame, matrix, opacity, blend, fx: None, adjust: false }
    }

    /// Size of the picture the matrix places: the working image with effects, else the frame.
    pub fn size(&self) -> (u32, u32) {
        match &self.fx {
            Some(fx) => fx.size,
            None => (self.frame.width, self.frame.height),
        }
    }
}

#[derive(Clone)]
pub enum FramePlan {
    /// Draw these layers over black.
    Layers { width: usize, height: usize, layers: Vec<PlanLayer> },
    /// The CPU produced the final image (fallback).
    Image(crate::Image),
}

fn cpu_frame(img: crate::Image) -> Arc<VideoFrame> {
    Arc::new(VideoFrame::rgba_f32(img.w as u32, img.h as u32, img.px))
}

fn simple_transition(id: &str) -> bool {
    matches!(id, "cross_dissolve" | "dip_to_black" | "dip_to_white" | "morph_cut")
}

/// The standard effects of a media clip if the GPU can draw it (any blend mode, no opacity masks,
/// every enabled standard effect with a GPU implementation, masked or not; empty without effects
/// or with effects off). None: the clip is rendered on the CPU.
fn gpu_chain<'a>(project: &Project, item: &'a TrackItem, opts: RenderOptions) -> Option<Vec<&'a filmcraft_project::EffectInstance>> {
    let is_media = project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Media(_) | ItemKind::Subclip { .. }));
    if !is_media || item.has_opacity_masks() {
        return None;
    }
    if !opts.effects {
        return Some(Vec::new());
    }
    let mut chain = Vec::new();
    for e in item.effects.iter().filter(|e| e.def().is_some_and(|d| !d.intrinsic) && !filmcraft_project::graphic::is_layer(e)) {
        if !e.enabled {
            continue;
        }
        // (masked effects run on the GPU too: `gpu_ops` wraps them in `FxOp::Masked`)
        if !(crate::gpufx::GPU_EFFECTS.contains(&e.effect.as_str()) || e.effect == "lumetri") {
            return None;
        }
        chain.push(e);
    }
    Some(chain)
}

/// Plan the frame at timeline `t`.
pub fn plan_frame(project: &Project, seq_id: ItemId, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider) -> FramePlan {
    let Some(seq) = project.sequence(seq_id) else { return FramePlan::Image(crate::Image::new(1, 1)) };
    let (w, h) = output_size(seq, opts.scale);
    // HDR / wide-gamut sequences composite and convert on the CPU, and so do sequences that mix
    // display-encoded values (Composite in Linear Color off).
    if !seq.settings.color.is_plain() || !seq.settings.composite_linear {
        return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
    }
    // Whole-frame fallback: adjustment layers the GPU can't run, or complex transitions, at t.
    if !layered_at(project, seq, t, Some(opts)) {
        return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
    }
    let mut layers = Vec::new();
    push_tracks(project, seq, t, opts, sources, 0, &mut layers);
    if opts.captions {
        for o in crate::caption_overlays(seq, t, w, h) {
            layers.push(PlanLayer::new(
                Arc::new(VideoFrame::rgba_f32(o.w as u32, o.h as u32, o.px)),
                Affine::translate(o.x as f64, o.y as f64),
                1.0,
                Blend::Normal,
            ));
        }
    }
    FramePlan::Layers { width: w, height: h, layers }
}

/// Whether the frame of `seq` at `t` can be planned as layers: no transition the compositor
/// cannot mix itself, and no adjustment layer unless `adjust` (the top sequence's options) is
/// given and the GPU can run it ([`adjustment_fx`]).
fn layered_at(project: &Project, seq: &Sequence, t: Tick, adjust: Option<RenderOptions>) -> bool {
    let is_adjustment = |it: &TrackItem| project.item(it.item).is_some_and(|p| matches!(p.kind, ItemKind::AdjustmentLayer { .. }));
    for tr in &seq.video_tracks {
        if !tr.enabled {
            continue;
        }
        if let Some(trn) = tr.transitions.iter().find(|x| x.range().contains(t)) {
            let ends = [trn.from, trn.to].into_iter().flatten().filter_map(|id| tr.item(id));
            if !simple_transition(&trn.effect.effect) || ends.into_iter().any(is_adjustment) {
                return false;
            }
        }
        if let Some(it) = tr.item_at(t)
            && it.enabled
            && is_adjustment(it)
            && adjust.is_none_or(|opts| opts.effects && adjustment_fx(project, seq, it, t, opts).is_none())
        {
            return false;
        }
    }
    true
}

/// The GPU effect stage of adjustment layer `item` at `t`: its standard effects as ops over the
/// whole output. None when the CPU must render it (masked effects or opacity masks, a Motion that
/// moves its frame off the full picture, or an effect without a GPU implementation).
fn adjustment_fx(project: &Project, seq: &Sequence, item: &TrackItem, t: Tick, opts: RenderOptions) -> Option<LayerFx> {
    if item.has_opacity_masks() {
        return None;
    }
    let mt = item.effect_time_at(t);
    let (w, h) = output_size(seq, opts.scale);
    if crate::adjustment_region(seq, item, project, mt, opts.scale, w, h).is_some() {
        return None;
    }
    let cx = crate::effects::FxCtx {
        t: mt,
        px_scale: opts.scale,
        seconds: (t - item.start).seconds(),
        timecode: "",
        clip_name: &item.name,
        project: Some(project),
        env: None,
        working: seq.settings.color.working,
    };
    let mut ops = Vec::new();
    for e in item.effects.iter().filter(|e| e.def().is_some_and(|d| !d.intrinsic) && !filmcraft_project::graphic::is_layer(e)) {
        if !e.enabled {
            continue;
        }
        if e.masks.iter().any(|m| m.mode != filmcraft_project::MaskMode::None) {
            return None;
        }
        ops.extend(crate::gpufx::gpu_ops(e, &cx, w, h)?);
    }
    Some(LayerFx { size: (w as u32, h as u32), decimation: 1, ops })
}

/// Push the layers of every video track of `seq` at `t`, bottom track first (`seq` must be
/// [`layered_at`] `t`). `nest` counts the nested sequences already followed to reach `seq`.
fn push_tracks(project: &Project, seq: &Sequence, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider, nest: u32, layers: &mut Vec<PlanLayer>) {
    let (w, h) = output_size(seq, opts.scale);
    for tr in &seq.video_tracks {
        if !tr.enabled {
            continue;
        }
        if let Some(trn) = tr.transitions.iter().find(|x| x.range().contains(t)) {
            // These dissolves are symmetric: Reverse (play B→A backwards) renders the same frames.
            let p = trn.progress(t) as f32;
            let a = trn.from.and_then(|id| tr.item(id)).filter(|i| i.enabled);
            let b = trn.to.and_then(|id| tr.item(id)).filter(|i| i.enabled);
            match trn.effect.effect.as_str() {
                "dip_to_black" | "dip_to_white" => {
                    let col = if trn.effect.effect == "dip_to_black" { [0.0, 0.0, 0.0, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
                    layers.push(PlanLayer::new(Arc::new(VideoFrame::rgba_f32(1, 1, col.to_vec())), Affine::scale(w as f64, h as f64), 1.0, Blend::Normal));
                    let (it, k) = if p < 0.5 { (a, 1.0 - p * 2.0) } else { (b, (p - 0.5) * 2.0) };
                    if let Some(it) = it {
                        push_item(project, seq, it, t, opts, sources, k, Some(Blend::Normal), nest, layers);
                    }
                }
                _ => {
                    // cross dissolve: A at full, B over it at p (premultiplied over == linear mix when A is opaque)
                    if let Some(it) = a {
                        push_item(project, seq, it, t, opts, sources, 1.0 - if b.is_none() { p } else { 0.0 }, Some(Blend::Normal), nest, layers);
                    }
                    if let Some(it) = b {
                        push_item(project, seq, it, t, opts, sources, p, Some(Blend::Normal), nest, layers);
                    }
                }
            }
            continue;
        }
        let Some(item) = tr.item_at(t) else { continue };
        if !item.enabled {
            continue;
        }
        if project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::AdjustmentLayer { .. })) {
            // (only the top sequence plans adjustment layers; see `layered_at`)
            if nest == 0
                && opts.effects
                && let Some(fx) = adjustment_fx(project, seq, item, t, opts)
            {
                let (op, bl) = crate::opacity_blend(item, item.effect_time_at(t));
                let blank = Arc::new(VideoFrame::rgba_f32(1, 1, vec![0.0; 4]));
                layers.push(PlanLayer { frame: blank, matrix: Affine::IDENTITY, opacity: op, blend: bl, fx: Some(Arc::new(fx)), adjust: true });
            }
            continue;
        }
        push_item(project, seq, item, t, opts, sources, 1.0, None, nest, layers);
    }
}

/// Whether a nested sequence can be drawn by pushing its own layers into the plan of the sequence
/// it is in, instead of being rendered to an image on the CPU first. Compositing its layers one
/// by one over what is below gives the same picture as compositing them together first only when
/// they all blend Normal, and the frame must be one that plans as layers in the same colour
/// pipeline, mixing in linear light (a nest that mixes display values is rendered on the CPU).
fn nest_is_plain(project: &Project, seq: &Sequence, nested: &Sequence, ft: Tick) -> bool {
    nested.settings.color == seq.settings.color
        && nested.settings.composite_linear
        && (nested.settings.width, nested.settings.height) == (seq.settings.width, seq.settings.height)
        && layered_at(project, nested, ft, None)
        && nested.video_tracks.iter().filter(|tr| tr.enabled).all(|tr| {
            tr.transitions.iter().any(|x| x.range().contains(ft))
                || tr.item_at(ft).filter(|i| i.enabled).is_none_or(|i| crate::opacity_blend(i, i.effect_time_at(ft)).1 == Blend::Normal)
        })
}

/// Push the layer(s) of `item`. `blend` overrides the item's own blend mode: inside a transition
/// the CPU reference mixes the clips and composites the result Normal, ignoring their modes.
/// `nest` counts the multi-camera clips already followed to reach `item` (see
/// [`crate::MAX_NEST_DEPTH`]).
#[allow(clippy::too_many_arguments)]
fn push_item(
    project: &Project,
    seq: &Sequence,
    item: &TrackItem,
    t: Tick,
    opts: RenderOptions,
    sources: &dyn SourceProvider,
    extra_opacity: f32,
    blend: Option<Blend>,
    nest: u32,
    out: &mut Vec<PlanLayer>,
) {
    // frame time (`ft`) vs. effect time (`mt`): they differ inside a frame hold without Hold Filters
    let ft = item.source_time_at(t);
    let mt = item.effect_time_at(t);
    let (op, own) = crate::opacity_blend(item, mt);
    let bl = blend.unwrap_or(own);
    // A multi-camera clip showing an angle with standard effects and / or a Motion (a grade, a
    // blur, a push-in): the angle's clip is drawn on the GPU through the multi-camera clip's
    // effects and Motion, with no CPU pass over the nested sequence. The nested frame size must be
    // this sequence's, so the nested canvas is the clip's own picture.
    if let Some(ItemKind::Sequence(nested)) = project.item(item.item).map(|p| &p.kind)
        && nest < crate::MAX_NEST_DEPTH
        && let Some(angle) = item.multicam_angle(nested)
        && (nested.settings.width, nested.settings.height) == (seq.settings.width, seq.settings.height)
        && let outer_motion = motion_matrix(seq, item, (nested.settings.width, nested.settings.height), mt)
        && ((opts.effects && item.has_standard_effects()) || !near_identity(&outer_motion))
        && !item.has_opacity_masks()
        && let Some(effects) = gpu_effects(item, opts)
        && let Some(tr) = nested.angle_video_track_index(angle).and_then(|i| nested.video_tracks.get(i))
        && !tr.transitions.iter().any(|x| x.range().contains(ft))
        && let Some(inner) = tr.item_at(ft).filter(|i| i.enabled)
        && crate::opacity_blend(inner, inner.effect_time_at(ft)).1 != Blend::Dissolve
    {
        let outer = Outer { item, motion: outer_motion, effects, t, mt };
        let blend_inner = bl;
        match push_media_gpu(project, nested, inner, ft, opts, sources, extra_opacity * op, blend_inner, Some(&outer), out) {
            Gpu::Drawn | Gpu::Nothing => return,
            Gpu::Fallback => {}
        }
    }
    // A multi-camera clip that only shows its angle (no effects, untransformed, same frame size)
    // draws the angle's clip directly: no CPU pass over the nested sequence.
    if let Some(ItemKind::Sequence(nested)) = project.item(item.item).map(|p| &p.kind)
        && nest < crate::MAX_NEST_DEPTH
        && let Some(angle) = item.multicam_angle(nested)
        && !(opts.effects && item.has_standard_effects())
        && (nested.settings.width, nested.settings.height) == (seq.settings.width, seq.settings.height)
        && near_identity(&motion_matrix(seq, item, (nested.settings.width, nested.settings.height), mt))
        && let Some(tr) = nested.angle_video_track_index(angle).and_then(|i| nested.video_tracks.get(i))
        && !tr.transitions.iter().any(|x| x.range().contains(ft))
        && tr.item_at(ft).is_none_or(|i| crate::opacity_blend(i, i.effect_time_at(ft)).1 != Blend::Dissolve)
    {
        // Inside the nested sequence the angle's clip is composited onto an empty canvas, where
        // every mode but Dissolve is Normal; the multicam clip's own mode then applies to it.
        if let Some(inner) = tr.item_at(ft).filter(|i| i.enabled) {
            push_item(project, nested, inner, ft, opts, sources, extra_opacity * op, Some(bl), nest + 1, out);
        }
        return;
    }
    // A nested sequence that is only shown (no effects, untransformed, fully opaque, blending
    // Normal) contributes its own layers: the compositor draws them like the clips of this
    // sequence, with no CPU pass over the nested sequence. Its captions are part of its picture.
    if let Some(ItemKind::Sequence(nested)) = project.item(item.item).map(|p| &p.kind)
        && nest < crate::MAX_NEST_DEPTH
        && item.multicam_angle(nested).is_none()
        && !(opts.effects && item.has_standard_effects())
        && !item.has_opacity_masks()
        && bl == Blend::Normal
        && extra_opacity * op >= 1.0 - 1e-6
        && near_identity(&motion_matrix(seq, item, (nested.settings.width, nested.settings.height), mt))
        && nest_is_plain(project, seq, nested, ft)
    {
        push_tracks(project, nested, ft, opts, sources, nest + 1, out);
        let (w, h) = output_size(nested, opts.scale);
        for o in crate::caption_overlays(nested, ft, w, h) {
            out.push(PlanLayer::new(
                Arc::new(VideoFrame::rgba_f32(o.w as u32, o.h as u32, o.px)),
                Affine::translate(o.x as f64, o.y as f64),
                1.0,
                Blend::Normal,
            ));
        }
        return;
    }
    // Graphic clips without standard effects: the layers are rasterised (cached) into one tight
    // image the GPU places as a layer.
    if !(opts.effects && item.has_standard_effects())
        && !item.has_opacity_masks()
        && project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Graphic { .. }))
    {
        let Some(size) = crate::source_size(project, item.item) else { return };
        let (w, h) = output_size(seq, opts.scale);
        let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&motion_matrix(seq, item, size, mt));
        if let Some((img, x, y)) = crate::graphic_clip::render_graphic_tight(item, mt, size, &m, w, h) {
            out.push(PlanLayer::new(cpu_frame(img), Affine::translate(x as f64, y as f64), op * extra_opacity, bl));
        }
        return;
    }
    match push_media_gpu(project, seq, item, t, opts, sources, extra_opacity, bl, None, out) {
        Gpu::Drawn | Gpu::Nothing => return,
        Gpu::Fallback => {}
    }
    // CPU-rendered layer (standard effects): drawn by the GPU as a pre-rendered canvas image.
    let tc = filmcraft_time::format_time(t, seq.settings.frame_rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48_000);
    if let Some((img, op2, _)) = crate::item_layer(project, seq, item, t, opts, sources, &tc) {
        out.push(PlanLayer::new(cpu_frame(img), Affine::IDENTITY, op2 * extra_opacity, bl));
    }
}

/// What [`push_media_gpu`] did.
enum Gpu {
    /// Its layer was pushed.
    Drawn,
    /// There is nothing to draw (the source or its frame is not there).
    Nothing,
    /// The GPU cannot draw it: render it on the CPU.
    Fallback,
}

/// The multi-camera clip around a media clip that [`push_media_gpu`] draws for it: the clip's
/// Motion (nested picture → this sequence), and its standard effects with the time they are
/// evaluated at.
struct Outer<'a> {
    item: &'a TrackItem,
    motion: Affine,
    effects: Vec<&'a filmcraft_project::EffectInstance>,
    /// Timeline time of the multi-camera clip (its effects and name context).
    t: Tick,
    /// Effect time of the multi-camera clip.
    mt: Tick,
}

/// The standard effects of `item` (any clip) when each has a GPU implementation (masked or not).
fn gpu_effects(item: &TrackItem, opts: RenderOptions) -> Option<Vec<&filmcraft_project::EffectInstance>> {
    if item.has_opacity_masks() {
        return None;
    }
    let mut chain = Vec::new();
    if !opts.effects {
        return Some(chain);
    }
    for e in item.effects.iter().filter(|e| e.def().is_some_and(|d| !d.intrinsic) && !filmcraft_project::graphic::is_layer(e)) {
        if !e.enabled {
            continue;
        }
        if !(crate::gpufx::GPU_EFFECTS.contains(&e.effect.as_str()) || e.effect == "lumetri") {
            return None;
        }
        chain.push(e);
    }
    Some(chain)
}

/// Draw a media clip on the GPU: its decoded frame placed by its Motion, through its standard
/// effects evaluated for the working image (the GPU effect stage), like the CPU's `item_layer`.
/// `outer` is the multi-camera clip showing it as an angle: its effects follow the clip's own and
/// its Motion places the picture.
fn push_media_gpu(
    project: &Project,
    seq: &Sequence,
    item: &TrackItem,
    t: Tick,
    opts: RenderOptions,
    sources: &dyn SourceProvider,
    extra_opacity: f32,
    bl: Blend,
    outer: Option<&Outer>,
    out: &mut Vec<PlanLayer>,
) -> Gpu {
    let Some(own) = gpu_chain(project, item, opts) else { return Gpu::Fallback };
    let mt = item.effect_time_at(t);
    let ft = item.source_time_at(t);
    let (op, _) = crate::opacity_blend(item, mt);
    let Some(src) = sources.source(item.item) else { return Gpu::Nothing };
    let Some(size) = crate::source_size(project, item.item) else { return Gpu::Nothing };
    let inner_motion = motion_matrix(seq, item, size, mt);
    // through an outer multi-camera clip: its effects work on the clip's picture, which is the
    // canvas only while the clip fills it
    if outer.is_some_and(|o| !o.effects.is_empty()) && !near_identity(&inner_motion) {
        return Gpu::Fallback;
    }
    let full = match outer {
        Some(o) => o.motion.then_apply(&inner_motion),
        None => inner_motion,
    };
    let lin = ((full.a * full.a + full.b * full.b).sqrt()).max((full.c * full.c + full.d * full.d).sqrt());
    let want = (lin * opts.scale as f64).clamp(1.0 / 64.0, 1.0) as f32;
    let Ok(frame) = src.video_frame(FrameRequest { time: ft, scale: want }) else { return Gpu::Nothing };
    let cs = crate::colorman::source_space(project, item.item, &frame);
    // log / HDR / wide-gamut media is converted on the CPU
    if crate::colorman::needs_management(&seq.settings.color, cs, &frame) {
        return Gpu::Fallback;
    }
    // Frame Blending / Optical Flow (renders as blending) of a speed-changed clip: the frame and
    // the next one, drawn one over the other at the blend weight — in linear light, over an
    // opaque picture exactly the CPU's `lerp` of the two
    let second = crate::interpolation_blend(item, t, src.info().frame_rate()).and_then(|(next, w)| {
        let f2 = src.video_frame(FrameRequest { time: next, scale: want }).ok()?;
        (f2.width == frame.width && f2.height == frame.height && w > 1e-4).then_some((f2, w.clamp(0.0, 1.0)))
    });
    let opacity = op * extra_opacity;
    let chain_len = own.len() + outer.map_or(0, |o| o.effects.len());
    if chain_len > 0 {
        // GPU effect stage: the working image the CPU would decode (`base_layer`), the effects
        // evaluated for it, placed as `item_layer` places it
        let n = crate::decimation(frame.width as f32, size.0 as f32 * want);
        let (lw, lh) = ((frame.width as usize / n).max(1), (frame.height as usize / n).max(1));
        let px_scale = lw as f32 / size.0.max(1) as f32;
        let mut ops = Vec::new();
        let own_cx = crate::effects::FxCtx {
            t: mt,
            px_scale,
            seconds: (t - item.start).seconds(),
            timecode: "",
            clip_name: &item.name,
            project: Some(project),
            env: None,
            working: seq.settings.color.working,
        };
        for e in &own {
            let Some(v) = crate::gpufx::gpu_ops(e, &own_cx, lw, lh) else { return Gpu::Fallback };
            ops.extend(v);
        }
        if let Some(o) = outer {
            let ocx = crate::effects::FxCtx {
                t: o.mt,
                px_scale,
                seconds: (o.t - o.item.start).seconds(),
                timecode: "",
                clip_name: &o.item.name,
                project: Some(project),
                env: None,
                working: seq.settings.color.working,
            };
            for e in &o.effects {
                let Some(v) = crate::gpufx::gpu_ops(e, &ocx, lw, lh) else { return Gpu::Fallback };
                ops.extend(v);
            }
        }
        let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&full).then_apply(&Affine::scale(1.0 / px_scale as f64, 1.0 / px_scale as f64));
        let fx = Arc::new(LayerFx { size: (lw as u32, lh as u32), decimation: n as u32, ops });
        out.push(PlanLayer { frame, matrix: m, opacity, blend: bl, fx: Some(fx.clone()), adjust: false });
        if let Some((f2, w)) = second {
            out.push(PlanLayer { frame: f2, matrix: m, opacity: opacity * w, blend: bl, fx: Some(fx), adjust: false });
        }
        return Gpu::Drawn;
    }
    // Draft playback at reduced resolution: hand over planes box-filtered to the size drawn
    // instead of the full picture (a quarter at 1/2, a sixteenth at 1/4 of the upload and
    // sampling). The mean is taken over Y'CbCr codes, not linear light as the shader's
    // supersampling does, so this stays limited to the opt-in draft mode.
    let n = if filmcraft_media::cancel::draft() { crate::decimation(frame.width as f32, size.0 as f32 * want) } else { 1 };
    let shrink = |f: Arc<VideoFrame>| match f.box_decimated(n) {
        Some(small) => Arc::new(small),
        None => f,
    };
    let frame = shrink(frame);
    let px_scale = frame.width as f64 / size.0.max(1) as f64;
    let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&full).then_apply(&Affine::scale(1.0 / px_scale, 1.0 / px_scale));
    out.push(PlanLayer::new(frame, m, opacity, bl));
    if let Some((f2, w)) = second {
        out.push(PlanLayer::new(shrink(f2), m, opacity * w, bl));
    }
    Gpu::Drawn
}

fn near_identity(m: &Affine) -> bool {
    (m.a - 1.0).abs() < 1e-9 && (m.d - 1.0).abs() < 1e-9 && m.b.abs() < 1e-9 && m.c.abs() < 1e-9 && m.e.abs() < 1e-6 && m.f.abs() < 1e-6
}

/// A layer's working image with its effects applied, on the CPU (the reference for the GPU
/// effect stage).
pub fn effect_image(frame: &VideoFrame, fx: &LayerFx) -> crate::Image {
    let (w, h, px) = frame.to_linear_f32_decimated(fx.decimation.max(1) as usize);
    let mut img = crate::Image { w, h, px };
    for op in &fx.ops {
        op.apply(&mut img);
    }
    img
}

/// Execute a plan on the CPU (reference for the GPU compositor).
pub fn execute_cpu(plan: &FramePlan) -> crate::Image {
    match plan {
        FramePlan::Image(img) => img.clone(),
        FramePlan::Layers { width, height, layers } => {
            let mut canvas = crate::Image::new(*width, *height);
            for l in layers {
                if l.adjust {
                    // an adjustment layer: its effects on everything below, drawn over it
                    let mut adj = canvas.clone();
                    for op in l.fx.iter().flat_map(|fx| &fx.ops) {
                        op.apply(&mut adj);
                    }
                    crate::blend::composite(&mut canvas, &adj, l.opacity, l.blend);
                    continue;
                }
                let src = match &l.fx {
                    Some(fx) => effect_image(&l.frame, fx),
                    None => crate::Image { w: l.frame.width as usize, h: l.frame.height as usize, px: l.frame.to_linear_f32() },
                };
                let placed = if l.matrix == Affine::scale(*width as f64, *height as f64) && src.w == 1 && src.h == 1 {
                    crate::Image::filled(*width, *height, src.get(0, 0))
                } else {
                    src.transformed(*width, *height, &l.matrix)
                };
                crate::blend::composite(&mut canvas, &placed, l.opacity, l.blend);
            }
            canvas
        }
    }
}
