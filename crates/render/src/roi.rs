//! Visible-region processing for the CPU layer path.
//!
//! A clip's standard effects used to run on its whole decoded picture even when Motion shows only
//! part of it: a 16:9 shot reframed into a 9:16 sequence shows about a third of its width, so two
//! thirds of the effect work was thrown away. When every standard effect on a clip is *local* —
//! a pixel's result depends only on that pixel, or on neighbours within a known radius — the
//! working picture is cropped to the part that lands in the output plus that radius, the effects
//! run on the crop, and the crop is placed where it came from. Inside the visible area the result
//! is the same as before: pointwise effects are computed per pixel, and for neighbourhood effects
//! (box-Gaussian blurs, unsharp, directional blur) the crop keeps every pixel whose influence can
//! reach a visible one. Pixels within the margin may differ at the crop's inner edges, but they
//! are never shown.
//!
//! Effects that depend on the picture's geometry or size (Crop, flips, Mirror, Offset, Transform,
//! Lumetri's vignette, frame-relative positions…), analysis effects, effect masks and opacity
//! masks keep the whole-picture path.

use filmcraft_geom::Affine;
use filmcraft_project::EffectInstance;

use crate::effects::FxCtx;
use crate::gpufx::FxOp;

/// Below this share of the picture's pixels the crop is not worth making.
pub(crate) const MAX_ROI_SHARE: f64 = 0.85;

