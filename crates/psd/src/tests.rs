//! A tiny PSD writer builds documents with the features the reader handles; real documents
//! (`FILMCRAFT_PSD_SAMPLE=<file.psd>`) are checked by the ignored `sample_document` test.

use super::*;

use crate::testing::*;

fn px(l: &Layer, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * l.w + x) * 4) as usize;
    [l.rgba[i], l.rgba[i + 1], l.rgba[i + 2], l.rgba[i + 3]]
}

#[test]
fn layers_names_bounds_opacity_blend_and_groups() {
    let mut title = layer("Línea 1 · CÉSAR", (2, 3, 6, 2), [255, 140, 0, 255]);
    title.extra.push((b"TySh", vec![0; 4]));
    title.rle = true;
    let mut glow = layer("Brillo", (0, 0, 10, 10), [255, 200, 120, 128]);
    glow.blend = "scrn";
    glow.opacity = 128;
    glow.fill = 128;
    let doc = psd(
        10,
        10,
        &[
            layer("Background", (0, 0, 10, 10), [10, 10, 10, 255]),
            group_end(),
            layer("Hidden by its group", (0, 0, 2, 2), [1, 2, 3, 255]),
            group_start("Oculto", true, 255),
            group_end(),
            title,
            glow,
            group_start("Textos", false, 128),
        ],
        [10, 10, 10],
    );
    let d = parse(&doc).unwrap();
    assert_eq!((d.width, d.height, d.depth), (10, 10, 8));
    let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Background", "Hidden by its group", "Línea 1 · CÉSAR", "Brillo"]);
    let t = &d.layers[2];
    assert_eq!((t.x, t.y, t.w, t.h), (2, 3, 6, 2));
    assert_eq!(t.kind, LayerKind::Text);
    assert_eq!(px(t, 5, 1), [255, 140, 0, 255], "RLE pixels");
    assert_eq!(t.groups, ["Textos"]);
    assert!((t.opacity - 128.0 / 255.0).abs() < 1e-3, "the group's opacity multiplies");
    let g = &d.layers[3];
    assert_eq!(BLEND_KEYS[g.blend], "scrn");
    assert!((g.opacity - (128.0f32 / 255.0).powi(3)).abs() < 1e-3, "layer × fill × group: {}", g.opacity);
    assert_eq!(px(g, 0, 0)[3], 128, "straight alpha kept");
    assert!(!d.layers[1].visible, "a hidden group hides its layers");
    assert!(d.layers[0].visible && d.layers[0].groups.is_empty());
    assert_eq!(d.composite.as_ref().map(|c| c[..4].to_vec()), Some(vec![10, 10, 10, 255]));
}

#[test]
fn masks_and_clipped_layers_and_adjustments() {
    let base = layer("César", (0, 0, 4, 4), [100, 150, 200, 255]);
    // the mask hides the left half of the layer
    let mut masked = base.clone();
    masked.name = "Masked";
    masked.mask = Some(((0, 0, 2, 4), vec![0; 8], 255));
    let mut clipped = layer("Clipped glow", (2, 0, 4, 4), [255, 255, 255, 255]);
    clipped.clip = true;
    let doc = psd(
        6,
        4,
        &[
            masked,
            base,
            clipped,
            adjustment("Bajar blancos (Curves)", b"curv", curves_data(&[(0, 0), (255, 200)]), true),
            adjustment("Tono cálido (Photo Filter)", b"phfl", photo_filter_data([236, 138, 0], 50, false), true),
            adjustment("Levels on everything", b"levl", vec![0; 4], false),
        ],
        [0, 0, 0],
    );
    let d = parse(&doc).unwrap();
    let names: Vec<&str> = d.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Masked", "César", "Clipped glow"], "adjustments are baked or dropped");
    assert_eq!(px(&d.layers[0], 0, 0)[3], 0, "the mask hides the left half");
    assert_eq!(px(&d.layers[0], 3, 0)[3], 255, "the mask's default shows the rest");
    // clipped to César (x 0..4): the glow (x 2..6) keeps x 2..4 only
    let g = &d.layers[2];
    assert_eq!((px(g, 0, 0)[3], px(g, 1, 0)[3], px(g, 2, 0)[3]), (255, 255, 0));
    // Curves (whites to 200) then a warm filter at 50 %, baked into César only
    let c = &d.layers[1];
    assert_eq!(c.baked, ["Curves", "Photo Filter"]);
    let p = px(c, 0, 0);
    let curved = [100.0f32 * 200.0 / 255.0, 150.0 * 200.0 / 255.0, 200.0 * 200.0 / 255.0];
    let warm = [curved[0] * (0.5 + 0.5 * 236.0 / 255.0), curved[1] * (0.5 + 0.5 * 138.0 / 255.0), curved[2] * 0.5];
    for i in 0..3 {
        assert!((p[i] as f32 - warm[i]).abs() <= 2.0, "channel {i}: {} vs {}", p[i], warm[i]);
    }
    assert_eq!(px(&d.layers[0], 3, 0)[..3], [100, 150, 200], "unclipped layers are untouched");
    assert!(d.warnings.iter().any(|w| w.contains("Levels")), "{:?}", d.warnings);
}

