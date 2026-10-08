//! Clean-room reader of Adobe Photoshop documents (`.psd`, version 1), written from Adobe's public
//! *Adobe Photoshop File Formats Specification* (the header, colour mode data, image resources,
//! layer and mask information and image data sections). It reads what FilmCraft needs to bring a
//! design in as a layered sequence (File ▸ Import PSD as Sequence):
//!
//! - the canvas size; RGB or Grayscale at 8 or 16 bits per channel (other modes, 32-bit and the
//!   large-document `.psb` format are refused with [`PsdError::Unsupported`]);
//! - every layer's name (Unicode), bounds and pixels (raw, PackBits RLE or ZIP), as straight-alpha
//!   RGBA with the layer mask applied; its opacity (layer × fill × enclosing groups), blend mode,
//!   visibility (a hidden group hides its layers), kind (pixel, type, shape, fill, smart object,
//!   adjustment) and enclosing groups. Type, shape and fill layers are read from the pixels
//!   Photoshop stores for them;
//! - clipping masks: a layer clipped to the one below keeps only the pixels over its base, and
//!   clipped **Curves** and **Photo Filter** adjustment layers are applied to their base layer;
//! - the merged image.
//!
//! Not reproduced (reported in [`Document::warnings`]): layer effects (shadows, glows, strokes),
//! adjustment layers that are not clipped or of other kinds, and blend modes of clipped layers.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::fmt;

/// Photoshop's blend mode keys, in the order of its blend menu (the same order and names as
/// FilmCraft's Opacity ▸ Blend Mode list). Index 0 is Normal.
pub const BLEND_KEYS: [&str; 27] = [
    "norm", "diss", "dark", "mul ", "idiv", "lbrn", "dkCl", "lite", "scrn", "div ", "lddg", "lgCl", "over", "sLit", "hLit", "vLit", "lLit", "pLit", "hMix",
    "diff", "smud", "fsub", "fdiv", "hue ", "sat ", "colr", "lum ",
];

#[derive(Clone, Debug, PartialEq)]
pub enum PsdError {
    /// Not a Photoshop document.
    NotPsd,
    /// A valid document this reader does not handle (PSB, CMYK, 32-bit…).
    Unsupported(String),
    /// The data ends before a section does.
    Truncated(&'static str),
    Corrupt(String),
}

impl fmt::Display for PsdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PsdError::NotPsd => write!(f, "not a Photoshop document"),
            PsdError::Unsupported(w) => write!(f, "unsupported Photoshop document: {w}"),
            PsdError::Truncated(w) => write!(f, "the Photoshop document ends inside its {w}"),
            PsdError::Corrupt(w) => write!(f, "damaged Photoshop document: {w}"),
        }
    }
}

impl std::error::Error for PsdError {}

pub type Result<T> = std::result::Result<T, PsdError>;

/// What a layer is.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerKind {
    Pixel,
    Text,
    Shape,
    Fill,
    SmartObject,
    /// An adjustment layer (its kind: "Curves", "Photo Filter", "Levels"…).
    Adjustment(String),
}

/// One layer, ready to place: pixels within its bounds on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    /// Top-left on the canvas (may lie outside it) and size in pixels.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// Straight-alpha RGBA, 8 bits, `w × h × 4`, layer mask and clipping applied.
    pub rgba: Vec<u8>,
    /// Layer opacity × fill opacity × the enclosing groups' opacity, 0..1.
    pub opacity: f32,
    /// Index into [`BLEND_KEYS`].
    pub blend: usize,
    /// Shown (false inside a hidden group too).
    pub visible: bool,
    pub kind: LayerKind,
    /// Enclosing groups, outermost first.
    pub groups: Vec<String>,
    /// Clipped to the layer below.
    pub clipped: bool,
    /// Has layer effects (not reproduced).
    pub has_effects: bool,
    /// Adjustments applied to this layer's pixels (clipped adjustment layers above it).
    pub baked: Vec<String>,
}

