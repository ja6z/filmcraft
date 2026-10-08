//! Caption layout and burn-in.
//!
//! [`overlay`] lays out the caption showing on a track at a time with the track's
//! [`CaptionStyle`] (font family and style, size relative to a 1080-line frame, colour,
//! background box, outline, alignment, anchor and margin; WebVTT `line:N%` and `align:` cue
//! settings override the style) and rasterises it into a small premultiplied linear-light RGBA
//! [`Overlay`] positioned in the frame. Renderers composite overlays over the finished picture.
//!
//! Captions with word times light the word being spoken when the style asks for it
//! ([`CaptionHighlight`]: the word in the highlight colour, or a box in that colour behind it).

use filmcraft_color::srgb_to_linear;
use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionHighlight, CaptionStyle, CaptionTrack, Sequence};
use filmcraft_time::Tick;

use filmcraft_text::{ParagraphStyle, TextStyle, layout, render};

/// A rendered caption: premultiplied linear-light RGBA f32 pixels at `(x, y)` in the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Overlay {
    /// Composite over a premultiplied RGBA f32 canvas of `cw`×`ch` pixels.
    pub fn composite_onto(&self, canvas: &mut [f32], cw: usize, ch: usize) {
        for y in 0..self.h {
            let cy = self.y + y as i32;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            for x in 0..self.w {
                let cx = self.x + x as i32;
                if cx < 0 || cx >= cw as i32 {
                    continue;
                }
                let s = &self.px[(y * self.w + x) * 4..(y * self.w + x) * 4 + 4];
                if s[3] <= 0.0 {
                    continue;
                }
                let i = (cy as usize * cw + cx as usize) * 4;
                let k = 1.0 - s[3];
                for c in 0..4 {
                    canvas[i + c] = s[c] + canvas[i + c] * k;
                }
            }
        }
    }
}

/// The track's face at `px`.
fn caption_style(style: &CaptionStyle, px: f32) -> TextStyle {
    TextStyle { family: style.font.clone(), style: style.font_style.clone(), size: px, ..Default::default() }
}

fn measure(text: &str, ts: &TextStyle) -> f32 {
    filmcraft_text::measure(text, ts)
}

/// Coverage of a rounded rectangle at pixel `(x, y)` (centre sampling with a one-pixel ramp).
fn rounded_rect_cover(x: f32, y: f32, r: [f32; 4], radius: f32) -> f32 {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let (hx, hy) = ((r[2] - r[0]) / 2.0 - radius, (r[3] - r[1]) / 2.0 - radius);
    let dx = ((x - cx).abs() - hx).max(0.0);
    let dy = ((y - cy).abs() - hy).max(0.0);
    let d = (dx * dx + dy * dy).sqrt() - radius;
    (0.5 - d).clamp(0.0, 1.0)
}

fn lin(c: [u8; 4]) -> [f32; 4] {
    let a = c[3] as f32 / 255.0;
    [srgb_to_linear(c[0] as f32 / 255.0) * a, srgb_to_linear(c[1] as f32 / 255.0) * a, srgb_to_linear(c[2] as f32 / 255.0) * a, a]
}

