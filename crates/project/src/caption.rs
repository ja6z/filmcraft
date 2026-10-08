//! Caption tracks.
//!
//! Premiere keeps captions in their own track area above the video tracks. A [`CaptionTrack`] has a
//! format (Subtitle, CEA-608, CEA-708, Teletext), a track style (font, size, colour, background box,
//! position) and a list of [`Caption`] blocks with exact in/out times in sequence ticks. Captions on
//! one track never overlap (like track items); [`CaptionTrack::check`] verifies it.
//!
//! Caption text is kept as written (lines separated by `\n`, inline `<i>`/`<b>`/`<u>` tags and
//! WebVTT cue settings preserved) so files round-trip; renderers strip markup for display.
//!
//! Captions made from a transcript also carry **word times** ([`CaptionWord`], one per word of the
//! text, relative to the caption's start so moves and ripples keep them in place), which drive the
//! track style's word-by-word highlight ([`CaptionHighlight`]).

use filmcraft_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};

use crate::{ClipId, TrackId};

/// Caption stream format of a track (Premiere's "Caption Format" choices).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptionFormat {
    /// Open/sidecar subtitles (SRT, WebVTT).
    #[default]
    Subtitle,
    /// CEA-608 (line 21) closed captions.
    Cea608,
    /// CEA-708 (DTVCC) closed captions.
    Cea708,
    /// EBU Teletext subtitles.
    Teletext,
}

impl CaptionFormat {
    pub const ALL: [CaptionFormat; 4] = [CaptionFormat::Subtitle, CaptionFormat::Cea608, CaptionFormat::Cea708, CaptionFormat::Teletext];
    pub fn label(self) -> &'static str {
        match self {
            CaptionFormat::Subtitle => "Subtitle",
            CaptionFormat::Cea608 => "CEA-608",
            CaptionFormat::Cea708 => "CEA-708",
            CaptionFormat::Teletext => "Teletext",
        }
    }
    pub fn from_name(s: &str) -> Option<CaptionFormat> {
        let n = s.to_ascii_lowercase().replace(['-', ' ', '_'], "");
        Self::ALL.iter().copied().find(|f| f.label().to_ascii_lowercase().replace(['-', ' '], "") == n)
    }
    /// Characters per line limit used when wrapping (608 is fixed at 32 columns).
    pub fn max_columns(self) -> Option<usize> {
        match self {
            CaptionFormat::Cea608 => Some(32),
            CaptionFormat::Teletext => Some(40),
            CaptionFormat::Cea708 => Some(42),
            CaptionFormat::Subtitle => None,
        }
    }
}

/// Horizontal text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptionAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// Vertical placement of the caption block in the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptionAnchor {
    Top,
    Middle,
    #[default]
    Bottom,
}

/// Word-by-word highlight of captions that carry word times (CapCut-style karaoke).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptionHighlight {
    #[default]
    None,
    /// The word being spoken takes the highlight colour.
    Color,
    /// A box in the highlight colour sits behind the word being spoken.
    Box,
}

impl CaptionHighlight {
    pub const ALL: [CaptionHighlight; 3] = [CaptionHighlight::None, CaptionHighlight::Color, CaptionHighlight::Box];
    pub fn label(self) -> &'static str {
        match self {
            CaptionHighlight::None => "None",
            CaptionHighlight::Color => "Color",
            CaptionHighlight::Box => "Box",
        }
    }
    pub fn from_name(s: &str) -> Option<CaptionHighlight> {
        Self::ALL.iter().copied().find(|h| h.label().eq_ignore_ascii_case(s.trim()))
    }
}

/// Track-level caption style (Premiere's caption track style / Essential Graphics text settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionStyle {
    /// Font family: the bundled Inter or any installed family (unknown names fall back to Inter).
    pub font: String,
    /// Style of `font` ("SemiBold", "Black", "Bold Italic"…); a weight the family lacks is
    /// synthesised from the nearest one.
    pub font_style: String,
    /// Font size in pixels for a 1080-line frame (scaled with the frame height).
    pub size: f32,
    /// Text colour, sRGB + alpha.
    pub color: [u8; 4],
    /// Draw a background box behind each line.
    pub background: bool,
    /// Background box colour, sRGB + alpha.
    pub background_color: [u8; 4],
    pub align: CaptionAlign,
    pub anchor: CaptionAnchor,
    /// Distance from the anchored frame edge, as a fraction of the frame height.
    pub margin: f32,
    /// Line height as a multiple of the font size.
    pub line_spacing: f32,
    /// Text outline width in pixels at 1080 lines (0 = none).
    pub outline: f32,
    pub outline_color: [u8; 4],
    /// Word-by-word highlight (captions with word times only).
    pub highlight: CaptionHighlight,
    pub highlight_color: [u8; 4],
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            font: "Inter".into(),
            font_style: "SemiBold".into(),
            size: 54.0,
            color: [255, 255, 255, 255],
            background: true,
            background_color: [0, 0, 0, 191],
            align: CaptionAlign::Center,
            anchor: CaptionAnchor::Bottom,
            margin: 0.08,
            line_spacing: 1.25,
            outline: 0.0,
            outline_color: [0, 0, 0, 255],
            highlight: CaptionHighlight::None,
            highlight_color: [255, 214, 10, 255],
        }
    }
}

