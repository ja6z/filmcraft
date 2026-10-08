//! A minimal Photoshop document writer (feature `testing`, and this crate's tests): RGB 8-bit
//! documents with layers (raw or RLE pixels), groups, masks, clipping, Unicode names, fill
//! opacity and Curves / Photo Filter adjustments, so other crates can test PSD import.

#![allow(clippy::unwrap_used)]

/// A layer for the test writer.
#[derive(Clone)]
pub struct L {
    pub name: &'static str,
    pub rect: (i32, i32, u32, u32),
    /// RGBA, rect w × h × 4 (empty for adjustments and groups).
    pub px: Vec<u8>,
    pub blend: &'static str,
    pub opacity: u8,
    pub fill: u8,
    pub clip: bool,
    pub hidden: bool,
    /// (rect, pixels, default) of a user mask.
    pub mask: Option<((i32, i32, u32, u32), Vec<u8>, u8)>,
    /// Additional info blocks (key, data).
    pub extra: Vec<(&'static [u8; 4], Vec<u8>)>,
    pub rle: bool,
}

pub fn layer(name: &'static str, rect: (i32, i32, u32, u32), rgba: [u8; 4]) -> L {
    let n = (rect.2 * rect.3) as usize;
    L { name, rect, px: rgba.repeat(n), blend: "norm", opacity: 255, fill: 255, clip: false, hidden: false, mask: None, extra: Vec::new(), rle: false }
}

pub fn adjustment(name: &'static str, key: &'static [u8; 4], data: Vec<u8>, clip: bool) -> L {
    L { extra: vec![(key, data)], clip, ..layer(name, (0, 0, 0, 0), [0; 4]) }
}

pub fn group_start(name: &'static str, hidden: bool, opacity: u8) -> L {
    L { hidden, opacity, extra: vec![(b"lsct", 1u32.to_be_bytes().to_vec())], ..layer(name, (0, 0, 0, 0), [0; 4]) }
}

pub fn group_end() -> L {
    L { extra: vec![(b"lsct", 3u32.to_be_bytes().to_vec())], ..layer("</Layer group>", (0, 0, 0, 0), [0; 4]) }
}

fn packbits(row: &[u8]) -> Vec<u8> {
    // one repeat run when the whole row is one value, else literal runs (both paths covered)
    if row.len() > 2 && row.iter().all(|&b| b == row[0]) && row.len() <= 128 {
        return vec![(1i16 - row.len() as i16) as i8 as u8, row[0]];
    }
    let mut out = Vec::new();
    for c in row.chunks(128) {
        out.push((c.len() - 1) as u8);
        out.extend_from_slice(c);
    }
    out
}

fn channel_data(plane: &[u8], w: usize, h: usize, rle: bool) -> Vec<u8> {
    if !rle {
        let mut v = vec![0, 0];
        v.extend_from_slice(plane);
        return v;
    }
    let rows: Vec<Vec<u8>> = plane.chunks(w.max(1)).take(h).map(packbits).collect();
    let mut v = vec![0, 1];
    for r in &rows {
        v.extend_from_slice(&(r.len() as u16).to_be_bytes());
    }
    for r in rows {
        v.extend_from_slice(&r);
    }
    v
}

fn section(v: &mut Vec<u8>, data: &[u8]) {
    v.extend_from_slice(&(data.len() as u32).to_be_bytes());
    v.extend_from_slice(data);
}

/// A version-1 RGB document, layers bottom first, with a raw merged image of `composite`.
pub fn psd(w: u32, h: u32, layers: &[L], composite: [u8; 3]) -> Vec<u8> {
    let mut v = b"8BPS".to_vec();
    v.extend_from_slice(&1u16.to_be_bytes());
    v.extend_from_slice(&[0; 6]);
    v.extend_from_slice(&3u16.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&8u16.to_be_bytes());
    v.extend_from_slice(&3u16.to_be_bytes());
    section(&mut v, &[]);
    section(&mut v, &[]);
    let mut records = Vec::new();
    let mut pixels = Vec::new();
    for l in layers {
        let (x, y, lw, lh) = l.rect;
        let (lw, lh) = (lw as usize, lh as usize);
        let plane = |c: usize| l.px.chunks(4).map(|p| p[c]).collect::<Vec<u8>>();
        let mut chans: Vec<(i16, Vec<u8>)> = if lw * lh > 0 {
            vec![
                (-1, channel_data(&plane(3), lw, lh, l.rle)),
                (0, channel_data(&plane(0), lw, lh, l.rle)),
                (1, channel_data(&plane(1), lw, lh, l.rle)),
                (2, channel_data(&plane(2), lw, lh, l.rle)),
            ]
        } else {
            vec![(-1, vec![0, 0]), (0, vec![0, 0]), (1, vec![0, 0]), (2, vec![0, 0])]
        };
        if let Some(((_, _, mw, mh), m, _)) = &l.mask {
            chans.push((-2, channel_data(m, *mw as usize, *mh as usize, false)));
        }
        for v in [y, x, y + lh as i32, x + lw as i32] {
            records.extend_from_slice(&v.to_be_bytes());
        }
        records.extend_from_slice(&(chans.len() as u16).to_be_bytes());
        for (id, d) in &chans {
            records.extend_from_slice(&id.to_be_bytes());
            records.extend_from_slice(&(d.len() as u32).to_be_bytes());
            pixels.extend_from_slice(d);
        }
        records.extend_from_slice(b"8BIM");
        records.extend_from_slice(l.blend.as_bytes());
        records.extend_from_slice(&[l.opacity, l.clip as u8, if l.hidden { 2 } else { 0 }, 0]);
        let mut extra = Vec::new();
        match &l.mask {
            Some(((mx, my, mw, mh), _, def)) => {
                let mut m = Vec::new();
                for v in [*my, *mx, my + *mh as i32, mx + *mw as i32] {
                    m.extend_from_slice(&v.to_be_bytes());
                }
                m.extend_from_slice(&[*def, 0, 0, 0]);
                section(&mut extra, &m);
            }
            None => section(&mut extra, &[]),
        }
        section(&mut extra, &[]);
        // Pascal name (ASCII placeholder), padded to 4; the real name goes in `luni`
        let ascii = b"layer";
        extra.push(ascii.len() as u8);
        extra.extend_from_slice(ascii);
        while extra.len() % 4 != 0 {
            extra.push(0);
        }
        let mut blocks: Vec<(&[u8; 4], Vec<u8>)> = l.extra.clone();
        let units: Vec<u16> = l.name.encode_utf16().collect();
        let mut luni = (units.len() as u32).to_be_bytes().to_vec();
        for u in units {
            luni.extend_from_slice(&u.to_be_bytes());
        }
        blocks.push((b"luni", luni));
        blocks.push((b"iOpa", vec![l.fill, 0, 0, 0]));
        for (k, d) in blocks {
            extra.extend_from_slice(b"8BIM");
            extra.extend_from_slice(k);
            let mut d = d;
            if d.len() % 2 == 1 {
                d.push(0);
            }
            section(&mut extra, &d);
        }
        section(&mut records, &extra);
    }
    let mut info = (layers.len() as i16).to_be_bytes().to_vec();
    info.extend_from_slice(&records);
    info.extend_from_slice(&pixels);
    let mut lm = Vec::new();
    section(&mut lm, &info);
    section(&mut lm, &[]);
    section(&mut v, &lm);
    // merged image, raw
    v.extend_from_slice(&0u16.to_be_bytes());
    for c in composite {
        v.extend(std::iter::repeat_n(c, (w * h) as usize));
    }
    v
}

pub fn curves_data(points: &[(u16, u16)]) -> Vec<u8> {
    // composite curve only: is_map 0, version 1, bit 0, n points of (output, input)
    let mut d = vec![0, 0, 1];
    d.extend_from_slice(&1u32.to_be_bytes());
    d.extend_from_slice(&(points.len() as u16).to_be_bytes());
    for &(input, output) in points {
        d.extend_from_slice(&output.to_be_bytes());
        d.extend_from_slice(&input.to_be_bytes());
    }
    d
}

pub fn photo_filter_data(rgb: [u8; 3], density: u32, keep: bool) -> Vec<u8> {
    let mut d = 2u16.to_be_bytes().to_vec();
    d.extend_from_slice(&0u16.to_be_bytes());
    for c in rgb {
        d.extend_from_slice(&(c as u16 * 257).to_be_bytes());
    }
    d.extend_from_slice(&0u16.to_be_bytes());
    d.extend_from_slice(&density.to_be_bytes());
    d.push(keep as u8);
    d
}
