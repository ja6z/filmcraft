//! Layer effects (`lfx2`, the object-based effects descriptor): drop shadow, outer glow, inner
//! shadow and inner glow are rendered as images of their own, so an editor can place each one
//! with its blend mode and opacity under (shadow, outer glow) or over (inner effects) its layer.
//!
//! The effect's mask comes from the layer's alpha: offset by distance and angle (shadows; the
//! document's global light when the effect uses it), spread (outer) or choke (inner) by a share of
//! the size, then softened by the rest of the size with a three-pass box blur (a close Gaussian).
//! Other effects (stroke, bevel, overlays, satin) are reported as not reproduced.

use crate::{BLEND_KEYS, Reader};

/// A rendered layer effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    DropShadow,
    OuterGlow,
    InnerShadow,
    InnerGlow,
}

impl EffectKind {
    pub fn label(self) -> &'static str {
        match self {
            EffectKind::DropShadow => "Drop Shadow",
            EffectKind::OuterGlow => "Outer Glow",
            EffectKind::InnerShadow => "Inner Shadow",
            EffectKind::InnerGlow => "Inner Glow",
        }
    }
    /// Drawn under the layer (shadow, outer glow) rather than over it (inner effects).
    pub fn behind(self) -> bool {
        matches!(self, EffectKind::DropShadow | EffectKind::OuterGlow)
    }
}

/// One rendered effect: its colour with the effect's coverage as alpha, positioned on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerEffect {
    pub kind: EffectKind,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Straight-alpha RGBA, `w × h × 4`.
    pub rgba: Vec<u8>,
    /// Index into [`BLEND_KEYS`].
    pub blend: usize,
    /// The effect's own opacity, 0..1 (the layer's opacity applies on top; its fill doesn't).
    pub opacity: f32,
}

// ------------------------------------------------------------------------------------ descriptor

/// A descriptor value (the parts effects use).
#[derive(Clone, Debug, PartialEq)]
enum D {
    Obj(Vec<(String, D)>),
    List(Vec<D>),
    Num(f64),
    Text(String),
    Enum(String),
    Bool(bool),
    Other,
}