/// A Photoshop document.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    /// Bits per channel of the file (pixels are always returned as 8-bit).
    pub depth: u16,
    /// Pixel layers, bottom first (adjustment layers baked or dropped, groups flattened).
    pub layers: Vec<Layer>,
    /// The merged image, RGBA 8-bit, `width × height × 4`, when the file has one.
    pub composite: Option<Vec<u8>>,
    /// What could not be reproduced.
    pub warnings: Vec<String>,
}

// ------------------------------------------------------------------------------------------ bytes

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|e| *e <= self.b.len()).ok_or(PsdError::Truncated(what))?;
        let s = &self.b[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }
    fn u16(&mut self, what: &'static str) -> Result<u16> {
        let s = self.take(2, what)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn i16(&mut self, what: &'static str) -> Result<i16> {
        Ok(self.u16(what)? as i16)
    }
    fn u32(&mut self, what: &'static str) -> Result<u32> {
        let s = self.take(4, what)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }
    /// A section introduced by a 32-bit length: its bytes.
    fn section(&mut self, what: &'static str) -> Result<&'a [u8]> {
        let n = self.u32(what)? as usize;
        self.take(n, what)
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.p)
    }
}

// ------------------------------------------------------------------------------------------ parse

/// One layer record before its pixels are decoded.
struct Record {
    name: String,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    channels: Vec<(i16, usize)>,
    blend: usize,
    opacity: u8,
    fill: u8,
    clipping: bool,
    hidden: bool,
    mask: Option<MaskInfo>,
    /// 0 none, 1/2 group (open/closed) top record, 3 the hidden group end marker.
    section: u32,
    kind: LayerKind,
    has_effects: bool,
    curves: Option<Vec<u8>>,
    photo_filter: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct MaskInfo {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    default: u8,
    disabled: bool,
}

/// Read a document.
pub fn parse(bytes: &[u8]) -> Result<Document> {
    let mut r = Reader::new(bytes);
    if r.take(4, "header")? != b"8BPS" {
        return Err(PsdError::NotPsd);
    }
    match r.u16("header")? {
        1 => {}
        2 => return Err(PsdError::Unsupported("large document format (.psb)".into())),
        v => return Err(PsdError::Corrupt(format!("version {v}"))),
    }
    r.take(6, "header")?;
    let channels = r.u16("header")?;
    let height = r.u32("header")?;
    let width = r.u32("header")?;
    let depth = r.u16("header")?;
    let mode = r.u16("header")?;
    if !matches!(depth, 8 | 16) {
        return Err(PsdError::Unsupported(format!("{depth} bits per channel")));
    }
    let gray = match mode {
        3 => false,
        1 => true,
        m => return Err(PsdError::Unsupported(format!("colour mode {}", mode_name(m)))),
    };
    if width == 0 || height == 0 || width > 300_000 || height > 300_000 {
        return Err(PsdError::Corrupt(format!("canvas {width}×{height}")));
    }
    r.section("colour mode data")?;
    r.section("image resources")?;
    let lm = r.section("layer and mask information")?;
    let mut warnings = Vec::new();
    let raw = layers_section(lm, depth, gray, &mut warnings)?;
    let composite = image_data(&mut r, width, height, channels, depth, gray).ok();
    let layers = bake_clipping(raw, &mut warnings);
    Ok(Document { width, height, depth, layers, composite, warnings })
}

fn mode_name(m: u16) -> &'static str {
    match m {
        0 => "Bitmap",
        2 => "Indexed",
        4 => "CMYK",
        7 => "Multichannel",
        8 => "Duotone",
        9 => "Lab",
        _ => "unknown",
    }
}

/// The layer and mask information section: the layers (8-bit layers here; 16-bit documents keep
/// theirs in an `Lr16` block at the end).
/// A layer and, for an adjustment layer, its raw settings.
type Flat = (Layer, Option<Vec<u8>>);

