//! Lumetri on the preview's GPU path ([`crate::gpufx::gpu_ops`]): the baked LUT, the vignette
//! and the sharpen ops, run through their CPU reference (`FxOp::apply`, what the shaders
//! reproduce), against the exact CPU Lumetri the export uses.
//!
//! Tolerance: PSNR of the 8-bit sRGB pictures ≥ 42 dB (the 33³ tetrahedral LUT's interpolation
//! error), and the worst pixel within 6 levels.

use super::*;
use crate::effects::FxCtx;
use crate::gpufx::{FxOp, gpu_ops};
use filmcraft_project::{EffectInstance, ParamValue, find_effect};

fn cx(working: filmcraft_color::WorkingSpace) -> FxCtx<'static> {
    FxCtx { t: Tick::ZERO, px_scale: 1.0, seconds: 0.0, timecode: "", clip_name: "", project: None, env: None, working }
}

fn lumetri(params: &[(&str, ParamValue)]) -> EffectInstance {
    let mut e = find_effect("lumetri").expect("lumetri").instance();
    for (k, v) in params {
        e.params.get_mut(*k).unwrap_or_else(|| panic!("lumetri.{k}")).value = v.clone();
    }
    e
}

fn fl(v: f64) -> ParamValue {
    ParamValue::Float(v)
}

fn on() -> ParamValue {
    ParamValue::Bool(true)
}

/// A flat, log-looking picture (lifted blacks, low saturation, like D-Log M before conversion):
/// hue across, lightness down, a grey ramp at the bottom; linear premultiplied, opaque.
fn picture(w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / (w - 1) as f32, y as f32 / (h - 1) as f32);
            let c = if y > h * 7 / 8 {
                [fx; 3]
            } else {
                let rgb = filmcraft_color::hsl_to_rgb(fx, 0.35, 0.18 + 0.64 * (1.0 - fy));
                rgb.map(filmcraft_color::srgb_to_linear)
            };
            let i = (y * w + x) * 4;
            img.px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
    }
    img
}

/// (PSNR dB, worst channel difference in 8-bit levels).
fn compare(a: &Image, b: &Image) -> (f64, u8) {
    let (a, b) = (a.to_rgba8(), b.to_rgba8());
    let mut se = 0f64;
    let mut worst = 0u8;
    let mut n = 0f64;
    for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
        for k in 0..3 {
            let d = p[k].abs_diff(q[k]);
            worst = worst.max(d);
            se += (d as f64).powi(2);
            n += 1.0;
        }
    }
    let mse = se / n;
    (if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }, worst)
}

fn gpu_path(e: &EffectInstance, cx: &FxCtx, img: &Image) -> (Image, Vec<FxOp>) {
    let ops = gpu_ops(e, cx, img.w, img.h).expect("Lumetri runs on the GPU");
    let mut out = img.clone();
    for op in &ops {
        op.apply(&mut out);
    }
    (out, ops)
}