/// When one word of a caption is spoken, relative to the caption's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CaptionWord {
    pub start: Tick,
    pub end: Tick,
}

/// One caption block on a caption track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub id: ClipId,
    /// Timeline start (sequence ticks).
    pub start: Tick,
    pub duration: Tick,
    /// Caption text; lines separated by `\n`.
    pub text: String,
    /// Speaker name (WebVTT `<v Name>` voice span).
    #[serde(default)]
    pub speaker: Option<String>,
    /// WebVTT cue identifier, kept for round trips.
    #[serde(default)]
    pub cue_id: Option<String>,
    /// WebVTT cue settings (`line:90% align:start`…), kept verbatim.
    #[serde(default)]
    pub settings: String,
    /// Word times, one per word of the plain text (captions made from a transcript); empty when
    /// unknown. Relative to `start`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<CaptionWord>,
}

impl Caption {
    pub fn end(&self) -> Tick {
        self.start + self.duration
    }
    pub fn range(&self) -> TimeRange {
        TimeRange::new(self.start, self.duration)
    }
    /// The caption's text with markup removed and entities decoded (what is drawn).
    pub fn plain_lines(&self) -> Vec<String> {
        plain_text(&self.text).lines().map(str::to_string).collect()
    }
    /// The words of the plain text (what [`Caption::words`] times, in order).
    pub fn tokens(&self) -> Vec<String> {
        plain_text(&self.text).split_whitespace().map(str::to_string).collect()
    }
    /// Whether the word times still describe the text (one per word).
    pub fn has_word_times(&self) -> bool {
        !self.words.is_empty() && self.words.len() == plain_text(&self.text).split_whitespace().count()
    }
    /// The word being spoken `rel` after the caption's start: the last word that has started (it
    /// stays lit through the pause after it). None before the first word or without word times.
    pub fn word_at(&self, rel: Tick) -> Option<usize> {
        if !self.has_word_times() {
            return None;
        }
        let n = self.words.partition_point(|w| w.start <= rel);
        n.checked_sub(1)
    }
    /// Keep the words where they are on the timeline after `start` moved from `old_start`.
    pub fn rebase_words(&mut self, old_start: Tick) {
        let d = old_start - self.start;
        for w in &mut self.words {
            w.start += d;
            w.end += d;
        }
    }
    /// Map the words' timeline times through `f` (which also saw the caption move from
    /// `old_start` to `start`), keeping them relative to the new start.
    pub fn map_words(&mut self, old_start: Tick, f: impl Fn(Tick) -> Tick) {
        for w in &mut self.words {
            w.start = f(old_start + w.start) - self.start;
            w.end = f(old_start + w.end) - self.start;
        }
    }
}

/// A caption track (shown in the caption area above the video tracks).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionTrack {
    pub id: TrackId,
    pub name: String,
    pub format: CaptionFormat,
    /// BCP-47 language tag (`en`, `fr-CA`…).
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub style: CaptionStyle,
    /// Track output (eye): shown in the Program monitor and burned in on export.
    pub enabled: bool,
    pub locked: bool,
    /// Follows inserts / extracts on other tracks.
    #[serde(default = "yes")]
    pub sync_lock: bool,
    /// Captions sorted by start, non-overlapping.
    pub captions: Vec<Caption>,
    /// WebVTT header blocks (STYLE / REGION / NOTE) kept for round trips.
    #[serde(default)]
    pub vtt_blocks: Vec<String>,
}

fn yes() -> bool {
    true
}

impl CaptionTrack {
    pub fn new(id: TrackId, name: String, format: CaptionFormat) -> Self {
        Self {
            id,
            name,
            format,
            language: "en".into(),
            style: CaptionStyle::default(),
            enabled: true,
            locked: false,
            sync_lock: true,
            captions: Vec::new(),
            vtt_blocks: Vec::new(),
        }
    }
    pub fn end(&self) -> Tick {
        self.captions.iter().map(Caption::end).max().unwrap_or(Tick::ZERO)
    }
    /// Caption covering `t`.
    pub fn caption_at(&self, t: Tick) -> Option<&Caption> {
        let idx = self.captions.partition_point(|c| c.start <= t);
        idx.checked_sub(1).map(|i| &self.captions[i]).filter(|c| t < c.end())
    }
    pub fn caption(&self, id: ClipId) -> Option<&Caption> {
        self.captions.iter().find(|c| c.id == id)
    }
    pub fn caption_mut(&mut self, id: ClipId) -> Option<&mut Caption> {
        self.captions.iter_mut().find(|c| c.id == id)
    }
    pub fn sort(&mut self) {
        self.captions.sort_by_key(|c| c.start);
    }
    /// Invariant check: captions sorted, positive duration, no overlaps.
    pub fn check(&self) -> Result<(), String> {
        for c in &self.captions {
            if c.duration.0 <= 0 {
                return Err(format!("{}: caption {:?} has non-positive duration", self.name, c.id));
            }
        }
        for w in self.captions.windows(2) {
            if w[0].end() > w[1].start {
                return Err(format!("{}: captions {:?} and {:?} overlap", self.name, w[0].id, w[1].id));
            }
        }
        Ok(())
    }
}

