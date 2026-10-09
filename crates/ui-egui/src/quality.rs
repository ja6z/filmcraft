//! Preview quality (View ▸ Preview Quality, and the Program monitor's quality menu): one choice,
//! as in CapCut, instead of tuning Playback Resolution, Paused Resolution and proxies apart.
//!
//! | quality | playing | paused | media |
//! |---|---|---|---|
//! | Auto | adapts (¼ … Full, starts at ½) | Full | proxies |
//! | Smooth | ¼ | ½ | proxies |
//! | Balanced | ½ | Full | proxies |
//! | High Quality | Full | Full | original media |
//!
//! **Auto** looks at the frames playback drops ([`crate::frames::PlaybackMeter`]) once a second:
//! more than [`DROP_DOWN`] dropped steps the playback resolution down one notch (not below ¼),
//! [`GOOD_WINDOWS`] seconds in a row under [`DROP_UP`] step it back up (not above Full, not within
//! [`HOLD_S`] of the last window that dropped frames, nor within half that of the previous step
//! up, so it doesn't oscillate). Choosing a resolution by hand turns
//! the preset off.

use serde_json::json;

use crate::FilmcraftApp;
use crate::state::{PlaybackRes, PreviewQuality};

/// Measurement window of Auto (seconds of playback).
pub const WINDOW_S: f64 = 1.0;
/// Dropped share of a window above which Auto lowers the resolution.
pub const DROP_DOWN: f64 = 0.12;
/// Dropped share under which a window counts as smooth.
pub const DROP_UP: f64 = 0.02;
/// Smooth windows in a row before Auto raises the resolution.
pub const GOOD_WINDOWS: u32 = 3;
/// No step up for this long after a step down (seconds).
pub const HOLD_S: f64 = 6.0;

/// Auto's measurement state (not saved).
#[derive(Clone, Debug, Default)]
pub struct AutoQuality {
    window_start: f64,
    shown: u64,
    dropped: u64,
    good: u32,
    hold_until: f64,
}

impl PreviewQuality {
    pub const ALL: [PreviewQuality; 4] = [PreviewQuality::Auto, PreviewQuality::Smooth, PreviewQuality::Balanced, PreviewQuality::High];

    /// Menu label (translated through the menus).
    pub fn label(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "Auto (adapts to playback)",
            PreviewQuality::Smooth => "Smooth",
            PreviewQuality::Balanced => "Balanced",
            PreviewQuality::High => "High Quality",
        }
    }

    /// Short name for the monitor's menu button.
    pub fn short(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "Auto",
            PreviewQuality::Smooth => "Smooth",
            PreviewQuality::Balanced => "Balanced",
            PreviewQuality::High => "High Quality",
        }
    }

    /// Command id suffix (`view.previewQuality.<id>`).
    pub fn id(self) -> &'static str {
        match self {
            PreviewQuality::Auto => "auto",
            PreviewQuality::Smooth => "smooth",
            PreviewQuality::Balanced => "balanced",
            PreviewQuality::High => "high",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.id() == id)
    }
}

/// Set the Program monitor's preview quality.
pub fn apply(app: &mut FilmcraftApp, q: PreviewQuality) {
    let (play, paused, proxies) = match q {
        PreviewQuality::Auto => (PlaybackRes::Half, PlaybackRes::Full, true),
        PreviewQuality::Smooth => (PlaybackRes::Quarter, PlaybackRes::Half, true),
        PreviewQuality::Balanced => (PlaybackRes::Half, PlaybackRes::Full, true),
        PreviewQuality::High => (PlaybackRes::Full, PlaybackRes::Full, false),
    };
    let v = &mut app.ui.program;
    v.quality = Some(q);
    v.res = play;
    v.paused_res = paused;
    v.high_quality = false;
    app.playback.auto_quality = AutoQuality::default();
    if let Err(e) = app.session.execute("media.toggleProxies", json!({"enabled": proxies})) {
        app.ui.status = e.to_string();
    }
}

fn lower(r: PlaybackRes) -> Option<PlaybackRes> {
    match r {
        PlaybackRes::Full => Some(PlaybackRes::Half),
        PlaybackRes::Half => Some(PlaybackRes::Quarter),
        _ => None,
    }
}

fn higher(r: PlaybackRes) -> Option<PlaybackRes> {
    match r {
        PlaybackRes::Quarter | PlaybackRes::Eighth | PlaybackRes::Sixteenth => Some(PlaybackRes::Half),
        PlaybackRes::Half => Some(PlaybackRes::Full),
        PlaybackRes::Full => None,
    }
}

