//! Markers ▸ Add Beat Markers…: find the beats of the music in the sequence and mark them with
//! sequence markers, so cuts, clips and keyframes snap to the rhythm (detection in
//! [`filmcraft_audio_dsp::beats`]).
//!
//! `markers.addBeatMarkers {clip?, track?, every?, beatsPerBar?, tempo?, color?, barColor?, replace?}`
//!
//! * **What is analysed**, as it plays (through the mixer, with clip effects, speed and remix):
//!   the audio clip `clip` — by default the selected audio clip when no `track` is given — over
//!   its span, its track alone; otherwise the Mix (`track: "mix"`, the default) or one audio track
//!   (`track: "A1"`, alone even when muted) between the sequence In and Out (the whole sequence
//!   without marks). Up to [`MAX_SECONDS`] at a time.
//! * **Markers**: one every `every` beats (a number, `"bar"` = once per bar, `"2bars"`), counted
//!   from the estimated first beat of a bar so bar markers land on the "one"; `beatsPerBar` 4
//!   (default), 3 or 2. Named "bar.beat" ("12.1"), comment "Beat · 95.0 BPM", coloured `color`
//!   (Yellow) and the first beat of a bar `barColor` (Rose); on the nearest frame.
//! * `tempo`: `auto`, `half` (a ballad read at double speed) or `double`.
//! * Running it again replaces the beat markers (comment "Beat · …") in the analysed range
//!   (`replace`, default true); other markers are kept. One undo step.

use filmcraft_audio_dsp::beats::{self, Tempo};
use filmcraft_project::{ClipId, Label, Marker, MarkerId, MarkerKind, TrackId, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Longest stretch analysed at once (seconds).
pub const MAX_SECONDS: f64 = 20.0 * 60.0;

/// Comment prefix of the markers this command adds (how a re-run finds them).
pub const COMMENT: &str = "Beat · ";

fn enabled(s: &Session) -> std::result::Result<(), String> {
    crate::commands::has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.audio_tracks.iter().any(|t| !t.items.is_empty()) { Ok(()) } else { Err("the sequence has no audio clips".into()) }
}

/// What to analyse: the audio track kept (None = the Mix), the range, and a label for the report.
fn source(s: &Session, p: &Value, cmd: &str) -> Result<(Option<TrackId>, TimeRange, String)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let clip = match u64_p(p, "clip") {
        Some(c) => Some(ClipId(c)),
        None if p.get("track").is_none_or(Value::is_null) => crate::remix::selected_audio(s),
        None => None,
    };
    if let Some(c) = clip {
        let (tid, it) = q.find_item(c).ok_or_else(|| bad(cmd, "no such clip"))?;
        let tr = q.track(tid).ok_or_else(|| bad(cmd, "no such track"))?;
        if tr.kind != TrackKind::Audio {
            return Err(bad(cmd, "pick an audio clip (the music)"));
        }
        return Ok((Some(tid), TimeRange::new(it.start, it.duration), format!("clip on {}", tr.name)));
    }
    let track = match p.get("track") {
        Some(Value::String(t)) if t.eq_ignore_ascii_case("mix") => None,
        Some(_) => {
            let t = crate::commands::track_p(s, p, "track", cmd)?.ok_or_else(|| bad(cmd, "unknown `track`"))?;
            if q.track(t).is_none_or(|t| t.kind != TrackKind::Audio) {
                return Err(bad(cmd, "`track` must be an audio track (A1, A2…) or \"mix\""));
            }
            Some(t)
        }
        None => None,
    };
    let range = crate::commands::in_out_range(s)?;
    let label = track.and_then(|t| q.track(t)).map(|t| t.name.clone()).unwrap_or_else(|| "Mix".into());
    Ok((track, range, label))
}

/// Markers every this many beats.
fn every_p(p: &Value, bpb: usize, cmd: &str) -> Result<usize> {
    Ok(match p.get("every") {
        None | Some(Value::Null) => 1,
        Some(Value::String(v)) => match v.to_ascii_lowercase().as_str() {
            "beat" | "1" => 1,
            "2" => 2,
            "bar" => bpb,
            "2bars" => 2 * bpb,
            "4bars" => 4 * bpb,
            _ => return Err(bad(cmd, "`every` is a number of beats, \"bar\", \"2bars\" or \"4bars\"")),
        },
        Some(v) => match v.as_u64() {
            Some(n) if (1..=64).contains(&n) => n as usize,
            _ => return Err(bad(cmd, "`every` must be 1 … 64 beats")),
        },
    })
}

/// The audio of `range` as it plays (`track` alone, or the Mix), mono, at a rate of 32 kHz or
/// less (the sequence rate divided by a whole number): (samples, rate, decimation).
fn render_mono(s: &Session, track: Option<TrackId>, range: TimeRange) -> Result<(Vec<f32>, u32, usize)> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut q = seq.clone();
    q.video_tracks.clear();
    if let Some(t) = track {
        q.audio_tracks.retain(|x| x.id == t);
        for x in &mut q.audio_tracks {
            x.muted = false;
            x.solo = false;
        }
    }
    let sr = q.settings.sample_rate.max(1);
    let k = (sr as usize).div_ceil(32_000).max(1);
    let start = range.start.to_units_floor(sr as i64);
    let total = range.duration.to_units_floor(sr as i64).max(0);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let mut out = Vec::with_capacity(total as usize / k + 1);
    let (mut acc, mut cnt) = (0f32, 0usize);
    let mut pos = 0i64;
    while pos < total {
        let n = (total - pos).min(sr as i64) as usize;
        let b = filmcraft_render::audio::mix_sequence(&s.project, &q, start + pos, n, &provider);
        let ch = b.channels.len().max(1) as f32;
        for i in 0..n {
            acc += b.channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / ch;
            cnt += 1;
            if cnt == k {
                out.push(acc / k as f32);
                (acc, cnt) = (0.0, 0);
            }
        }
        pos += n as i64;
    }
    Ok((out, sr / k as u32, k))
}