/// Strip `<…>` tags and decode the common HTML/WebVTT entities.
pub fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // a tag runs to the next '>' (if none, keep the '<' literally)
                let rest: String = chars.clone().collect();
                if let Some(end) = rest.find('>') {
                    for _ in 0..rest[..=end].chars().count() {
                        chars.next();
                    }
                } else {
                    out.push('<');
                }
            }
            '{' if chars.peek() == Some(&'\\') => {
                // SSA-style override blocks some SRT files carry: {\an8}
                let rest: String = chars.clone().collect();
                if let Some(end) = rest.find('}') {
                    for _ in 0..rest[..=end].chars().count() {
                        chars.next();
                    }
                } else {
                    out.push('{');
                }
            }
            '&' => {
                let rest: String = chars.clone().take(8).collect();
                let ents = [
                    ("amp;", '&'),
                    ("lt;", '<'),
                    ("gt;", '>'),
                    ("nbsp;", '\u{a0}'),
                    ("quot;", '"'),
                    ("apos;", '\''),
                    ("lrm;", '\u{200e}'),
                    ("rlm;", '\u{200f}'),
                ];
                if let Some((name, ch)) = ents.iter().find(|(n, _)| rest.starts_with(n)) {
                    for _ in 0..name.len() {
                        chars.next();
                    }
                    out.push(*ch);
                } else {
                    out.push('&');
                }
            }
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        assert_eq!(plain_text("<i>Hello</i> &amp; <b>bye</b>"), "Hello & bye");
        assert_eq!(plain_text("{\\an8}Top"), "Top");
        assert_eq!(plain_text("a < b"), "a < b");
        assert_eq!(plain_text("<v Bob>Hi</v>"), "Hi");
    }

    #[test]
    fn caption_at() {
        let mut t = CaptionTrack::new(TrackId(1), "Subtitle".into(), CaptionFormat::Subtitle);
        for (i, s) in [0i64, 100, 300].iter().enumerate() {
            t.captions.push(Caption {
                id: ClipId(i as u64),
                start: Tick(*s),
                duration: Tick(50),
                text: format!("c{i}"),
                speaker: None,
                cue_id: None,
                settings: String::new(),
                words: Vec::new(),
            });
        }
        assert_eq!(t.caption_at(Tick(120)).unwrap().text, "c1");
        assert!(t.caption_at(Tick(160)).is_none());
        assert!(t.check().is_ok());
        assert_eq!(CaptionFormat::from_name("cea608"), Some(CaptionFormat::Cea608));
        assert_eq!(CaptionFormat::from_name("CEA-708"), Some(CaptionFormat::Cea708));
    }

    #[test]
    fn word_times_follow_the_text_and_the_caption() {
        let w = |a: i64, b: i64| CaptionWord { start: Tick(a), end: Tick(b) };
        let mut c = Caption {
            id: ClipId(1),
            start: Tick(100),
            duration: Tick(100),
            text: "Ella me\n<i>traicionó</i>".into(),
            speaker: None,
            cue_id: None,
            settings: String::new(),
            words: vec![w(10, 20), w(25, 30), w(40, 80)],
        };
        assert!(c.has_word_times(), "one time per word of the plain text");
        assert_eq!((c.word_at(Tick(5)), c.word_at(Tick(10)), c.word_at(Tick(33)), c.word_at(Tick(95))), (None, Some(0), Some(1), Some(2)));
        // the caption moves its start; the words stay put on the timeline
        c.start = Tick(120);
        c.rebase_words(Tick(100));
        assert_eq!(c.words[0], w(-10, 0));
        c.map_words(Tick(120), |x| x + Tick(5));
        assert_eq!(c.words[2], w(25, 65));
        // a different number of words: the times no longer apply
        c.text = "Ella me traicionó otra vez".into();
        assert!(!c.has_word_times());
        assert_eq!(c.word_at(Tick(30)), None);
        assert_eq!(CaptionHighlight::from_name("BOX"), Some(CaptionHighlight::Box));
    }
}