#[test]
fn photo_filter_can_keep_luminosity() {
    let p = filter([120, 120, 120], [236.0 / 255.0, 138.0 / 255.0, 0.0], 0.3, true);
    let luma = |q: [u8; 3]| 0.299 * q[0] as f32 + 0.587 * q[1] as f32 + 0.114 * q[2] as f32;
    assert!((luma(p) - 120.0).abs() < 1.5, "{p:?}");
    assert!(p[0] > p[2], "warmer: {p:?}");
}

#[test]
fn a_monotone_curve_through_its_points() {
    let lut = spline_lut(&mut vec![(0.0, 0.0), (64.0, 56.0), (190.0, 192.0), (255.0, 234.0)]);
    assert_eq!((lut[0], lut[64], lut[190], lut[255]), (0, 56, 192, 234));
    assert!(lut.windows(2).all(|w| w[0] <= w[1]), "monotone");
}

#[test]
fn refuses_what_it_cannot_read() {
    assert_eq!(parse(b"GIF89a"), Err(PsdError::NotPsd));
    let mut v = psd(2, 2, &[], [0, 0, 0]);
    v[5] = 2;
    assert!(matches!(parse(&v), Err(PsdError::Unsupported(w)) if w.contains("psb")));
    let mut v = psd(2, 2, &[], [0, 0, 0]);
    v[25] = 4;
    assert!(matches!(parse(&v), Err(PsdError::Unsupported(w)) if w.contains("CMYK")));
    let v = psd(4, 4, &[layer("A", (0, 0, 4, 4), [1, 2, 3, 255])], [0, 0, 0]);
    for cut in [10, 30, 60, v.len() / 2] {
        assert!(parse(&v[..cut]).is_err(), "truncated at {cut}");
    }
}

/// `FILMCRAFT_PSD_SAMPLE=/path/poster.psd cargo test -p filmcraft-psd -- --ignored --nocapture`
#[test]
#[ignore = "needs a real document in FILMCRAFT_PSD_SAMPLE"]
fn sample_document() {
    let Some(path) = std::env::var_os("FILMCRAFT_PSD_SAMPLE") else { return };
    let d = parse(&std::fs::read(path).unwrap()).unwrap();
    eprintln!("{}×{} {}-bit, composite {}", d.width, d.height, d.depth, d.composite.is_some());
    for l in &d.layers {
        eprintln!(
            "{:<40} {:?} {:>5},{:>5} {:>5}×{:<5} {:>4.0}% {} {}{}{}",
            l.name,
            l.kind,
            l.x,
            l.y,
            l.w,
            l.h,
            l.opacity * 100.0,
            BLEND_KEYS[l.blend],
            if l.visible { "" } else { "hidden " },
            if l.clipped { "clipped " } else { "" },
            l.baked.join("+")
        );
    }
    for w in &d.warnings {
        eprintln!("warning: {w}");
    }
}