fn label_p(p: &Value, k: &str, default: Label, cmd: &str) -> Result<Label> {
    match str_p(p, k) {
        None => Ok(default),
        Some(n) => Label::from_name(n).ok_or_else(|| bad(cmd, format!("unknown colour `{n}`"))),
    }
}

/// `markers.addBeatMarkers`.
pub fn add_beat_markers(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "markers.addBeatMarkers";
    let bpb = match p.get("beatsPerBar").and_then(Value::as_u64) {
        None => 4,
        Some(n @ 1..=12) => n as usize,
        Some(_) => return Err(bad(cmd, "`beatsPerBar` must be 1 … 12")),
    };
    let every = every_p(p, bpb, cmd)?;
    let tempo = match str_p(p, "tempo").map(str::to_ascii_lowercase).as_deref() {
        None | Some("auto") => Tempo::Auto,
        Some("half") => Tempo::Half,
        Some("double") => Tempo::Double,
        Some(_) => return Err(bad(cmd, "`tempo` is auto, half or double")),
    };
    let color = label_p(p, "color", Label::Yellow, cmd)?;
    let bar_color = label_p(p, "barColor", Label::Rose, cmd)?;
    let replace = bool_p(p, "replace").unwrap_or(true);
    let (track, range, what) = source(s, p, cmd)?;
    if range.duration.seconds() > MAX_SECONDS {
        return Err(bad(
            cmd,
            format!("that is {:.0} min of audio: set In and Out around at most {:.0} min", range.duration.seconds() / 60.0, MAX_SECONDS / 60.0),
        ));
    }
    let (mono, rate, k) = render_mono(s, track, range)?;
    if mono.iter().all(|v| v.abs() < 1e-5) {
        return Err(EngineError::Other(format!("the audio of the {what} is silent there: nothing to analyse")));
    }
    let found = beats::detect(&[&mono], rate, bpb, tempo)
        .map_err(|_| EngineError::Other(format!("no steady beat found in the {what} (it needs music with a pulse)")))?;
    let sr = (rate as i64) * k as i64;
    let seq_rate = s.sequence_rate();
    let end = range.end().min(s.active_sequence().map(|q| q.duration()).unwrap_or(range.end()));
    let comment = format!("{COMMENT}{:.1} BPM", found.bpm);
    let mut marks: Vec<(Tick, String, Label)> = Vec::new();
    for (i, &b) in found.beats.iter().enumerate() {
        if (i as i64 - found.downbeat as i64).rem_euclid(every as i64) != 0 {
            continue;
        }
        let t = seq_rate.snap_nearest(range.start + Tick::from_units(b * k as i64, sr));
        if t < range.start || t >= end || marks.last().is_some_and(|m| m.0 == t) {
            continue;
        }
        let (bar, beat) = found.bar_beat(i);
        let c = if beat == 1 && bpb > 1 { bar_color } else { color };
        marks.push((t, format!("{bar}.{beat}"), c));
    }
    let added = marks.len();
    let (r0, r1) = (range.start, range.end());
    let removed = s.edit_sequence("Add Beat Markers", move |q, ctx, _| {
        let before = q.markers.len();
        if replace {
            q.markers.retain(|m| !(m.comment.starts_with(COMMENT) && m.start >= r0 && m.start < r1));
        }
        let removed = before - q.markers.len();
        for (t, name, c) in marks {
            q.markers.push(Marker {
                id: MarkerId(ctx.alloc()),
                start: t,
                duration: Tick::ZERO,
                name,
                comment: comment.clone(),
                kind: MarkerKind::Comment,
                color: c,
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(removed)
    })?;
    let first_bar = found.beats.get(found.downbeat).map(|&b| (range.start + Tick::from_units(b * k as i64, sr)).seconds());
    Ok(json!({
        "bpm": (found.bpm * 10.0).round() / 10.0,
        "beatsPerBar": bpb,
        "beats": found.beats.len(),
        "every": every,
        "markers": added,
        "removed": removed,
        "firstDownbeatSeconds": first_bar,
        "source": what,
        "start": range.start.0,
        "end": range.end().0,
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "markers.addBeatMarkers",
        label: "Add Beat Markers…",
        menu: &["Markers"],
        shortcut: None,
        params: r#"{"clip":id?,"track":"mix"|"A1"|id?,"every":n|"bar"|"2bars"|"4bars"=1,"beatsPerBar":n=4,"tempo":"auto|half|double"?,"color":label?,"barColor":label?,"replace":bool=true}"#,
        enabled,
        run: add_beat_markers,
        journal: true,
    }]
}
