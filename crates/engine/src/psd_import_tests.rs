//! Import PSD as Sequence end to end: a written document becomes a sequence of its size with one
//! named track per layer, each layer in place with its opacity and blend mode.

use filmcraft_project::{ParamValue, find_effect};
use filmcraft_psd::testing::{Fx, effects_data, layer, psd};
use serde_json::json;

use crate::Session;
use crate::media_test_util::{frame_rgba, tmp_dir};

#[test]
fn a_psd_becomes_a_layered_sequence() {
    let dir = tmp_dir("psd-import");
    let mut title = layer("TÍTULO", (2, 1, 4, 2), [250, 120, 0, 255]);
    title.blend = "scrn";
    title.opacity = 128;
    let mut hidden = layer("Oculta", (0, 0, 3, 3), [0, 255, 0, 255]);
    hidden.hidden = true;
    let doc = psd(8, 6, &[layer("Fondo", (0, 0, 8, 6), [20, 40, 60, 255]), title, hidden], [20, 40, 60]);
    let path = dir.join("Poster.psd");
    std::fs::write(&path, doc).unwrap();
    let out = dir.join("layers");

    let mut s = Session::default();
    let r = s
        .execute("file.importPsdAsSequence", json!({"path": path.to_string_lossy(), "fps": 30.0, "durationSeconds": 2.0, "destination": out.to_string_lossy()}))
        .unwrap();
    assert_eq!(r["size"], json!([8, 6]));
    assert_eq!(r["layers"].as_array().unwrap().len(), 3, "{r}");
    assert!(std::fs::read_dir(out.join("Poster")).unwrap().count() == 3, "one PNG per layer, the PSD untouched");

    let seq = s.active_sequence().unwrap().clone();
    assert_eq!((seq.settings.width, seq.settings.height), (8, 6));
    assert!(!seq.settings.composite_linear, "layers mix in display values, as in Photoshop");
    let names: Vec<&str> = seq.video_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Fondo", "TÍTULO", "Oculta"], "bottom layer on V1");
    let clip = &seq.video_tracks[1].items[0];
    assert_eq!(clip.name, "TÍTULO");
    assert_eq!(clip.duration, filmcraft_time::FrameRate::FPS_30.tick_of(60));
    let param = |effect: &str, p: &str| clip.effects.iter().find(|e| e.effect == effect).and_then(|e| e.params.get(p)).map(|p| p.value.clone());
    assert_eq!(param("motion", "position"), Some(ParamValue::Vec2(filmcraft_geom::Vec2 { x: 4.0, y: 2.0 })), "centred on the layer");
    assert_eq!(param("opacity", "opacity"), Some(ParamValue::Float(50.2)));
    let screen = filmcraft_project::effect::BLEND_MODES.iter().position(|m| *m == "Screen").unwrap() as u32;
    assert_eq!(param("opacity", "blend"), Some(ParamValue::Choice(screen)));
    assert!(!seq.video_tracks[2].items[0].enabled, "hidden layers come in disabled");
    assert!(find_effect("opacity").is_some());
    // the items sit in a bin named after the document
    let bins = s.execute("project.inspect", json!({})).unwrap().to_string();
    assert!(bins.contains("Poster (PSD)"), "{bins}");

    // the picture: the title lightens its rectangle only, the hidden layer shows nowhere
    let (w, _, px) = frame_rgba(&mut s, 0, 1.0);
    let at = |x: usize, y: usize| px[(y * w + x) * 4..][..3].to_vec();
    let bg = at(7, 5);
    assert!((bg[0] as i32 - 20).abs() <= 2 && (bg[2] as i32 - 60).abs() <= 2, "background {bg:?}");
    assert_eq!(at(1, 1), bg, "left of the title");
    assert_eq!(at(0, 0), bg, "the hidden layer is off");
    let t = at(3, 1);
    assert!(t[0] > bg[0] + 40, "the title is screened over the background: {t:?} vs {bg:?}");
    assert_eq!(at(3, 3), bg, "below the title");

    // File ▸ Import (and drops) of a .psd make the same layered sequence
    let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap();
    assert_eq!(r["sequences"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(s.active_sequence().unwrap().video_tracks.len(), 3);

    assert!(s.execute("file.importPsdAsSequence", json!({"path": dir.join("missing.psd").to_string_lossy()})).is_err());
    std::fs::write(dir.join("fake.psd"), b"not a psd").unwrap();
    let e = s.execute("file.importPsdAsSequence", json!({"path": dir.join("fake.psd").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("not a Photoshop document"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sequence ▸ Composite in Linear Color: a 50 % white layer over black is half the light when on,
/// half the display value (as in Photoshop) when off; the setting is undoable and only written
/// to the project when off.
#[test]
fn composite_in_linear_color_toggles_how_layers_mix() {
    let dir = tmp_dir("psd-linear");
    let mut white = layer("Blanco", (0, 0, 4, 4), [255, 255, 255, 255]);
    white.opacity = 128;
    std::fs::write(dir.join("Mix.psd"), psd(4, 4, &[layer("Negro", (0, 0, 4, 4), [0, 0, 0, 255]), white], [0, 0, 0])).unwrap();
    let mut s = Session::default();
    s.execute("file.importPsdAsSequence", json!({"path": dir.join("Mix.psd").to_string_lossy(), "destination": dir.to_string_lossy()})).unwrap();
    let display = frame_rgba(&mut s, 0, 1.0).2[0];
    assert!((display as i32 - 128).abs() <= 2, "half the display value: {display}");
    let json = serde_json::to_string(&s.active_sequence().unwrap().settings).unwrap();
    assert!(json.contains("composite_linear"), "written when off");
    let r = s.execute("sequence.compositeInLinearColor", json!({})).unwrap();
    assert_eq!(r["enabled"], json!(true));
    let linear = frame_rgba(&mut s, 0, 1.0).2[0];
    assert!((linear as i32 - 188).abs() <= 2, "half the light: {linear}");
    let json = serde_json::to_string(&s.active_sequence().unwrap().settings).unwrap();
    assert!(!json.contains("composite_linear"), "the default isn't written");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.active_sequence().unwrap().settings.composite_linear, "undoable");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shadows and glows come in as tracks of their own around their layer, with their blend mode and
/// opacity, and show in the picture.
#[test]
fn layer_effects_get_their_own_tracks() {
    let dir = tmp_dir("psd-effects");
    let mut title = layer("Título", (10, 10, 20, 10), [255, 255, 255, 255]);
    title.opacity = 128;
    title.extra.push((
        b"lfx2",
        effects_data(&[
            Fx { blend: "Mltp", opacity: 50.0, distance: 4.0, ..Fx::new("DrSh") },
            Fx { blend: "Nrml", size: 3.0, color: [255.0, 0.0, 0.0], ..Fx::new("IrGl") },
        ]),
    ));
    let mut sparks = layer("Chispas", (40, 40, 8, 8), [255, 200, 0, 255]);
    sparks.extra.push((b"lfx2", effects_data(&[Fx { blend: "Scrn", opacity: 70.0, size: 6.0, color: [255.0, 138.0, 30.0], ..Fx::new("OrGl") }])));
    let doc = psd(64, 64, &[layer("Fondo", (0, 0, 64, 64), [40, 40, 40, 255]), title, sparks], [40, 40, 40]);
    let path = dir.join("Fx.psd");
    std::fs::write(&path, doc).unwrap();
    let mut s = Session::default();
    let r = s.execute("file.importPsdAsSequence", json!({"path": path.to_string_lossy(), "destination": dir.join("out").to_string_lossy()})).unwrap();
    let seq = s.active_sequence().unwrap().clone();
    let names: Vec<&str> = seq.video_tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Fondo", "Título · Drop Shadow", "Título", "Título · Inner Glow", "Chispas · Outer Glow", "Chispas"], "{r}");
    let param = |track: usize, effect: &str, p: &str| {
        seq.video_tracks[track].items[0].effects.iter().find(|e| e.effect == effect).and_then(|e| e.params.get(p)).map(|p| p.value.clone())
    };
    let mode = |m: &str| ParamValue::Choice(filmcraft_project::effect::BLEND_MODES.iter().position(|x| *x == m).unwrap() as u32);
    assert_eq!(param(1, "opacity", "blend"), Some(mode("Multiply")));
    assert_eq!(param(1, "opacity", "opacity"), Some(ParamValue::Float(25.1)), "the effect's 50 % × the layer's 50 %");
    assert_eq!(param(4, "opacity", "blend"), Some(mode("Screen")));
    assert_eq!(r["layers"][1]["effects"], json!(["Drop Shadow", "Inner Glow"]));
    // the picture: the glow lights the background around the sparks, the shadow darkens below the title
    let (w, _, px) = frame_rgba(&mut s, 0, 1.0);
    let at = |x: usize, y: usize| px[(y * w + x) * 4..][..3].to_vec();
    assert!(at(38, 44)[0] > 50, "glow next to the sparks: {:?}", at(38, 44));
    assert!(at(15, 22)[0] < 40, "shadow under the title: {:?}", at(15, 22));
    assert_eq!(at(2, 2)[0], at(60, 2)[0], "the background elsewhere");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `FILMCRAFT_PSD_SAMPLE=/path/poster.psd cargo test -p filmcraft-engine --lib real_psd -- --ignored --nocapture`:
/// the imported sequence's first frame against the document's merged image.
#[test]
#[ignore = "needs a real document in FILMCRAFT_PSD_SAMPLE"]
fn real_psd_matches_its_merged_image() {
    let Some(path) = std::env::var_os("FILMCRAFT_PSD_SAMPLE") else { return };
    let dir = tmp_dir("psd-real");
    let doc = filmcraft_psd::parse(&std::fs::read(&path).unwrap()).unwrap();
    let mut s = Session::default();
    let r = s.execute("file.importPsdAsSequence", json!({"path": path.to_string_lossy(), "destination": dir.to_string_lossy()})).unwrap();
    eprintln!("{} layers; warnings: {}", r["layers"].as_array().unwrap().len(), r["warnings"]);
    let (w, h, px) = frame_rgba(&mut s, 0, 1.0);
    assert_eq!((w as u32, h as u32), (doc.width, doc.height));
    let merged = doc.composite.expect("merged image");
    let q = crate::media_test_util::psnr(&px, &merged);
    eprintln!("first frame vs the PSD's merged image: {q:.1} dB");
    let png = filmcraft_export::encode_png(px, w as u32, h as u32).unwrap();
    std::fs::write(dir.join("frame0.png"), png).unwrap();
    eprintln!("frame: {}", dir.join("frame0.png").display());
    assert!(q > 24.0, "{q:.1} dB");
}