impl D {
    fn get(&self, k: &str) -> Option<&D> {
        match self {
            D::Obj(items) => items.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self, k: &str) -> Option<f64> {
        match self.get(k)? {
            D::Num(v) => Some(*v),
            _ => None,
        }
    }
    fn flag(&self, k: &str) -> Option<bool> {
        match self.get(k)? {
            D::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn enumeration(&self, k: &str) -> Option<&str> {
        match self.get(k)? {
            D::Enum(e) => Some(e.as_str()),
            _ => None,
        }
    }
}

fn id(r: &mut Reader) -> Option<String> {
    let n = r.u32("descriptor").ok()? as usize;
    let n = if n == 0 { 4 } else { n };
    Some(r.take(n, "descriptor").ok()?.iter().map(|&c| c as char).collect())
}

fn unicode(r: &mut Reader) -> Option<String> {
    let n = r.u32("descriptor").ok()? as usize;
    let b = r.take(n.checked_mul(2)?, "descriptor").ok()?;
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
    Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
}

fn descriptor(r: &mut Reader, depth: usize) -> Option<D> {
    if depth > 16 {
        return None;
    }
    unicode(r)?;
    id(r)?;
    let n = r.u32("descriptor").ok()? as usize;
    let mut items = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let key = id(r)?;
        let ty: [u8; 4] = r.take(4, "descriptor").ok()?.try_into().ok()?;
        items.push((key, value(r, &ty, depth)?));
    }
    Some(D::Obj(items))
}

fn value(r: &mut Reader, ty: &[u8; 4], depth: usize) -> Option<D> {
    Some(match ty {
        b"Objc" | b"GlbO" => descriptor(r, depth + 1)?,
        b"VlLs" => {
            let n = r.u32("descriptor").ok()? as usize;
            let mut v = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                let t: [u8; 4] = r.take(4, "descriptor").ok()?.try_into().ok()?;
                v.push(value(r, &t, depth + 1)?);
            }
            D::List(v)
        }
        b"doub" => D::Num(f64::from_be_bytes(r.take(8, "descriptor").ok()?.try_into().ok()?)),
        b"UntF" => {
            r.take(4, "descriptor").ok()?;
            D::Num(f64::from_be_bytes(r.take(8, "descriptor").ok()?.try_into().ok()?))
        }
        b"long" => D::Num(r.i32("descriptor").ok()? as f64),
        b"comp" => {
            r.take(8, "descriptor").ok()?;
            D::Other
        }
        b"bool" => D::Bool(r.u8("descriptor").ok()? != 0),
        b"TEXT" => D::Text(unicode(r)?),
        b"enum" => {
            id(r)?;
            D::Enum(id(r)?)
        }
        b"type" | b"GlbC" => {
            unicode(r)?;
            id(r)?;
            D::Other
        }
        b"tdta" | b"alis" | b"Pth " => {
            let n = r.u32("descriptor").ok()? as usize;
            r.take(n, "descriptor").ok()?;
            D::Other
        }
        b"UnFl" => {
            r.take(4, "descriptor").ok()?;
            let n = r.u32("descriptor").ok()? as usize;
            r.take(n.checked_mul(8)?, "descriptor").ok()?;
            D::Other
        }
        _ => return None,
    })
}

/// Blend mode descriptor names → index into [`BLEND_KEYS`].
fn blend_index(name: &str) -> usize {
    const NAMES: [&str; 27] = [
        "Nrml",
        "Dslv",
        "Drkn",
        "Mltp",
        "CBrn",
        "linearBurn",
        "darkerColor",
        "Lghn",
        "Scrn",
        "CDdg",
        "linearDodge",
        "lighterColor",
        "Ovrl",
        "SftL",
        "HrdL",
        "vividLight",
        "linearLight",
        "pinLight",
        "hardMix",
        "Dfrn",
        "Xclu",
        "blendSubtraction",
        "blendDivide",
        "H   ",
        "Strt",
        "Clr ",
        "Lmns",
    ];
    debug_assert_eq!(NAMES.len(), BLEND_KEYS.len());
    NAMES.iter().position(|n| *n == name).unwrap_or(0)
}

/// An effect's settings.
#[derive(Clone, Copy, Debug)]
struct Settings {
    kind: EffectKind,
    blend: usize,
    opacity: f32,
    color: [u8; 3],
    angle: f32,
    distance: f32,
    spread: f32,
    size: f32,
    centre: bool,
}

fn settings(kind: EffectKind, d: &D, scale: f32, global_angle: f32) -> Option<Settings> {
    if d.flag("enab") == Some(false) || d.flag("present") == Some(false) {
        return None;
    }
    let color = match d.get("Clr ") {
        Some(c) => [c.num("Rd  ").unwrap_or(0.0), c.num("Grn ").unwrap_or(0.0), c.num("Bl  ").unwrap_or(0.0)].map(|v| v.round().clamp(0.0, 255.0) as u8),
        None => [0, 0, 0],
    };
    let use_global = d.flag("uglg").unwrap_or(true);
    Some(Settings {
        kind,
        blend: blend_index(d.enumeration("Md  ").unwrap_or("Nrml")),
        opacity: (d.num("Opct").unwrap_or(75.0) / 100.0).clamp(0.0, 1.0) as f32,
        color,
        angle: if use_global { global_angle } else { d.num("lagl").unwrap_or(global_angle as f64) as f32 },
        distance: (d.num("Dstn").unwrap_or(0.0) as f32 * scale).max(0.0),
        spread: (d.num("Ckmt").unwrap_or(0.0) as f32).clamp(0.0, 100.0),
        size: (d.num("blur").unwrap_or(0.0) as f32 * scale).clamp(0.0, 250.0),
        centre: d.enumeration("glwS") == Some("SrcC"),
    })
}

/// The effects of an `lfx2` block: those this module renders, and the names of the others that
/// are switched on.
pub(crate) fn read(data: &[u8], global_angle: f32) -> (Vec<SettingsBox>, Vec<&'static str>) {
    let mut r = Reader::new(data);
    let (Ok(_), Ok(_)) = (r.u32("effects"), r.u32("effects")) else { return (Vec::new(), Vec::new()) };
    let Some(root) = descriptor(&mut r, 0) else { return (Vec::new(), vec!["layer effects"]) };
    if root.flag("masterFXSwitch") == Some(false) {
        return (Vec::new(), Vec::new());
    }
    let scale = (root.num("Scl ").unwrap_or(100.0) / 100.0) as f32;
    let mut out = Vec::new();
    for (key, kind) in [
        ("DrSh", EffectKind::DropShadow),
        ("dropShadowMulti", EffectKind::DropShadow),
        ("OrGl", EffectKind::OuterGlow),
        ("IrSh", EffectKind::InnerShadow),
        ("innerShadowMulti", EffectKind::InnerShadow),
        ("IrGl", EffectKind::InnerGlow),
    ] {
        let items: Vec<&D> = match root.get(key) {
            Some(D::List(v)) => v.iter().collect(),
            Some(d) => vec![d],
            None => Vec::new(),
        };
        out.extend(items.into_iter().filter_map(|d| settings(kind, d, scale, global_angle)).map(SettingsBox));
    }
    let mut other = Vec::new();
    for (keys, name) in [
        (&["FrFX", "frameFXMulti"][..], "Stroke"),
        (&["ebbl"][..], "Bevel & Emboss"),
        (&["SoFi", "solidFillMulti"][..], "Color Overlay"),
        (&["GrFl", "gradientFillMulti"][..], "Gradient Overlay"),
        (&["patternFill"][..], "Pattern Overlay"),
        (&["ChFX"][..], "Satin"),
    ] {
        let on = keys.iter().any(|k| match root.get(k) {
            Some(D::List(v)) => v.iter().any(|d| d.flag("enab") != Some(false)),
            Some(d) => d.flag("enab") != Some(false),
            None => false,
        });
        if on {
            other.push(name);
        }
    }
    (out, other)
}

/// Opaque wrapper so the settings stay private to this module.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SettingsBox(Settings);

// ------------------------------------------------------------------------------------- rendering

/// Render an effect from a layer's straight-alpha RGBA at `(x, y)`, `w × h`.
pub(crate) fn render(s: &SettingsBox, rgba: &[u8], x: i32, y: i32, w: usize, h: usize) -> Option<LayerEffect> {
    let s = s.0;
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return None;
    }
    let rad = s.angle.to_radians();
    let (dx, dy) = match s.kind {
        // light from `angle`: the shadow falls the other way (y grows downwards)
        EffectKind::DropShadow | EffectKind::InnerShadow => ((-rad.cos() * s.distance).round() as i32, (rad.sin() * s.distance).round() as i32),
        _ => (0, 0),
    };
    let hard = s.size * s.spread / 100.0;
    let soft = (s.size - hard).max(0.0);
    let m = (s.distance + s.size).ceil() as usize + 2;
    let (cw, ch) = (w + 2 * m, h + 2 * m);
    let mut a = vec![0.0f32; cw * ch];
    for yy in 0..h {
        for xx in 0..w {
            a[(yy + m) * cw + xx + m] = rgba[(yy * w + xx) * 4 + 3] as f32 / 255.0;
        }
    }
    let cover: Vec<f32> = match s.kind {
        EffectKind::DropShadow | EffectKind::OuterGlow => {
            let mut p = shift(&a, cw, ch, dx, dy);
            dilate(&mut p, cw, ch, hard.round() as usize);
            blur3(&mut p, cw, ch, soft);
            p
        }
        EffectKind::InnerShadow | EffectKind::InnerGlow => {
            let shifted = shift(&a, cw, ch, dx, dy);
            let mut inv: Vec<f32> = shifted.iter().map(|v| 1.0 - v).collect();
            dilate(&mut inv, cw, ch, hard.round() as usize);
            blur3(&mut inv, cw, ch, soft);
            if s.kind == EffectKind::InnerGlow && s.centre {
                inv.iter_mut().for_each(|v| *v = 1.0 - *v);
            }
            inv.iter().zip(&a).map(|(v, al)| v * al).collect()
        }
    };
    // inner effects stay within the layer; outer ones keep their margin
    let (ox, oy, ow, oh) = if s.kind.behind() { (0, 0, cw, ch) } else { (m, m, w, h) };
    let mut out = vec![0u8; ow * oh * 4];
    for yy in 0..oh {
        for xx in 0..ow {
            let c = cover[(yy + oy) * cw + xx + ox];
            let o = &mut out[(yy * ow + xx) * 4..(yy * ow + xx) * 4 + 4];
            o[..3].copy_from_slice(&s.color);
            o[3] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    Some(LayerEffect {
        kind: s.kind,
        x: x - m as i32 + ox as i32,
        y: y - m as i32 + oy as i32,
        w: ow as u32,
        h: oh as u32,
        rgba: out,
        blend: s.blend,
        opacity: s.opacity,
    })
}

/// `p` moved by `(dx, dy)` (zero coming in).
fn shift(p: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> Vec<f32> {
    if dx == 0 && dy == 0 {
        return p.to_vec();
    }
    let mut out = vec![0.0f32; p.len()];
    for y in 0..h as i32 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x - dx;
            if sx >= 0 && sx < w as i32 {
                out[(y * w as i32 + x) as usize] = p[(sy * w as i32 + sx) as usize];
            }
        }
    }
    out
}

/// Separable max filter of radius `r` (spread / choke).
fn dilate(p: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; p.len()];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = p[y * w + a..=y * w + b].iter().copied().fold(0.0, f32::max);
        }
    }
    for x in 0..w {
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            p[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).fold(0.0, f32::max);
        }
    }
}

/// Three box blurs whose combined reach is `size` pixels (≈ a Gaussian of that extent).
fn blur3(p: &mut [f32], w: usize, h: usize, size: f32) {
    let r = (size / 3.0).round() as usize;
    if r == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; p.len()];
    for _ in 0..3 {
        box_pass(p, &mut tmp, w, h, r, true);
        box_pass(&tmp, p, w, h, r, false);
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let k = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = 0.0f32;
        for i in 0..=r.min(n - 1) {
            acc += src[at(line, i)];
        }
        for i in 0..n {
            dst[at(line, i)] = acc * k;
            if i + r + 1 < n {
                acc += src[at(line, i + r + 1)];
            }
            if i >= r {
                acc -= src[at(line, i - r)];
            }
        }
    }
}