fn layers_section(b: &[u8], depth: u16, gray: bool, warnings: &mut Vec<String>) -> Result<Vec<Flat>> {
    if b.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(b);
    let info = r.section("layer info")?;
    if !info.is_empty() {
        return layer_info(info, depth, gray, warnings);
    }
    // global layer mask info, then additional blocks (Lr16 / Lr32)
    if r.left() >= 4 {
        r.section("global layer mask")?;
    }
    while r.left() >= 12 {
        let sig = r.take(4, "additional layer information")?;
        let key = r.take(4, "additional layer information")?;
        let data = r.section("additional layer information")?;
        if sig != b"8BIM" && sig != b"8B64" {
            break;
        }
        match key {
            b"Lr16" => return layer_info(data, 16, gray, warnings),
            b"Lr32" => return Err(PsdError::Unsupported("32 bits per channel layers".into())),
            _ => {}
        }
    }
    Ok(Vec::new())
}

fn layer_info(b: &[u8], depth: u16, gray: bool, warnings: &mut Vec<String>) -> Result<Vec<Flat>> {
    let mut r = Reader::new(b);
    let count = r.i16("layer count")?.unsigned_abs() as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(record(&mut r)?);
    }
    // channel image data, record by record
    let mut layers: Vec<(Record, Option<Vec<u8>>)> = Vec::with_capacity(count);
    for rec in records {
        let w = (rec.right - rec.left).max(0) as usize;
        let h = (rec.bottom - rec.top).max(0) as usize;
        let mut planes: Vec<(i16, Vec<u8>)> = Vec::new();
        for &(id, len) in &rec.channels {
            let data = r.take(len, "layer pixels")?;
            let (cw, ch) = match (id, rec.mask) {
                (-2 | -3, Some(m)) => ((m.right - m.left).max(0) as usize, (m.bottom - m.top).max(0) as usize),
                _ => (w, h),
            };
            if cw == 0 || ch == 0 || data.len() < 2 {
                continue;
            }
            let plane = channel(data, cw, ch, depth).map_err(|e| PsdError::Corrupt(format!("layer “{}”: {e}", rec.name)))?;
            planes.push((id, plane));
        }
        let rgba = (w > 0 && h > 0).then(|| assemble(&rec, &planes, w, h, gray));
        layers.push((rec, rgba));
    }
    Ok(flatten(layers, warnings))
}

