//! File ▸ Import PSD as Sequence: a Photoshop document becomes a sequence of its canvas size with
//! one video track per layer (the bottom layer on V1, tracks named after the layers), each layer a
//! still clip of the Still Image Default Duration placed where it sits on the canvas (Motion
//! position, 100 % scale, anchored at the layer's own centre so it scales and rotates in place),
//! with the layer's opacity and blend mode. Hidden layers come in as disabled clips. The sequence
//! mixes its layers in display values (Composite in Linear Color off), as Photoshop does, so
//! semi-transparent layers look as they did in the design.
//!
//! The layers ([`filmcraft_psd`]) are written as trimmed PNGs to `PSD Layers/<document>/` next to
//! the project (else in the data directory) and imported into a bin named after the document; the
//! PSD itself is only read. Clipped Curves / Photo Filter adjustments are baked into their layer;
//! what can't be reproduced (some layer effects, other adjustments) is listed in `warnings`.
//!
//! Layer effects that [`filmcraft_psd`] renders (drop shadow, outer glow, inner shadow, inner
//! glow) get tracks of their own named "<layer> · <effect>", with the effect's blend mode and
//! opacity (× the layer's opacity, not its fill): right under the layer for shadows and outer
//! glows, right over it for inner effects, so each can be animated or switched off on its own.

use std::path::{Path, PathBuf};

use filmcraft_project::{ItemId, Param, ParamValue, SequenceSettings, TrackKind};
use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, f64_p, str_p};
use crate::{EngineError, Result, Session};

/// A file-name-safe version of a layer name.
fn safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Layer".into() } else { s.chars().take(80).collect() }
}

/// Where the layer files of document `stem` go: `PSD Layers/<stem>` next to the project, else in
/// the data directory, else the temp folder; numbered when taken.
fn layers_dir(s: &Session, stem: &str, dest: Option<&str>) -> PathBuf {
    let root = match dest {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => s
            .path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(Path::to_path_buf)
            .or_else(|| s.prefs_path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf))
            .unwrap_or_else(std::env::temp_dir)
            .join("PSD Layers"),
    };
    let mut n = 1;
    loop {
        let d = root.join(if n == 1 { stem.to_string() } else { format!("{stem} {n}") });
        if !d.exists() {
            return d;
        }
        n += 1;
    }
}