/// Tests switch visible-region processing off to compare with the whole-picture path. Results
/// inside the visible area are the same either way, so other tests running meanwhile are unaffected.
#[cfg(test)]
pub(crate) static DISABLED_FOR_TEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Visible-region processing is on (always, outside tests).
pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    {
        !DISABLED_FOR_TEST.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// Extra pixels (x, y) of working picture an effect needs around the visible region, or None when
/// it depends on the whole picture. `w`×`h` is the working picture's size.
pub(crate) fn effect_margin(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<(f32, f32)> {
    if !e.enabled {
        return Some((0.0, 0.0));
    }
    if !e.masks.is_empty() {
        return None;
    }
    if e.effect == "lumetri" {
        return lumetri_margin(e, cx, w, h);
    }
    if e.effect == "crop" {
        return crop_margin(e, cx);
    }
    // Only effects whose CPU path is the GPU-capable op (vfx handles none of these ids).
    if !crate::gpufx::GPU_EFFECTS.contains(&e.effect.as_str()) {
        return None;
    }
    let op = FxOp::eval(e, cx, w, h)?;
    let sum = |r: &[u32]| r.iter().map(|v| *v as f32).sum::<f32>();
    match &op {
        FxOp::BrightnessContrast { .. }
        | FxOp::ProcAmp { .. }
        | FxOp::Tint { .. }
        | FxOp::BlackWhite
        | FxOp::ColorBalance { .. }
        | FxOp::LeaveColor { .. }
        | FxOp::ChangeToColor { .. }
        | FxOp::ColorPass { .. }
        | FxOp::Gamma { .. }
        | FxOp::Levels { .. }
        | FxOp::Extract { .. }
        | FxOp::Invert { .. }
        | FxOp::Posterize { .. }
        | FxOp::AscCdl { .. }
        | FxOp::ChannelMix { .. }
        | FxOp::ColorReplace { .. }
        | FxOp::AlphaAdjust { .. } => Some((0.0, 0.0)),
        FxOp::Gaussian { rx, ry, .. } | FxOp::Unsharp { rx, ry, .. } => Some((sum(rx) + 1.0, sum(ry) + 1.0)),
        FxOp::DirectionalBlur { dx, dy, .. } => Some((dx.abs() + 2.0, dy.abs() + 2.0)),
        _ => None,
    }
}

/// Crop cuts percentages of the whole picture (frame-relative), except with nothing cut and no
/// Zoom: then it only feathers the picture's border by a pixel width, which is local. The real
/// borders stay inside the region (it is clamped to the picture) and feather as before; the crop's
/// inner edges get feathered too, but only within the margin, which is never shown.
fn crop_margin(e: &EffectInstance, cx: &FxCtx) -> Option<(f32, f32)> {
    let cut = ["left", "top", "right", "bottom"].iter().any(|k| e.f64_at(k, cx.t).abs() > 1e-6);
    let zoom = e.param("zoom").and_then(|p| p.value.as_bool()).unwrap_or(false);
    if cut || zoom {
        return None;
    }
    let fe = (e.f64_at("feather", cx.t) as f32 * cx.px_scale).max(0.0);
    fe.is_finite().then_some((fe + 2.0, fe + 2.0))
}

/// Lumetri is local unless its vignette (frame-relative) or HSL secondary (refine blur / denoise)
/// is in use; its creative Sharpen reads a small neighbourhood.
fn lumetri_margin(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<(f32, f32)> {
    let on = |id: &str| e.param(id).and_then(|p| p.value.as_bool());
    let f = |id: &str| e.f64_at(id, cx.t);
    if on("vignette_on").unwrap_or(true) && f("vignette_amount").abs() > 1e-6 {
        return None;
    }
    if on("hsl_on").unwrap_or(false) {
        return None;
    }
    if on("creative_on").unwrap_or(true) && f("sharpen").abs() > 1e-3 {
        // the radius `effects::lumetri` sharpens with
        let r = 1.2 * cx.px_scale.max(0.35);
        let (rx, ry) = crate::effects::gaussian_boxes(w, h, r, r);
        let sum = |v: &[u32]| v.iter().map(|x| *x as f32).sum::<f32>();
        return Some((sum(&rx) + 1.0, sum(&ry) + 1.0));
    }
    Some((0.0, 0.0))
}

/// The part `[x0, y0, x1, y1]` of a `lw`×`lh` working picture to keep when `effects` all are local:
/// what `m` (picture pixels → output pixels) puts into the `ow`×`oh` output, plus the effects'
/// margins (they add up along the chain) and two pixels for the final bilinear placement. None
/// when an effect needs the whole picture, the region is most of the picture anyway, or nothing
/// is visible.
pub(crate) fn visible_region<'a>(
    lw: usize,
    lh: usize,
    m: &Affine,
    ow: usize,
    oh: usize,
    effects: impl Iterator<Item = &'a EffectInstance>,
    cx: &FxCtx,
) -> Option<[usize; 4]> {
    if lw == 0 || lh == 0 || ow == 0 || oh == 0 {
        return None;
    }
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    for e in effects {
        let (ex, ey) = effect_margin(e, cx, lw, lh)?;
        mx += ex;
        my += ey;
    }
    let inv = m.inverse()?;
    let corners = [(0.0, 0.0), (ow as f64, 0.0), (0.0, oh as f64), (ow as f64, oh as f64)];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in corners {
        let p = inv.apply(filmcraft_geom::Vec2::new(x, y));
        if !(p.x.is_finite() && p.y.is_finite()) {
            return None;
        }
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let pad_x = mx as f64 + 2.0;
    let pad_y = my as f64 + 2.0;
    let rx0 = (x0 - pad_x).floor().clamp(0.0, lw as f64) as usize;
    let ry0 = (y0 - pad_y).floor().clamp(0.0, lh as f64) as usize;
    let rx1 = (x1 + pad_x).ceil().clamp(0.0, lw as f64) as usize;
    let ry1 = (y1 + pad_y).ceil().clamp(0.0, lh as f64) as usize;
    if rx1 <= rx0 || ry1 <= ry0 {
        return None;
    }
    let share = ((rx1 - rx0) * (ry1 - ry0)) as f64 / (lw * lh) as f64;
    (share <= MAX_ROI_SHARE).then_some([rx0, ry0, rx1, ry1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Image;
    use filmcraft_project::{ParamValue, find_effect};
    use filmcraft_time::Tick;

    fn cx(px_scale: f32) -> FxCtx<'static> {
        FxCtx { t: Tick::ZERO, px_scale, seconds: 0.0, timecode: "", clip_name: "", project: None, env: None, working: filmcraft_color::WorkingSpace::Rec709 }
    }

    fn effect(id: &str, params: &[(&str, ParamValue)]) -> EffectInstance {
        let mut e = find_effect(id).unwrap_or_else(|| panic!("{id}")).instance();
        for (k, v) in params {
            e.params.get_mut(*k).unwrap_or_else(|| panic!("{id}.{k}")).value = v.clone();
        }
        e
    }

    /// A busy test picture: gradients, a hard edge and partial alpha.
    fn picture(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let a = if (x / 7 + y / 5) % 3 == 0 { 0.6 } else { 1.0 };
                let r = (x as f32 / w as f32).powf(1.3);
                let g = (y as f32 / h as f32).sqrt();
                let b = if x > w / 2 { 0.8 } else { 0.1 };
                let i = (y * w + x) * 4;
                img.px[i..i + 4].copy_from_slice(&[r * a, g * a, b * a, a]);
            }
        }
        img
    }

    fn crop(img: &Image, r: [usize; 4]) -> Image {
        img.cropped(r[0], r[1], r[2], r[3])
    }

    /// Effects on the crop give the whole-picture result everywhere outside the margin.
    #[test]
    fn local_effects_on_the_crop_match_the_whole_picture() {
        let fl = ParamValue::Float;
        let cases: Vec<Vec<EffectInstance>> = vec![
            vec![effect("brightness_contrast", &[("brightness", fl(-25.0)), ("contrast", fl(20.0))])],
            vec![effect("gaussian_blur", &[("blurriness", fl(45.0))])],
            vec![effect("gaussian_blur", &[("blurriness", fl(30.0)), ("repeat_edge", ParamValue::Bool(true))])],
            vec![effect("gaussian_blur", &[("blurriness", fl(45.0))]), effect("brightness_contrast", &[("brightness", fl(-25.0))])],
            vec![effect("unsharp_mask", &[("amount", fl(150.0)), ("radius", fl(3.0))])],
            vec![effect("directional_blur", &[("length", fl(12.0)), ("direction", fl(30.0))])],
            vec![effect("lumetri", &[("temperature", fl(16.0)), ("saturation", fl(104.0)), ("vignette_amount", fl(0.0))])],
            vec![effect("tint", &[]), effect("gaussian_blur", &[("blurriness", fl(20.0))]), effect("gaussian_blur", &[("blurriness", fl(10.0))])],
            // the vertical reframes of the promo: border feather only, alone and before a blur
            vec![effect("crop", &[("feather", fl(40.0))])],
            vec![effect("crop", &[("feather", fl(60.0))]), effect("gaussian_blur", &[("blurriness", fl(30.0))])],
        ];
        let (w, h) = (240usize, 135usize);
        let full_img = picture(w, h);
        // the output shows a vertical slice of the picture (like a 9:16 reframe)
        let m = Affine::translate(-100.0, 0.0);
        for chain in cases {
            let c = cx(0.5);
            let r = visible_region(w, h, &m, 45, 135, chain.iter(), &c)
                .unwrap_or_else(|| panic!("{:?} should be local", chain.iter().map(|e| &e.effect).collect::<Vec<_>>()));
            let mut full = full_img.clone();
            let mut part = crop(&full_img, r);
            for e in &chain {
                crate::effects::apply(&mut full, e, &c);
                crate::effects::apply(&mut part, e, &c);
            }
            // compare the visible columns 100..145 (all rows)
            let mut worst = 0.0f32;
            for y in 0..h {
                for x in 100..145 {
                    let a = full.get(x, y);
                    let b = part.get(x - r[0], y - r[1]);
                    for k in 0..4 {
                        worst = worst.max((a[k] - b[k]).abs());
                    }
                }
            }
            assert!(worst < 1e-5, "{:?}: visible pixels differ by {worst}", chain.iter().map(|e| &e.effect).collect::<Vec<_>>());
            assert!(r[2] - r[0] < w, "{:?}: the crop is narrower than the picture", chain.iter().map(|e| &e.effect).collect::<Vec<_>>());
        }
    }

    #[test]
    fn geometry_and_frame_relative_effects_keep_the_whole_picture() {
        let fl = ParamValue::Float;
        let m = Affine::translate(-100.0, 0.0);
        let c = cx(1.0);
        for e in [
            effect("crop", &[("left", fl(10.0))]),
            effect("crop", &[("zoom", ParamValue::Bool(true))]),
            effect("crop", &[("bottom", fl(5.0)), ("feather", fl(40.0))]),
            effect("horizontal_flip", &[]),
            effect("lumetri", &[("vignette_amount", fl(-2.0))]),
            effect("mirror", &[]),
            effect("vignette", &[]),
        ] {
            assert!(visible_region(240, 135, &m, 45, 135, std::iter::once(&e), &c).is_none(), "{} must not be cropped", e.effect);
        }
        // a masked effect too
        let mut blur = effect("gaussian_blur", &[("blurriness", fl(10.0))]);
        blur.masks.push(filmcraft_project::Mask::new(
            "m",
            filmcraft_project::MaskPath::ellipse(filmcraft_geom::Vec2::new(50.0, 50.0), filmcraft_geom::Vec2::new(20.0, 20.0)),
        ));
        assert!(visible_region(240, 135, &m, 45, 135, std::iter::once(&blur), &c).is_none());
    }

    #[test]
    fn regions_that_are_most_of_the_picture_or_degenerate_are_skipped() {
        let c = cx(1.0);
        let b = effect("brightness_contrast", &[]);
        // whole picture visible
        assert!(visible_region(240, 135, &Affine::IDENTITY, 240, 135, std::iter::once(&b), &c).is_none());
        // nothing visible / empty sizes / singular transform
        assert!(visible_region(240, 135, &Affine::translate(-1000.0, 0.0), 45, 135, std::iter::once(&b), &c).is_none());
        assert!(visible_region(0, 0, &Affine::IDENTITY, 45, 135, std::iter::once(&b), &c).is_none());
        assert!(visible_region(240, 135, &Affine::scale(0.0, 0.0), 45, 135, std::iter::once(&b), &c).is_none());
        // a slice is kept, with the 2 px placement pad
        let r = visible_region(240, 135, &Affine::translate(-100.0, 0.0), 45, 135, std::iter::once(&b), &c).expect("slice");
        assert_eq!(r, [98, 0, 147, 135]);
    }
}