fn record(r: &mut Reader) -> Result<Record> {
    let top = r.i32("layer record")?;
    let left = r.i32("layer record")?;
    let bottom = r.i32("layer record")?;
    let right = r.i32("layer record")?;
    let n = r.u16("layer record")? as usize;
    let mut channels = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.i16("layer record")?;
        let len = r.u32("layer record")? as usize;
        channels.push((id, len));
    }
    if r.take(4, "layer record")? != b"8BIM" {
        return Err(PsdError::Corrupt("layer record signature".into()));
    }
    let key = r.take(4, "layer record")?;
    let blend = BLEND_KEYS.iter().position(|k| k.as_bytes() == key).unwrap_or(0);
    let opacity = r.u8("layer record")?;
    let clipping = r.u8("layer record")? != 0;
    let flags = r.u8("layer record")?;
    r.u8("layer record")?;
    let extra = r.section("layer record")?;
    let mut e = Reader::new(extra);
    let mask_data = e.section("layer mask")?;
    let mask = (mask_data.len() >= 18).then(|| {
        let mut m = Reader::new(mask_data);
        let (t, l, b, rr) = (m.i32("").unwrap_or(0), m.i32("").unwrap_or(0), m.i32("").unwrap_or(0), m.i32("").unwrap_or(0));
        let default = m.u8("").unwrap_or(0);
        let mflags = m.u8("").unwrap_or(0);
        MaskInfo { top: t, left: l, bottom: b, right: rr, default, disabled: mflags & 0x02 != 0 }
    });
    e.section("blending ranges")?;
    let nlen = e.u8("layer name")? as usize;
    let raw = e.take(nlen, "layer name")?;
    let pad = (4 - (1 + nlen) % 4) % 4;
    e.take(pad.min(e.left()), "layer name")?;
    let mut rec = Record {
        name: raw.iter().map(|&c| c as char).collect(),
        top,
        left,
        bottom,
        right,
        channels,
        blend,
        opacity,
        fill: 255,
        clipping,
        hidden: flags & 0x02 != 0,
        mask,
        section: 0,
        kind: LayerKind::Pixel,
        has_effects: false,
        curves: None,
        photo_filter: None,
    };
    let (mut vector, mut fill) = (false, false);
    while e.left() >= 12 {
        let sig = e.take(4, "additional layer information")?;
        if sig != b"8BIM" && sig != b"8B64" {
            break;
        }
        let key: [u8; 4] = e.take(4, "additional layer information")?.try_into().map_err(|_| PsdError::Truncated("additional layer information"))?;
        let data = e.section("additional layer information")?;
        match &key {
            b"luni" if data.len() >= 4 => {
                let n = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                let units: Vec<u16> = data[4..].as_chunks::<2>().0.iter().take(n).map(|c| u16::from_be_bytes(*c)).collect();
                let s = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
                if !s.is_empty() {
                    rec.name = s;
                }
            }
            b"iOpa" if !data.is_empty() => rec.fill = data[0],
            b"lsct" | b"lsdk" if data.len() >= 4 => rec.section = u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
            b"TySh" | b"tySh" => rec.kind = LayerKind::Text,
            b"vmsk" | b"vsms" | b"vscg" | b"vstk" => vector = true,
            b"SoCo" | b"GdFl" | b"PtFl" => fill = true,
            b"SoLd" | b"PlLd" | b"SoLE" => rec.kind = LayerKind::SmartObject,
            b"lfx2" | b"lrFX" | b"lmfx" => rec.has_effects = true,
            b"curv" => {
                rec.kind = LayerKind::Adjustment("Curves".into());
                rec.curves = Some(data.to_vec());
            }
            b"phfl" => {
                rec.kind = LayerKind::Adjustment("Photo Filter".into());
                rec.photo_filter = Some(data.to_vec());
            }
            k => {
                if let Some(name) = adjustment_name(k) {
                    rec.kind = LayerKind::Adjustment(name.into());
                }
            }
        }
    }
    if rec.kind == LayerKind::Pixel {
        if vector {
            rec.kind = LayerKind::Shape;
        } else if fill {
            rec.kind = LayerKind::Fill;
        }
    }
    Ok(rec)
}

fn adjustment_name(key: &[u8; 4]) -> Option<&'static str> {
    Some(match key {
        b"levl" => "Levels",
        b"brit" => "Brightness/Contrast",
        b"hue2" | b"hue " => "Hue/Saturation",
        b"blnc" => "Color Balance",
        b"selc" => "Selective Color",
        b"mixr" => "Channel Mixer",
        b"grdm" => "Gradient Map",
        b"thrs" => "Threshold",
        b"nvrt" => "Invert",
        b"post" => "Posterize",
        b"expA" => "Exposure",
        b"vibA" => "Vibrance",
        b"blwh" => "Black & White",
        b"clrL" => "Color Lookup",
        _ => return None,
    })
}