/// `file.importPsdAsSequence {path, fps?, durationSeconds?, destination?}`.
pub fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.importPsdAsSequence";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "need `path`"))?.to_string();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let doc = filmcraft_psd::parse(&bytes).map_err(|e| EngineError::Other(format!("{}: {e}", file_name(&path))))?;
    let stem = Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "PSD".into());
    let rate = match f64_p(p, "fps") {
        Some(f) if f > 0.0 => FrameRate::from_f64(f),
        _ => s.active_sequence().map_or(FrameRate::FPS_30, |q| q.settings.frame_rate),
    };
    let dur = match f64_p(p, "durationSeconds") {
        Some(d) if d > 0.0 => rate.snap_nearest(Tick::from_seconds_f64(d)).max(rate.frame_duration()),
        _ => s.prefs.timeline.still_duration(rate),
    };

    // ---- the layers (and their shadows / glows) as files, bottom first: an effect drawn behind
    // its layer (drop shadow, outer glow) comes right under it, an inner effect right over it
    let layers: Vec<&filmcraft_psd::Layer> = doc.layers.iter().filter(|l| l.w > 0 && l.h > 0 && !l.rgba.is_empty()).collect();
    if layers.is_empty() {
        return Err(EngineError::Other(format!("{} has no layers with pixels to import", file_name(&path))));
    }
    let mut elements: Vec<Element> = Vec::new();
    for l in &layers {
        let fx = |e: &filmcraft_psd::LayerEffect| Element {
            name: format!("{} · {}", l.name, e.kind.label()),
            x: e.x,
            y: e.y,
            w: e.w,
            h: e.h,
            rgba: e.rgba.clone(),
            opacity: e.opacity * l.layer_opacity,
            blend: e.blend,
            visible: l.visible,
            effect_of: Some(l.name.clone()),
        };
        elements.extend(l.effects.iter().filter(|e| e.kind.behind()).map(fx));
        elements.push(Element {
            name: l.name.clone(),
            x: l.x,
            y: l.y,
            w: l.w,
            h: l.h,
            rgba: l.rgba.clone(),
            opacity: l.opacity,
            blend: l.blend,
            visible: l.visible,
            effect_of: None,
        });
        elements.extend(l.effects.iter().filter(|e| !e.kind.behind()).map(fx));
    }
    let dir = layers_dir(s, &stem, str_p(p, "destination"));
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    let mut files = Vec::with_capacity(elements.len());
    for (i, el) in elements.iter().enumerate() {
        let png = filmcraft_export::encode_png(el.rgba.clone(), el.w, el.h).map_err(|e| EngineError::Other(format!("“{}”: {e}", el.name)))?;
        let f = dir.join(format!("{:02} {}.png", i + 1, safe(&el.name)));
        let fs = f.to_string_lossy().into_owned();
        s.services.write_file(&fs, &png).map_err(|e| EngineError::Other(format!("{fs}: {e}")))?;
        files.push(fs);
    }
    let r = s.execute("file.import", json!({"paths": files}))?;
    let items: Vec<ItemId> = r["items"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(ItemId).collect()).unwrap_or_default();
    if items.len() != elements.len() {
        return Err(EngineError::Other(format!("imported {} of {} layer files from {}", items.len(), elements.len(), dir.display())));
    }

    // ---- the sequence: one track per layer or effect, bottom first
    // layers mix in display values, as in Photoshop, so the design looks the same
    let settings = SequenceSettings { width: doc.width, height: doc.height, frame_rate: rate, composite_linear: false, ..Default::default() };
    let seq_label = s.prefs.labels.defaults.sequence;
    let placed: Vec<(String, i32, i32, u32, u32, f32, usize, bool)> =
        elements.iter().map(|l| (l.name.clone(), l.x, l.y, l.w, l.h, l.opacity, l.blend, l.visible)).collect();
    let n = placed.len();
    let seq = s.edit("Import PSD as Sequence", |pr, st| {
        let bin = pr.add_bin(&format!("{stem} (PSD)"), None);
        pr.move_to_bin(&items, Some(bin));
        let id = pr.new_sequence(&stem, settings, n, 2, Some(bin));
        if let Some(it) = pr.item_mut(id) {
            it.label = seq_label;
        }
        for (k, ((name, x, y, w, h, opacity, blend, visible), item)) in placed.iter().zip(&items).enumerate() {
            let Some(mut ti) = pr.make_track_item(*item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, dur), rate) else { continue };
            ti.name = name.clone();
            ti.enabled = *visible;
            for e in &mut ti.effects {
                match e.effect.as_str() {
                    "motion" => {
                        let centre = filmcraft_geom::Vec2 { x: *x as f64 + *w as f64 / 2.0, y: *y as f64 + *h as f64 / 2.0 };
                        e.params.insert("position".into(), Param::new(ParamValue::Vec2(centre)));
                        e.params.insert("scale".into(), Param::new(ParamValue::Float(100.0)));
                    }
                    "opacity" => {
                        e.params.insert("opacity".into(), Param::new(ParamValue::Float((*opacity as f64 * 1000.0).round() / 10.0)));
                        e.params.insert("blend".into(), Param::new(ParamValue::Choice(*blend as u32)));
                    }
                    _ => {}
                }
            }
            if let Some(t) = pr.sequence_mut(id).and_then(|q| q.video_tracks.get_mut(k)) {
                t.name = name.clone();
                t.items.push(ti);
            }
        }
        st.active_sequence = Some(id);
        if !st.open_sequences.contains(&id) {
            st.open_sequences.push(id);
        }
        Ok(id)
    })?;
    s.events.push(crate::Event::OpenSequence(seq));
    Ok(json!({
        "sequence": seq.0,
        "size": [doc.width, doc.height],
        "folder": dir.to_string_lossy(),
        "layers": layers.iter().map(|l| json!({
            "name": l.name, "visible": l.visible, "opacity": l.opacity, "kind": format!("{:?}", l.kind), "baked": l.baked,
            "blend": filmcraft_project::effect::BLEND_MODES.get(l.blend).copied().unwrap_or("Normal"),
            "effects": l.effects.iter().map(|e| e.kind.label()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "tracks": elements.iter().zip(&items).enumerate().map(|(k, (el, i))| json!({
            "track": k + 1, "name": el.name, "item": i.0, "effectOf": el.effect_of, "opacity": el.opacity,
            "blend": filmcraft_project::effect::BLEND_MODES.get(el.blend).copied().unwrap_or("Normal"),
        })).collect::<Vec<_>>(),
        "warnings": doc.warnings,
    }))
}

/// One picture to place: a layer, or one of its effects.
struct Element {
    name: String,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    opacity: f32,
    blend: usize,
    visible: bool,
    /// The layer an effect belongs to (None for the layer itself).
    effect_of: Option<String>,
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "file.importPsdAsSequence",
        label: "Import PSD as Sequence…",
        menu: &["File"],
        shortcut: None,
        params: r#"{"path":str,"fps":f64?,"durationSeconds":f64?,"destination":str?}"#,
        enabled: always,
        run: import,
        journal: true,
    }]
}
