//! A tiny PSD writer builds documents with the features the reader handles; real documents
//! (`FILMCRAFT_PSD_SAMPLE=<file.psd>`) are checked by the ignored `sample_document` test.

use super::*;

use crate::testing::*;
use crate::{EffectKind, LayerEffect};

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
        let fx: Vec<_> = l.effects.iter().map(|e| (e.kind.label(), BLEND_KEYS[e.blend], e.opacity, e.x, e.y, e.w, e.h)).collect();
        eprintln!(
            "{:<40} {:?} {:>5},{:>5} {:>5}×{:<5} {:>4.0}% {} {}{}{} fx {:?}",
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
            l.baked.join("+"),
            fx
        );
    }
    for w in &d.warnings {
        eprintln!("warning: {w}");
    }
}

fn alpha_at(e: &LayerEffect, x: i32, y: i32) -> u8 {
    let (ex, ey) = (x - e.x, y - e.y);
    if ex < 0 || ey < 0 || ex >= e.w as i32 || ey >= e.h as i32 {
        return 0;
    }
    e.rgba[(ey as usize * e.w as usize + ex as usize) * 4 + 3]
}

fn with_fx(rect: (i32, i32, u32, u32), fx: &[Fx]) -> Layer {
    let mut l = layer("Fx", rect, [255, 255, 255, 255]);
    l.extra.push((b"lfx2", effects_data(fx)));
    let d = parse(&psd(80, 80, &[l], [0, 0, 0])).unwrap();
    d.layers.into_iter().next().unwrap()
}

#[test]
fn drop_shadows_fall_away_from_the_light() {
    let l = with_fx((10, 10, 20, 10), &[Fx { blend: "Mltp", distance: 4.0, ..Fx::new("DrSh") }]);
    assert_eq!(l.effects.len(), 1);
    let e = &l.effects[0];
    assert_eq!((e.kind, BLEND_KEYS[e.blend], e.opacity), (EffectKind::DropShadow, "mul ", 1.0));
    assert!(e.kind.behind());
    // light from 90° (above): the shadow is the layer moved 4 px down
    assert_eq!(alpha_at(e, 15, 22), 255, "below the layer");
    assert_eq!(alpha_at(e, 15, 12), 0, "the top rows moved down");
    assert_eq!(e.rgba[..3], [0, 0, 0]);
    // the global light (120° unless the document says otherwise): down and to the right
    let l = with_fx((10, 10, 20, 10), &[Fx { global: true, distance: 10.0, ..Fx::new("DrSh") }]);
    let e = &l.effects[0];
    assert_eq!(alpha_at(e, 10 + 5, 10 + 9), 255);
    assert_eq!(alpha_at(e, 10 + 4, 10 + 8), 0, "the corner moved by (5, 9)");
}

#[test]
fn glows_fade_with_distance_and_stay_inside_or_out() {
    let l = with_fx((20, 20, 30, 30), &[Fx { blend: "Scrn", opacity: 70.0, color: [255.0, 138.0, 30.0], size: 9.0, ..Fx::new("OrGl") }]);
    let e = &l.effects[0];
    assert_eq!((e.kind, BLEND_KEYS[e.blend]), (EffectKind::OuterGlow, "scrn"));
    assert!((e.opacity - 0.7).abs() < 1e-6);
    assert_eq!(e.rgba[..3], [255, 138, 30]);
    let (near, mid, far) = (alpha_at(e, 18, 35), alpha_at(e, 15, 35), alpha_at(e, 8, 35));
    assert!(near > mid && mid > far, "{near} > {mid} > {far}");
    assert_eq!(far, 0, "beyond the size");
    // an inner glow lights the edges, not the middle, and nothing outside
    let l = with_fx((20, 20, 30, 30), &[Fx { size: 6.0, color: [255.0, 255.0, 0.0], ..Fx::new("IrGl") }]);
    let e = &l.effects[0];
    assert!(!e.kind.behind());
    assert_eq!((e.x, e.y, e.w, e.h), (20, 20, 30, 30), "inner effects keep the layer's bounds");
    assert!(alpha_at(e, 20, 35) > 100, "edge");
    assert_eq!(alpha_at(e, 35, 35), 0, "middle");
    // an inner shadow falls inside the top edge for light from above
    let l = with_fx((20, 20, 30, 30), &[Fx { distance: 5.0, ..Fx::new("IrSh") }]);
    let e = &l.effects[0];
    assert_eq!(alpha_at(e, 35, 21), 255, "inside the top edge");
    assert_eq!(alpha_at(e, 35, 45), 0, "the bottom edge is lit");
}

#[test]
fn effects_it_cannot_draw_are_reported() {
    let mut l = layer("Con borde", (0, 0, 10, 10), [255, 255, 255, 255]);
    l.extra.push((b"lfx2", effects_data(&[Fx::new("FrFX"), Fx { size: 3.0, ..Fx::new("OrGl") }])));
    let d = parse(&psd(20, 20, &[l], [0, 0, 0])).unwrap();
    assert_eq!(d.layers[0].effects.len(), 1, "the glow is drawn");
    assert!(d.warnings.iter().any(|w| w.contains("Con borde") && w.contains("Stroke")), "{:?}", d.warnings);
}