/// Decode one channel to 8-bit samples.
fn channel(data: &[u8], w: usize, h: usize, depth: u16) -> std::result::Result<Vec<u8>, String> {
    let bps = (depth / 8) as usize;
    let comp = u16::from_be_bytes([data[0], data[1]]);
    let body = &data[2..];
    let row = w * bps;
    let raw: Vec<u8> = match comp {
        0 => body.get(..row * h).ok_or("short raw channel")?.to_vec(),
        1 => {
            let counts = body.get(..h * 2).ok_or("short RLE row table")?;
            let mut p = h * 2;
            let mut out = Vec::with_capacity(row * h);
            for c in counts.as_chunks::<2>().0 {
                let n = u16::from_be_bytes(*c) as usize;
                let src = body.get(p..p + n).ok_or("short RLE row")?;
                p += n;
                packbits(src, row, &mut out)?;
            }
            out
        }
        2 | 3 => {
            let mut v = miniz_oxide::inflate::decompress_to_vec_zlib(body).map_err(|e| format!("ZIP: {e:?}"))?;
            v.resize(row * h, 0);
            if comp == 3 {
                unpredict(&mut v, w, h, bps);
            }
            v
        }
        c => return Err(format!("compression {c}")),
    };
    Ok(if bps == 2 { raw.as_chunks::<2>().0.iter().map(|s| s[0]).collect() } else { raw })
}

/// PackBits: one row of `row` bytes appended to `out`.
fn packbits(src: &[u8], row: usize, out: &mut Vec<u8>) -> std::result::Result<(), String> {
    let start = out.len();
    let mut i = 0;
    while i < src.len() && out.len() - start < row {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            out.extend_from_slice(src.get(i..i + k).ok_or("short literal run")?);
            i += k;
        } else if n != -128 {
            let v = *src.get(i).ok_or("short repeat run")?;
            out.extend(std::iter::repeat_n(v, (1 - n as isize) as usize));
            i += 1;
        }
    }
    out.resize(start + row, 0);
    Ok(())
}

/// Undo ZIP-with-prediction: each sample is stored as the difference from the one to its left.
fn unpredict(v: &mut [u8], w: usize, h: usize, bps: usize) {
    for y in 0..h {
        let r = &mut v[y * w * bps..(y + 1) * w * bps];
        if bps == 2 {
            let mut prev = 0u16;
            for c in r.as_chunks_mut::<2>().0 {
                let s = u16::from_be_bytes(*c).wrapping_add(prev);
                *c = s.to_be_bytes();
                prev = s;
            }
        } else {
            for x in 1..r.len() {
                r[x] = r[x].wrapping_add(r[x - 1]);
            }
        }
    }
}

/// Planes → straight RGBA within the layer bounds, with the user mask applied.
fn assemble(rec: &Record, planes: &[(i16, Vec<u8>)], w: usize, h: usize, gray: bool) -> Vec<u8> {
    let plane = |id: i16| planes.iter().find(|(i, _)| *i == id).map(|(_, p)| p.as_slice());
    let (r, g, b) = if gray { (plane(0), plane(0), plane(0)) } else { (plane(0), plane(1), plane(2)) };
    let a = plane(-1);
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let px = &mut out[i * 4..i * 4 + 4];
        px[0] = r.and_then(|p| p.get(i)).copied().unwrap_or(0);
        px[1] = g.and_then(|p| p.get(i)).copied().unwrap_or(0);
        px[2] = b.and_then(|p| p.get(i)).copied().unwrap_or(0);
        px[3] = a.map_or(255, |p| p.get(i).copied().unwrap_or(0));
    }
    if let (Some(m), Some(mask)) = (rec.mask.filter(|m| !m.disabled), plane(-2)) {
        let mw = (m.right - m.left).max(0) as usize;
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = (rec.left + x as i32, rec.top + y as i32);
                let v = if cx >= m.left && cx < m.right && cy >= m.top && cy < m.bottom {
                    mask.get((cy - m.top) as usize * mw + (cx - m.left) as usize).copied().unwrap_or(m.default)
                } else {
                    m.default
                };
                let al = &mut out[(y * w + x) * 4 + 3];
                *al = ((*al as u32 * v as u32 + 127) / 255) as u8;
            }
        }
    }
    out
}