/// One playback refresh with Auto on (`now`: egui time in seconds).
pub fn tick(app: &mut FilmcraftApp, now: f64) {
    if app.ui.program.quality != Some(PreviewQuality::Auto) || !app.playback.playing || app.playback.preroll.is_some() {
        return;
    }
    let (shown, dropped) = (app.playback.meter.shown, app.playback.meter.dropped);
    let a = &mut app.playback.auto_quality;
    // a new play restarts the meter
    if a.window_start <= 0.0 || shown < a.shown || dropped < a.dropped {
        *a = AutoQuality { window_start: now, shown, dropped, hold_until: a.hold_until, ..Default::default() };
        return;
    }
    if now - a.window_start < WINDOW_S {
        return;
    }
    let (s, d) = (shown - a.shown, dropped - a.dropped);
    a.window_start = now;
    a.shown = shown;
    a.dropped = dropped;
    if s + d < 8 {
        return;
    }
    let rate = d as f64 / (s + d) as f64;
    let res = app.ui.program.res;
    if rate > DROP_DOWN {
        // (the hold runs from the last bad window, also at the lowest resolution)
        a.good = 0;
        a.hold_until = now + HOLD_S;
        if let Some(r) = lower(res) {
            app.ui.program.res = r;
        }
    } else if rate < DROP_UP {
        a.good += 1;
        if a.good >= GOOD_WINDOWS
            && now >= a.hold_until
            && let Some(r) = higher(res)
        {
            a.good = 0;
            a.hold_until = now + HOLD_S / 2.0;
            app.ui.program.res = r;
        }
    } else {
        a.good = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> FilmcraftApp {
        FilmcraftApp::new(filmcraft_engine::Session::default())
    }

    #[test]
    fn presets_set_resolutions_and_proxies() {
        let mut a = app();
        apply(&mut a, PreviewQuality::Smooth);
        assert_eq!((a.ui.program.res, a.ui.program.paused_res), (PlaybackRes::Quarter, PlaybackRes::Half));
        assert!(a.session.prefs.media.enable_proxies);
        apply(&mut a, PreviewQuality::High);
        assert_eq!((a.ui.program.res, a.ui.program.paused_res), (PlaybackRes::Full, PlaybackRes::Full));
        assert!(!a.session.prefs.media.enable_proxies, "High Quality plays the original media");
        apply(&mut a, PreviewQuality::Balanced);
        assert_eq!((a.ui.program.res, a.ui.program.paused_res, a.ui.program.quality), (PlaybackRes::Half, PlaybackRes::Full, Some(PreviewQuality::Balanced)));
    }

    /// Simulate playback refreshes: `per_s` frames a second, `drop` of them dropped.
    fn play(a: &mut FilmcraftApp, from: f64, secs: f64, drop: f64) -> f64 {
        let mut t = from;
        while t < from + secs {
            t += 1.0 / 30.0;
            if (t * 1000.0) as u64 % 1000 < (drop * 1000.0) as u64 {
                a.playback.meter.dropped += 1;
            } else {
                a.playback.meter.shown += 1;
            }
            tick(a, t);
        }
        t
    }

    #[test]
    fn auto_lowers_on_drops_and_raises_when_smooth() {
        let mut a = app();
        apply(&mut a, PreviewQuality::Auto);
        a.playback.playing = true;
        assert_eq!(a.ui.program.res, PlaybackRes::Half);
        // dropping a third of the frames: down to ¼ and no lower
        let t = play(&mut a, 1.0, 4.0, 0.33);
        assert_eq!(a.ui.program.res, PlaybackRes::Quarter);
        // smooth again: it waits out the hold, then steps back up one notch at a time
        let t = play(&mut a, t, 4.0, 0.0);
        assert_eq!(a.ui.program.res, PlaybackRes::Quarter, "held after the last dropping window");
        let t = play(&mut a, t, 3.0, 0.0);
        assert_eq!(a.ui.program.res, PlaybackRes::Half);
        play(&mut a, t, 4.0, 0.0);
        assert_eq!(a.ui.program.res, PlaybackRes::Full);
    }

    #[test]
    fn a_manual_resolution_or_another_preset_is_left_alone() {
        let mut a = app();
        apply(&mut a, PreviewQuality::Balanced);
        a.playback.playing = true;
        play(&mut a, 1.0, 5.0, 0.5);
        assert_eq!(a.ui.program.res, PlaybackRes::Half, "only Auto adapts");
    }
}
