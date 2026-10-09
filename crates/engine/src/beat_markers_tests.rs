//! Tests of [`crate::beat_markers`]: Markers ▸ Add Beat Markers… on generated rhythmic music
//! (120 BPM, 16 bars of 4/4, 32 s; drums on every beat, a new chord on every bar).

use std::sync::Arc;

use filmcraft_audio_dsp::remix as dsp;
use filmcraft_project::{ClipId, Label, Marker};
use filmcraft_time::Tick;
use serde_json::json;

use super::*;

const SR: u32 = 48_000;

/// A session with the generated music on A1 at `at` seconds, the clip selected; the true beat
/// times of the music (seconds from its start).
fn session(at: f64) -> (Session, ClipId, Vec<f64>) {
    let (m, truth) = dsp::test_music(SR, 120.0, 16, 3);
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "beats", "audio": 2, "video": 1, "fps": 30.0})).unwrap();
    let inter: Vec<f32> = (0..m[0].len()).flat_map(|i| [m[0][i], m[1][i]]).collect();
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&inter, SR).into();
    let item = crate::commands::import_bytes(&mut s, "/music.wav", bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": at})).unwrap();
    let c = ClipId(r["clips"][0].as_u64().unwrap());
    s.execute("timeline.select", json!({"clips": [c.0]})).unwrap();
    (s, c, truth.iter().map(|t| *t as f64 / SR as f64).collect())
}

fn markers(s: &Session) -> Vec<Marker> {
    s.active_sequence().unwrap().markers.clone()
}

/// Within half a frame (markers sit on frames) plus the detector's ~12 ms.
const TOL: f64 = 0.5 / 30.0 + 0.012;

fn near(t: f64, truth: &[f64]) -> bool {
    truth.iter().any(|x| (x - t).abs() <= TOL)
}

#[test]
fn marks_every_beat_and_numbers_the_bars() {
    let (mut s, _, truth) = session(0.0);
    let r = s.execute("markers.addBeatMarkers", json!({})).unwrap();
    assert!((r["bpm"].as_f64().unwrap() - 120.0).abs() < 1.0, "{r}");
    let ms = markers(&s);
    assert_eq!(ms.len() as u64, r["markers"].as_u64().unwrap());
    assert!(ms.len() + 2 >= truth.len() && ms.len() <= truth.len(), "{} markers for {} beats", ms.len(), truth.len());
    let fd = Tick::from_seconds_f64(1.0 / 30.0);
    for (i, m) in ms.iter().enumerate() {
        let t = m.start.seconds();
        assert!(near(t, &truth), "marker {} at {t:.3} s is not on a beat", m.name);
        assert_eq!(m.start.0 % fd.0.max(1), 0, "marker {} is not on a frame", m.name);
        assert!(m.comment.starts_with(crate::beat_markers::COMMENT), "{}", m.comment);
        // bar.beat names follow the music: a new bar every 4 beats, on the bar starts (the chord changes)
        let (bar, beat) = m.name.split_once('.').map(|(a, b)| (a.parse::<i64>().unwrap(), b.parse::<usize>().unwrap())).unwrap();
        assert!((1..=4).contains(&beat), "{}", m.name);
        let on_bar_start = truth.iter().step_by(4).any(|x| (x - t).abs() <= TOL);
        assert_eq!(beat == 1, on_bar_start, "marker {} at {t:.3} s", m.name);
        assert_eq!(m.color, if beat == 1 { Label::Rose } else { Label::Yellow }, "{}", m.name);
        // bar 0 is the pickup before the first "one"; bars count up from there
        let seen_one = ms[..i].iter().any(|x| x.name.ends_with(".1"));
        assert_eq!(bar == 0, !seen_one && beat != 1, "marker {}", m.name);
        if i > 0 {
            assert!(bar >= ms[i - 1].name.split('.').next().unwrap().parse::<i64>().unwrap());
        }
    }
}