/// Groups flattened into names, visibility and opacity; records become layers (bottom first).
fn flatten(records: Vec<(Record, Option<Vec<u8>>)>, warnings: &mut Vec<String>) -> Vec<Flat> {
    // records run bottom → top; a group is its end marker (3), its layers, then its folder (1/2)
    struct Group {
        name: String,
        visible: bool,
        opacity: f32,
    }
    let mut stack: Vec<Group> = Vec::new();
    let mut out: Vec<Flat> = Vec::new();
    for (rec, rgba) in records.into_iter().rev() {
        let alpha = rec.opacity as f32 / 255.0 * rec.fill as f32 / 255.0;
        match rec.section {
            1 | 2 => {
                stack.push(Group { name: rec.name, visible: !rec.hidden, opacity: rec.opacity as f32 / 255.0 });
                continue;
            }
            3 => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let visible = !rec.hidden && stack.iter().all(|g| g.visible);
        let opacity = alpha * stack.iter().map(|g| g.opacity).product::<f32>();
        let groups: Vec<String> = stack.iter().map(|g| g.name.clone()).collect();
        let w = (rec.right - rec.left).max(0) as u32;
        let h = (rec.bottom - rec.top).max(0) as u32;
        if rec.has_effects && visible {
            warnings.push(format!("“{}”: layer effects (shadows, glows, strokes) are not imported", rec.name));
        }
        let settings = rec.curves.or(rec.photo_filter);
        let layer = Layer {
            name: rec.name,
            x: rec.left,
            y: rec.top,
            w,
            h,
            rgba: rgba.unwrap_or_default(),
            opacity,
            blend: rec.blend,
            visible,
            kind: rec.kind,
            groups,
            clipped: rec.clipping,
            has_effects: rec.has_effects,
            baked: Vec::new(),
        };
        out.push((layer, settings));
    }
    out.reverse();
    out
}

/// Clipping masks: clipped pixel layers keep only what lies over their base; clipped Curves and
/// Photo Filter layers are applied to their base; other adjustments are dropped with a warning.
fn bake_clipping(flat: Vec<Flat>, warnings: &mut Vec<String>) -> Vec<Layer> {
    let (mut layers, settings): (Vec<Layer>, Vec<Option<Vec<u8>>>) = flat.into_iter().unzip();
    let mut base: Option<usize> = None;
    let mut drop = Vec::new();
    for i in 0..layers.len() {
        let adjustment = match &layers[i].kind {
            LayerKind::Adjustment(k) => Some(k.clone()),
            _ => None,
        };
        if !layers[i].clipped {
            if let Some(kind) = adjustment {
                if layers[i].visible {
                    warnings.push(format!("“{}”: {kind} adjustment layers that affect every layer below are not imported", layers[i].name));
                }
                drop.push(i);
            } else {
                base = Some(i);
            }
            continue;
        }
        let Some(bi) = base else {
            if adjustment.is_some() {
                drop.push(i);
            }
            continue;
        };
        match adjustment {
            Some(kind) => {
                drop.push(i);
                if !layers[i].visible {
                    continue;
                }
                let amount = layers[i].opacity;
                let data = settings[i].clone().unwrap_or_default();
                let done = match kind.as_str() {
                    "Curves" => curves(&data).map(|lut| {
                        apply(&mut layers[bi].rgba, amount, |p| {
                            [lut[0][lut[1][p[0] as usize] as usize], lut[0][lut[2][p[1] as usize] as usize], lut[0][lut[3][p[2] as usize] as usize]]
                        })
                    }),
                    "Photo Filter" => photo_filter(&data).map(|(c, d, keep)| apply(&mut layers[bi].rgba, amount, |p| filter(p, c, d, keep))),
                    _ => None,
                };
                if done.is_some() {
                    layers[bi].baked.push(kind);
                } else {
                    warnings.push(format!("“{}”: {kind} adjustment is not imported", layers[i].name));
                }
            }
            None => {
                // a clipped pixel layer shows only over its base
                let (b, l) = if bi < i {
                    let (lo, hi) = layers.split_at_mut(i);
                    (&lo[bi], &mut hi[0])
                } else {
                    continue;
                };
                for y in 0..l.h as i32 {
                    for x in 0..l.w as i32 {
                        let (bx, by) = (l.x + x - b.x, l.y + y - b.y);
                        let ba = if bx >= 0 && by >= 0 && bx < b.w as i32 && by < b.h as i32 {
                            b.rgba[(by as usize * b.w as usize + bx as usize) * 4 + 3]
                        } else {
                            0
                        };
                        let j = (y as usize * l.w as usize + x as usize) * 4 + 3;
                        l.rgba[j] = ((l.rgba[j] as u32 * ba as u32 + 127) / 255) as u8;
                    }
                }
                if l.blend != 0 {
                    warnings.push(format!("“{}”: clipped with a blend mode; imported clipped, the blend mode applies to everything below", l.name));
                }
            }
        }
    }
    for i in drop.into_iter().rev() {
        layers.remove(i);
    }
    layers
}

/// Mix `f(pixel)` into each pixel's colour by `amount`.
fn apply(rgba: &mut [u8], amount: f32, f: impl Fn([u8; 3]) -> [u8; 3]) {
    let k = amount.clamp(0.0, 1.0);
    for p in rgba.as_chunks_mut::<4>().0 {
        let o = f([p[0], p[1], p[2]]);
        for c in 0..3 {
            p[c] = (p[c] as f32 + (o[c] as f32 - p[c] as f32) * k).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Curves (`curv`): look-up tables for the composite curve and red, green, blue (identity when a
/// curve is absent). Points are (output, input) pairs, interpolated with a monotone cubic.
fn curves(d: &[u8]) -> Option<[[u8; 256]; 4]> {
    let mut r = Reader::new(d);
    let is_map = r.u8("curves").ok()? != 0;
    let version = r.u16("curves").ok()?;
    if !matches!(version, 1 | 4) {
        return None;
    }
    let present = r.u32("curves").ok()?;
    let identity: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut luts = [identity; 4];
    for bit in 0..16 {
        if present & (1 << bit) == 0 {
            continue;
        }
        let lut = if is_map {
            let m = r.take(256, "curves").ok()?;
            std::array::from_fn(|i| m[i])
        } else {
            let n = r.u16("curves").ok()? as usize;
            let mut pts = Vec::with_capacity(n);
            for _ in 0..n {
                let out = r.u16("curves").ok()? as f32;
                let inp = r.u16("curves").ok()? as f32;
                pts.push((inp, out));
            }
            spline_lut(&mut pts)
        };
        if bit < 4 {
            luts[bit] = lut;
        }
    }
    Some(luts)
}

/// A 0..255 curve through `pts` (input, output): monotone cubic (Fritsch–Carlson).
fn spline_lut(pts: &mut Vec<(f32, f32)>) -> [u8; 256] {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|a, b| a.0 == b.0);
    if pts.len() < 2 {
        return std::array::from_fn(|i| i as u8);
    }
    let n = pts.len();
    let d: Vec<f32> = pts.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0).max(1e-6)).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[i] = t * a * d[i];
                m[i + 1] = t * b * d[i];
            }
        }
    }
    std::array::from_fn(|x| {
        let x = x as f32;
        let y = if x <= pts[0].0 {
            pts[0].1
        } else if x >= pts[n - 1].0 {
            pts[n - 1].1
        } else {
            let k = pts.windows(2).position(|w| x < w[1].0).unwrap_or(n - 2);
            let (x0, y0, x1, y1) = (pts[k].0, pts[k].1, pts[k + 1].0, pts[k + 1].1);
            let hh = x1 - x0;
            let t = (x - x0) / hh;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * hh * m[k] + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * hh * m[k + 1]
        };
        y.round().clamp(0.0, 255.0) as u8
    })
}