/// Word-wrap one line to `max_w` pixels.
fn wrap(line: &str, ts: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in line.split(' ') {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if !cur.is_empty() && measure(&cand, ts) > max_w {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    out.push(cur);
    out
}

/// Placement overrides from WebVTT cue settings.
fn vtt_overrides(settings: &str) -> (Option<f32>, Option<CaptionAlign>) {
    let mut line = None;
    let mut align = None;
    for kv in settings.split_whitespace() {
        let Some((k, v)) = kv.split_once(':') else { continue };
        match k {
            "line" => {
                let v = v.split(',').next().unwrap_or("");
                if let Some(p) = v.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
                    line = Some((p / 100.0).clamp(0.0, 1.0));
                }
            }
            "align" => {
                align = match v {
                    "start" | "left" => Some(CaptionAlign::Left),
                    "end" | "right" => Some(CaptionAlign::Right),
                    "center" | "middle" => Some(CaptionAlign::Center),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    (line, align)
}

/// Lay out and rasterise one caption for a `w`×`h` frame.
pub fn render_caption(c: &Caption, style: &CaptionStyle, w: usize, h: usize) -> Option<Overlay> {
    render_caption_at(c, style, None, w, h)
}

/// [`render_caption`] `at` a time after the caption's start, lighting the word being spoken
/// when the style has a highlight and the caption has word times.
pub fn render_caption_at(c: &Caption, style: &CaptionStyle, at: Option<Tick>, w: usize, h: usize) -> Option<Overlay> {
    if w == 0 || h == 0 {
        return None;
    }
    let scale = h as f32 / 1080.0;
    let px = (style.size * scale).max(4.0);
    let ts = caption_style(style, px);
    let max_w = w as f32 * 0.9;
    let mut lines: Vec<String> = Vec::new();
    // (speaker names are metadata; like Premiere, they are not burned in)
    for l in c.plain_lines() {
        // words one space apart, so the n-th word drawn is the n-th word timed
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if !l.is_empty() {
            lines.extend(wrap(&l, &ts, max_w));
        }
    }
    lines.retain(|l| !l.is_empty());
    if lines.is_empty() {
        return None;
    }
    let active = at.filter(|_| style.highlight != CaptionHighlight::None).and_then(|t| c.word_at(t));
    let vm = filmcraft_text::fonts::face(filmcraft_text::resolve(&style.font, &style.font_style).face).metrics(px);
    let (asc, desc) = (vm.ascent, vm.descent);
    let lh = px * style.line_spacing.max(0.8);
    let pad_x = (px * 0.3).round();
    let block_h = lh * lines.len() as f32;
    let (line_override, align_override) = vtt_overrides(&c.settings);
    let align = align_override.unwrap_or(style.align);
    let margin = style.margin.clamp(0.0, 0.45) * h as f32;
    let top = match (line_override, style.anchor) {
        (Some(f), _) => (f * h as f32).min(h as f32 - block_h),
        (None, CaptionAnchor::Top) => margin,
        (None, CaptionAnchor::Middle) => (h as f32 - block_h) / 2.0,
        (None, CaptionAnchor::Bottom) => h as f32 - margin - block_h,
    }
    .max(0.0)
    .round();
    let side = w as f32 * 0.05;
    let widths: Vec<f32> = lines.iter().map(|l| measure(l, &ts)).collect();
    let xs: Vec<f32> = widths
        .iter()
        .map(|&lw| match align {
            CaptionAlign::Left => side + pad_x,
            CaptionAlign::Right => w as f32 - side - pad_x - lw,
            CaptionAlign::Center => (w as f32 - lw) / 2.0,
        })
        .collect();
    let outline = (style.outline * scale).max(0.0);
    let grow = outline.ceil() + 1.0;
    let bx0 = xs.iter().zip(&widths).map(|(x, _)| x - pad_x - grow).fold(f32::MAX, f32::min).floor();
    let bx1 = xs.iter().zip(&widths).map(|(x, lw)| x + lw + pad_x + grow).fold(f32::MIN, f32::max).ceil();
    let by0 = (top - grow).floor();
    let by1 = (top + block_h + grow).ceil();
    let (ox, oy) = (bx0 as i32, by0 as i32);
    let ow = (bx1 - bx0).max(1.0) as usize;
    let oh = (by1 - by0).max(1.0) as usize;
    let mut cover = vec![0.0f32; ow * oh];
    let mut bg = vec![0.0f32; ow * oh];
    // the lit word: its glyphs (Color) or its box (Box)
    let mut lit = vec![0.0f32; if active.is_some() { ow * oh } else { 0 }];
    let mut first_word = 0;
    for (i, line) in lines.iter().enumerate() {
        let lt = top + lh * i as f32;
        if style.background && style.background_color[3] > 0 {
            let (x0, x1) = (xs[i] - pad_x, xs[i] + widths[i] + pad_x);
            let (y0, y1) = (lt, lt + lh);
            for y in (y0.floor() as i32).max(oy)..(y1.ceil() as i32).min(oy + oh as i32) {
                let fy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
                for x in (x0.floor() as i32).max(ox)..(x1.ceil() as i32).min(ox + ow as i32) {
                    let fx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                    let j = (y - oy) as usize * ow + (x - ox) as usize;
                    bg[j] = (bg[j] + fx * fy).min(1.0);
                }
            }
        }
        let baseline = lt + (lh - (asc + desc)) / 2.0 + asc;
        let l = layout(line, &ts, &ParagraphStyle::default());
        let mut m = filmcraft_text::Mask { w: ow, h: oh, a: std::mem::take(&mut cover) };
        render::draw(&l, &render::at(xs[i], baseline.round()), &mut m, (ox, oy));
        cover = m.a;
        let words: Vec<&str> = line.split(' ').collect();
        if let Some(k) = active.and_then(|a| a.checked_sub(first_word)).filter(|k| *k < words.len()) {
            let before = if k == 0 { String::new() } else { format!("{} ", words[..k].join(" ")) };
            let x0 = xs[i] + measure(&before, &ts);
            match style.highlight {
                CaptionHighlight::Box => {
                    let pad = (px * 0.18).round();
                    let r = [x0 - pad, lt + lh * 0.06, x0 + measure(words[k], &ts) + pad, lt + lh * 0.94];
                    let radius = (px * 0.22).min((r[3] - r[1]) / 2.0);
                    for y in (r[1].floor() as i32).max(oy)..(r[3].ceil() as i32).min(oy + oh as i32) {
                        for x in (r[0].floor() as i32).max(ox)..(r[2].ceil() as i32).min(ox + ow as i32) {
                            let j = (y - oy) as usize * ow + (x - ox) as usize;
                            lit[j] = lit[j].max(rounded_rect_cover(x as f32 + 0.5, y as f32 + 0.5, r, radius));
                        }
                    }
                }
                CaptionHighlight::Color => {
                    let wl = layout(words[k], &ts, &ParagraphStyle::default());
                    let mut m = filmcraft_text::Mask { w: ow, h: oh, a: std::mem::take(&mut lit) };
                    render::draw(&wl, &render::at(x0, baseline.round()), &mut m, (ox, oy));
                    lit = m.a;
                }
                CaptionHighlight::None => {}
            }
        }
        first_word += words.len();
    }
    let stroke = if outline > 0.0 { dilate(&cover, ow, oh, outline) } else { Vec::new() };
    let (tc, bc, oc, hc) = (lin(style.color), lin(style.background_color), lin(style.outline_color), lin(style.highlight_color));
    let is_box = style.highlight == CaptionHighlight::Box;
    let mut out = vec![0.0f32; ow * oh * 4];
    for j in 0..ow * oh {
        let mut p = [0.0f32; 4];
        let over = |p: &mut [f32; 4], c: [f32; 4], a: f32| {
            if a > 0.0 {
                for k in 0..4 {
                    p[k] = c[k] * a + p[k] * (1.0 - c[3] * a);
                }
            }
        };
        over(&mut p, bc, bg[j]);
        let l = lit.get(j).copied().unwrap_or(0.0);
        if is_box {
            over(&mut p, hc, l);
        }
        if !stroke.is_empty() {
            over(&mut p, oc, stroke[j]);
        }
        // the lit word's glyphs take the highlight colour (edges blend with the text colour)
        let text = if !is_box && l > 0.0 && cover[j] > 0.0 {
            let k = (l / cover[j]).min(1.0);
            [0, 1, 2, 3].map(|c| tc[c] + (hc[c] - tc[c]) * k)
        } else {
            tc
        };
        over(&mut p, text, cover[j]);
        out[j * 4..j * 4 + 4].copy_from_slice(&p);
    }
    Some(Overlay { x: ox, y: oy, w: ow, h: oh, px: out })
}

/// Max-filter a coverage mask with a disc of radius `r`.
fn dilate(m: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let ri = r.ceil() as i32;
    let mut offs = Vec::new();
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d <= r + 0.5 {
                offs.push((dx, dy, (r + 0.5 - d).clamp(0.0, 1.0)));
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut v = 0.0f32;
            for &(dx, dy, wgt) in &offs {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    v = v.max(m[sy as usize * w + sx as usize] * wgt);
                }
            }
            out[y as usize * w + x as usize] = v;
        }
    }
    out
}

/// The overlay for a caption track at timeline time `t` (None when nothing shows).
pub fn track_overlay(track: &CaptionTrack, t: Tick, w: usize, h: usize) -> Option<Overlay> {
    if !track.enabled {
        return None;
    }
    let c = track.caption_at(t)?;
    render_caption_at(c, &track.style, Some(t - c.start), w, h)
}

/// Overlays of every visible caption track of a sequence at `t`, bottom track first.
pub fn sequence_overlays(seq: &Sequence, t: Tick, w: usize, h: usize) -> Vec<Overlay> {
    seq.caption_tracks.iter().rev().filter_map(|tr| track_overlay(tr, t, w, h)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{CaptionFormat, CaptionWord, ClipId, TrackId};

    fn cap(text: &str) -> Caption {
        Caption {
            id: ClipId(1),
            start: Tick(0),
            duration: Tick(100),
            text: text.into(),
            speaker: None,
            cue_id: None,
            settings: String::new(),
            words: Vec::new(),
        }
    }

    fn timed(text: &str, words: &[(i64, i64)]) -> Caption {
        let mut c = cap(text);
        c.duration = Tick(1000);
        c.words = words.iter().map(|&(a, b)| CaptionWord { start: Tick(a), end: Tick(b) }).collect();
        c
    }

    /// Red pixels of an overlay: how many and their mean x.
    fn reds(o: &Overlay) -> (usize, f32) {
        let (mut n, mut sx) = (0, 0.0);
        for (i, p) in o.px.chunks(4).enumerate() {
            if p[3] > 0.5 && p[0] > 0.5 * p[3] && p[1] < 0.1 * p[3] {
                n += 1;
                sx += (i % o.w) as f32;
            }
        }
        (n, if n > 0 { sx / n as f32 } else { 0.0 })
    }

    #[test]
    fn the_track_font_is_used() {
        let inter = render_caption(&cap("Hello world"), &CaptionStyle::default(), 1920, 1080).unwrap();
        let mono = CaptionStyle { font: "JetBrains Mono".into(), font_style: "Regular".into(), ..Default::default() };
        let m = render_caption(&cap("Hello world"), &mono, 1920, 1080).unwrap();
        assert_ne!(inter.w, m.w, "another face sets the line at another width");
        let heavier = CaptionStyle { font_style: "Black".into(), ..Default::default() };
        let b = render_caption(&cap("Hello world"), &heavier, 1920, 1080).unwrap();
        let ink = |o: &Overlay| o.px.chunks(4).filter(|p| p[0] > 0.9).count();
        assert!(ink(&b) > ink(&inter), "a heavier style inks more pixels");
    }

    #[test]
    fn the_spoken_word_is_lit() {
        let st = CaptionStyle { background: false, highlight: CaptionHighlight::Color, highlight_color: [255, 0, 0, 255], ..Default::default() };
        let c = timed("uno dos", &[(0, 400), (500, 900)]);
        let o = render_caption_at(&c, &st, Some(Tick(600)), 1920, 1080).unwrap();
        let (n, x) = reds(&o);
        assert!(n > 200 && x > o.w as f32 / 2.0, "the second word is red: {n} px at x {x} of {}", o.w);
        let o = render_caption_at(&c, &st, Some(Tick(450)), 1920, 1080).unwrap();
        let (n, x) = reds(&o);
        assert!(n > 200 && x < o.w as f32 / 2.0, "the first word stays lit through the pause: {n} px at x {x}");
        // nothing lit without a time, without word times, or with the highlight off
        assert_eq!(reds(&render_caption(&c, &st, 1920, 1080).unwrap()).0, 0);
        assert_eq!(reds(&render_caption_at(&cap("uno dos"), &st, Some(Tick(600)), 1920, 1080).unwrap()).0, 0);
        let off = CaptionStyle { highlight: CaptionHighlight::None, ..st.clone() };
        assert_eq!(reds(&render_caption_at(&c, &off, Some(Tick(600)), 1920, 1080).unwrap()).0, 0);
        // Box: a red box behind the word, the text stays white
        let bx = CaptionStyle { highlight: CaptionHighlight::Box, ..st.clone() };
        let o = render_caption_at(&c, &bx, Some(Tick(600)), 1920, 1080).unwrap();
        let (n_box, x) = reds(&o);
        assert!(n_box > 2000 && x > o.w as f32 / 2.0, "box behind the second word: {n_box} px at x {x}");
        assert!(o.px.chunks(4).any(|p| p[0] > 0.9 && p[1] > 0.9 && p[2] > 0.9), "white text on it");
        // the track overlay lights by the time inside the caption
        let mut t = CaptionTrack::new(TrackId(1), "S".into(), CaptionFormat::Subtitle);
        t.style = st;
        let mut c2 = c.clone();
        c2.start = Tick(10_000);
        t.captions.push(c2);
        let o = track_overlay(&t, Tick(10_600), 1920, 1080).unwrap();
        assert!(reds(&o).1 > o.w as f32 / 2.0);
    }

    #[test]
    fn bottom_centre_with_box() {
        let st = CaptionStyle::default();
        let o = render_caption(&cap("Hello world"), &st, 1920, 1080).unwrap();
        // centred horizontally, in the lower part of the frame
        let cx = o.x as f32 + o.w as f32 / 2.0;
        assert!((cx - 960.0).abs() < 4.0, "centre {cx}");
        assert!(o.y > 800 && (o.y as usize + o.h) < 1080, "y {} h {}", o.y, o.h);
        // white text pixels exist and the box is dark translucent
        let mut white = 0;
        let mut boxy = 0;
        for p in o.px.chunks(4) {
            if p[3] > 0.99 && p[0] > 0.9 {
                white += 1;
            } else if p[3] > 0.7 && p[0] < 0.01 {
                boxy += 1;
            }
        }
        assert!(white > 500, "{white}");
        assert!(boxy > 1000, "{boxy}");
    }

    #[test]
    fn top_anchor_and_vtt_line() {
        let st = CaptionStyle { anchor: CaptionAnchor::Top, ..Default::default() };
        let o = render_caption(&cap("Top"), &st, 1280, 720).unwrap();
        assert!(o.y < 100);
        let mut c = cap("Line");
        c.settings = "line:50% align:start".into();
        let o = render_caption(&c, &CaptionStyle::default(), 1280, 720).unwrap();
        assert!((o.y - 360).abs() < 4, "{}", o.y);
        assert!(o.x < 120, "{}", o.x);
    }

    #[test]
    fn wraps_and_composites() {
        let long = "word ".repeat(60);
        let o = render_caption(&cap(&long), &CaptionStyle::default(), 640, 360).unwrap();
        assert!(o.w <= 640 && o.h > 40, "{}x{}", o.w, o.h);
        let mut canvas = vec![0.0f32; 640 * 360 * 4];
        o.composite_onto(&mut canvas, 640, 360);
        assert!(canvas.chunks(4).any(|p| p[0] > 0.9));
    }

    #[test]
    fn outline_draws_around_text() {
        let st = CaptionStyle { background: false, outline: 4.0, outline_color: [255, 0, 0, 255], ..Default::default() };
        let o = render_caption(&cap("O"), &st, 1920, 1080).unwrap();
        assert!(o.px.chunks(4).any(|p| p[0] > 0.9 && p[1] < 0.05 && p[3] > 0.9), "red outline pixels");
    }

    #[test]
    fn hidden_track_draws_nothing() {
        let mut t = CaptionTrack::new(TrackId(1), "S".into(), CaptionFormat::Subtitle);
        t.captions.push(cap("x"));
        assert!(track_overlay(&t, Tick(5), 100, 100).is_some());
        t.enabled = false;
        assert!(track_overlay(&t, Tick(5), 100, 100).is_none());
    }
}