#[test]
fn baked_lumetri_matches_the_cpu() {
    let input = format!("builtin:{}", crate::luts::builtins()[0].id);
    let look = crate::luts::builtins().iter().find(|b| b.id.starts_with("look-")).map(|b| format!("builtin:{}", b.id)).unwrap_or_default();
    let roll_off = ParamValue::Curve(vec![[0.0, 0.0], [0.25, 0.245], [0.55, 0.555], [0.75, 0.735], [0.88, 0.83], [1.0, 0.905]]);
    let cases: Vec<(&str, Vec<(&str, ParamValue)>)> = vec![
        // the Corazón grade: camera LUT + basic correction + a highlight roll-off + vignette
        (
            "input LUT, basic, curve, vignette",
            vec![
                ("input_lut", ParamValue::Text(input.clone())),
                ("exposure", fl(-0.65)),
                ("contrast", fl(28.0)),
                ("whites", fl(32.0)),
                ("blacks", fl(-12.0)),
                ("saturation", fl(97.0)),
                ("temperature", fl(-6.0)),
                ("curves_on", on()),
                ("curve_luma", roll_off.clone()),
                ("vignette_on", on()),
                ("vignette_amount", fl(-0.6)),
                ("vignette_midpoint", fl(55.0)),
                ("vignette_feather", fl(70.0)),
            ],
        ),
        ("basic only", vec![("exposure", fl(0.6)), ("shadows", fl(40.0)), ("highlights", fl(-30.0)), ("tint", fl(12.0))]),
        (
            "creative",
            vec![
                ("creative_on", on()),
                ("look_lut", ParamValue::Text(look)),
                ("look_intensity", fl(70.0)),
                ("faded_film", fl(25.0)),
                ("vibrance", fl(30.0)),
                ("creative_sat", fl(120.0)),
                ("shadow_tint", ParamValue::Color([0.3, 0.5, 0.7, 1.0])),
                ("highlight_tint", ParamValue::Color([0.7, 0.55, 0.4, 1.0])),
            ],
        ),
        (
            "wheels, hue curve, sharpen",
            vec![
                ("wheels_on", on()),
                ("wheel_shadows", ParamValue::Vec2(filmcraft_geom::Vec2::new(0.1, -0.05))),
                ("wheel_highlights_l", fl(-15.0)),
                ("curves_on", on()),
                ("hue_vs_sat", ParamValue::Curve(vec![[0.0, 0.5], [0.33, 0.8], [0.66, 0.4], [1.0, 0.5]])),
                ("creative_on", on()),
                ("sharpen", fl(40.0)),
            ],
        ),
    ];
    let img = picture(256, 160);
    let c = cx(filmcraft_color::WorkingSpace::Rec709);
    for (name, params) in cases {
        let e = lumetri(&params);
        let mut cpu = img.clone();
        crate::effects::apply(&mut cpu, &e, &c);
        let (gpu, ops) = gpu_path(&e, &c, &img);
        let (db, worst) = compare(&cpu, &gpu);
        eprintln!("{name}: {} ops, {db:.1} dB, worst {worst}", ops.len());
        assert!(db >= 42.0 && worst <= 6, "{name}: {db:.1} dB, worst {worst} levels");
        assert!(ops.iter().all(FxOp::gpu_ok));
    }
}

#[test]
fn vignette_splits_the_bake_and_sharpen_is_an_unsharp_op() {
    let c = cx(filmcraft_color::WorkingSpace::Rec709);
    let ops = gpu_ops(&lumetri(&[("exposure", fl(0.3))]), &c, 64, 36).expect("gpu");
    assert!(matches!(ops.as_slice(), [FxOp::Lut3 { .. }]));
    let ops =
        gpu_ops(&lumetri(&[("vignette_on", on()), ("vignette_amount", fl(-1.0)), ("creative_on", on()), ("sharpen", fl(50.0))]), &c, 64, 36).expect("gpu");
    assert!(matches!(ops.as_slice(), [FxOp::Lut3 { .. }, FxOp::Vignette { .. }, FxOp::Lut3 { .. }, FxOp::Unsharp { .. }]), "{ops:?}");
}

#[test]
fn bakes_are_cached_and_follow_the_parameters() {
    let c = cx(filmcraft_color::WorkingSpace::Rec709);
    let key = |e: &EffectInstance| match gpu_ops(e, &c, 8, 8).as_deref() {
        Some([FxOp::Lut3 { key, lut }]) => (*key, Arc::as_ptr(lut) as usize),
        other => panic!("{other:?}"),
    };
    let a = lumetri(&[("contrast", fl(20.0))]);
    let b = lumetri(&[("contrast", fl(21.0))]);
    assert_eq!(key(&a), key(&a), "the same parameters reuse the bake");
    assert_ne!(key(&a).0, key(&b).0, "a changed slider bakes again");
}

#[test]
fn hdr_and_hsl_refine_stay_on_the_cpu() {
    let e = lumetri(&[("exposure", fl(0.5))]);
    assert!(gpu_ops(&e, &cx(filmcraft_color::WorkingSpace::Rec2100Pq), 8, 8).is_none(), "HDR grades are not baked");
    let e = lumetri(&[("hsl_on", on()), ("hsl_blur", fl(30.0))]);
    assert!(gpu_ops(&e, &cx(filmcraft_color::WorkingSpace::Rec709), 8, 8).is_none(), "HSL Blur looks at neighbours");
    // the CPU render keeps its exact Lumetri: `FxOp::eval` has no op for it
    assert!(FxOp::eval(&lumetri(&[("exposure", fl(0.5))]), &cx(filmcraft_color::WorkingSpace::Rec709), 8, 8).is_none());
}