/// Photo Filter (`phfl`): filter colour (RGB 0..1), density (0..1), preserve luminosity.
fn photo_filter(d: &[u8]) -> Option<([f32; 3], f32, bool)> {
    let mut r = Reader::new(d);
    let version = r.u16("photo filter").ok()?;
    let color = match version {
        2 => {
            let space = r.u16("photo filter").ok()?;
            let c = [r.u16("photo filter").ok()?, r.u16("photo filter").ok()?, r.u16("photo filter").ok()?];
            r.u16("photo filter").ok()?;
            if space != 0 {
                return None;
            }
            c.map(|v| v as f32 / 65535.0)
        }
        3 => {
            // CIE XYZ (D50) as 32-bit fixed point → linear sRGB → sRGB
            let x = r.u32("photo filter").ok()? as f32 / 65536.0;
            let y = r.u32("photo filter").ok()? as f32 / 65536.0;
            let z = r.u32("photo filter").ok()? as f32 / 65536.0;
            let lin = [3.1339 * x - 1.6169 * y - 0.4906 * z, -0.9788 * x + 1.9161 * y + 0.0335 * z, 0.0719 * x - 0.2290 * y + 1.4052 * z];
            lin.map(|v| if v <= 0.003_130_8 { v.max(0.0) * 12.92 } else { 1.055 * v.min(1.0).powf(1.0 / 2.4) - 0.055 })
        }
        _ => return None,
    };
    let density = r.u32("photo filter").ok()? as f32 / 100.0;
    let keep = r.u8("photo filter").ok()? != 0;
    Some((color, density.clamp(0.0, 1.0), keep))
}