#[test]
fn every_bar_and_reruns_replace_their_own_markers() {
    let (mut s, _, truth) = session(0.0);
    s.execute("markers.add", json!({"time": Tick::from_seconds_f64(3.0).0, "name": "mine"})).unwrap();
    let r = s.execute("markers.addBeatMarkers", json!({"every": "bar"})).unwrap();
    let bars: Vec<Marker> = markers(&s).into_iter().filter(|m| m.name != "mine").collect();
    assert_eq!(bars.len() as u64, r["markers"].as_u64().unwrap());
    assert!((15..=16).contains(&bars.len()), "{} bar markers", bars.len());
    let starts: Vec<f64> = truth.iter().step_by(4).copied().collect();
    for m in &bars {
        assert!(m.name.ends_with(".1"), "{}", m.name);
        assert!(near(m.start.seconds(), &starts), "bar marker {} at {:.3} s is not on a bar start", m.name, m.start.seconds());
    }
    // every beat now: the bar markers are replaced, the user's marker stays
    let r2 = s.execute("markers.addBeatMarkers", json!({"every": 1})).unwrap();
    assert_eq!(r2["removed"].as_u64().unwrap(), bars.len() as u64);
    let ms = markers(&s);
    assert!(ms.iter().any(|m| m.name == "mine"));
    assert_eq!(ms.len() as u64, r2["markers"].as_u64().unwrap() + 1);
    // one undo step brings the bar markers back
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(markers(&s).len(), bars.len() + 1);
}

#[test]
fn follows_the_clip_where_it_sits_and_honours_in_out() {
    let (mut s, c, truth) = session(2.5);
    let r = s.execute("markers.addBeatMarkers", json!({"clip": c.0, "every": 2})).unwrap();
    let shifted: Vec<f64> = truth.iter().map(|t| t + 2.5).collect();
    let ms = markers(&s);
    assert!(!ms.is_empty(), "{r}");
    for m in &ms {
        assert!(m.start.seconds() >= 2.5 - TOL, "{} before the clip", m.name);
        assert!(near(m.start.seconds(), &shifted), "{} at {:.3} s", m.name, m.start.seconds());
        assert!(m.name.ends_with(".1") || m.name.ends_with(".3"), "every 2 beats from the one: {}", m.name);
    }
    // the Mix between In (10 s) and Out (20 s) only
    s.execute("markers.clearAll", json!({})).unwrap();
    s.execute("timeline.select", json!({"clips": []})).unwrap();
    s.execute("markers.markIn", json!({"time": Tick::from_seconds_f64(10.0).0})).unwrap();
    s.execute("markers.markOut", json!({"time": Tick::from_seconds_f64(20.0).0})).unwrap();
    let r = s.execute("markers.addBeatMarkers", json!({"track": "mix"})).unwrap();
    assert_eq!(r["source"], "Mix");
    let ms = markers(&s);
    assert!((18..=21).contains(&ms.len()), "{} markers in 10 s at 120 BPM", ms.len());
    for m in &ms {
        let t = m.start.seconds();
        assert!((10.0 - TOL..20.1).contains(&t), "marker at {t:.3} s outside In/Out");
        assert!(near(t, &shifted), "marker at {t:.3} s");
    }
}

#[test]
fn a_muted_track_is_still_analysed_and_bad_input_is_refused() {
    let (mut s, _, _) = session(0.0);
    s.execute("timeline.setTrack", json!({"track": "A1", "muted": true})).unwrap();
    s.execute("timeline.select", json!({"clips": []})).unwrap();
    // the Mix is silent with A1 muted; the track itself is not
    let e = s.execute("markers.addBeatMarkers", json!({"track": "mix"})).unwrap_err().to_string();
    assert!(e.contains("silent"), "{e}");
    let r = s.execute("markers.addBeatMarkers", json!({"track": "A1", "tempo": "half", "beatsPerBar": 2})).unwrap();
    assert!((r["bpm"].as_f64().unwrap() - 60.0).abs() < 1.0, "half tempo: {r}");
    for bad in [
        json!({"track": "V1"}),
        json!({"every": 0}),
        json!({"every": "often"}),
        json!({"tempo": "slow"}),
        json!({"beatsPerBar": 40}),
        json!({"color": "Plaid"}),
    ] {
        assert!(s.execute("markers.addBeatMarkers", bad.clone()).is_err(), "{bad} accepted");
    }
}

#[test]
fn disabled_without_audio_and_listed_in_the_markers_menu() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "empty"})).unwrap();
    let c = crate::find_command("markers.addBeatMarkers").unwrap();
    assert_eq!(c.menu, &["Markers"]);
    assert_eq!((c.enabled)(&s).unwrap_err(), "the sequence has no audio clips");
    let (s, _, _) = session(0.0);
    assert!((c.enabled)(&s).is_ok());
}