/// One pixel through a photo filter: multiplied by the filter colour by `density`, then (when
/// preserving luminosity) brought back to its original luma.
fn filter(p: [u8; 3], c: [f32; 3], density: f32, keep: bool) -> [u8; 3] {
    let v = p.map(|x| x as f32 / 255.0);
    let mut o = [0.0f32; 3];
    for i in 0..3 {
        o[i] = v[i] * (1.0 - density) + v[i] * c[i] * density;
    }
    if keep {
        let luma = |q: &[f32; 3]| 0.299 * q[0] + 0.587 * q[1] + 0.114 * q[2];
        let (a, b) = (luma(&v), luma(&o));
        if b > 1e-4 {
            let k = a / b;
            o = o.map(|x| x * k);
        }
    }
    o.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The image data section: the merged picture.
fn image_data(r: &mut Reader, w: u32, h: u32, channels: u16, depth: u16, gray: bool) -> Result<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let comp = r.u16("image data")?;
    let bps = (depth / 8) as usize;
    let n = channels as usize;
    let row = w * bps;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(n);
    match comp {
        0 => {
            for _ in 0..n {
                planes.push(r.take(row * h, "image data")?.to_vec());
            }
        }
        1 => {
            let mut counts = Vec::with_capacity(n * h);
            for _ in 0..n * h {
                counts.push(r.u16("image data")? as usize);
            }
            for c in 0..n {
                let mut plane = Vec::with_capacity(row * h);
                for y in 0..h {
                    let src = r.take(counts[c * h + y], "image data")?;
                    packbits(src, row, &mut plane).map_err(PsdError::Corrupt)?;
                }
                planes.push(plane);
            }
        }
        c => return Err(PsdError::Unsupported(format!("merged image compression {c}"))),
    }
    let planes: Vec<Vec<u8>> = planes.into_iter().map(|p| if bps == 2 { p.as_chunks::<2>().0.iter().map(|s| s[0]).collect() } else { p }).collect();
    let get = |c: usize, i: usize| planes.get(c).and_then(|p| p.get(i)).copied().unwrap_or(0);
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let (r0, g0, b0) = if gray { (get(0, i), get(0, i), get(0, i)) } else { (get(0, i), get(1, i), get(2, i)) };
        out[i * 4..i * 4 + 4].copy_from_slice(&[r0, g0, b0, 255]);
    }
    Ok(out)
}

#[cfg(any(test, feature = "testing"))]
pub mod testing;

#[cfg(test)]
mod tests;
