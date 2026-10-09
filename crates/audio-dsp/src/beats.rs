//! Beat detection for beat markers (Markers ▸ Add Beat Markers…): where the beats of a piece of
//! music fall, its tempo, and which beat is the first of a bar.
//!
//! ```text
//! mono ─► beat tracker of [`crate::remix`] (log spectral flux → tempo by autocorrelation →
//!         dynamic-programming beats, weak / off-grid edge beats dropped) ─► beat attacks
//!      ─► per beat: harmonic change (1 − cos of the 12-bin chroma of the beat and the one
//!         before) and low-end attack (log flux below 200 Hz, kick and bass)
//!      ─► downbeat: of the `beats_per_bar` bar phases, the one whose beats carry the most
//!         z(harmonic change) + ½·z(low-end attack) — chords change and kicks land on the "one"
//! ```
//!
//! Tempo is ambiguous by octaves (a ballad at 72 BPM with hi-hats on eighths reads as 144):
//! [`Tempo::Half`] keeps every other beat (the stronger ones), [`Tempo::Double`] adds the beat
//! halfway between two.
//!
//! The downbeat is an estimate (music that changes chords mid-bar or accents the backbeat can
//! fool it by a beat or two); beat positions are within ~10 ms of the attacks. Deterministic.

use crate::remix::{RemixError, Tracked, cos, local_max, onset_frame, stft_each, track};

/// How to read the tempo the tracker found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tempo {
    #[default]
    Auto,
    /// Half the tempo found: every other beat, the stronger of the two phases.
    Half,
    /// Twice the tempo found: a beat halfway between each two.
    Double,
}

/// The beats of a piece of music.
#[derive(Clone, Debug, PartialEq)]
pub struct Beats {
    pub sample_rate: u32,
    /// Attack of each beat (samples), increasing.
    pub beats: Vec<i64>,
    /// Beat period (samples).
    pub period: f64,
    /// Tempo (beats per minute).
    pub bpm: f64,
    pub beats_per_bar: usize,
    /// Index in [`Beats::beats`] of the first downbeat (the first beat of a bar), below
    /// `beats_per_bar`; the beats before it are a pickup.
    pub downbeat: usize,
}

impl Beats {
    /// Bar (from 1; 0 for a pickup before the first downbeat) and beat in the bar (from 1) of beat
    /// `i`.
    pub fn bar_beat(&self, i: usize) -> (i64, usize) {
        let bpb = self.beats_per_bar.max(1) as i64;
        let k = i as i64 - self.downbeat as i64;
        (k.div_euclid(bpb) + 1, k.rem_euclid(bpb) as usize + 1)
    }
}

/// Find the beats of planar audio (channels are summed to mono) with `beats_per_bar` beats to a
/// bar (4 for 4/4, 3 for 3/4; 1 numbers the beats without bars).
pub fn detect(channels: &[&[f32]], sample_rate: u32, beats_per_bar: usize, tempo: Tempo) -> Result<Beats, RemixError> {
    let mono = crate::remix::mono(channels);
    let tr = track(&mono, sample_rate)?;
    let len = mono.len() as i64;
    let mut frames = tr.frames.clone();
    let mut period = tr.period;
    match tempo {
        Tempo::Auto => {}
        Tempo::Half => {
            let strength = |p: usize| frames.iter().skip(p).step_by(2).map(|&t| local_max(&tr.env, t, 2)).sum::<f32>();
            let p = usize::from(strength(1) > strength(0));
            frames = frames.iter().copied().skip(p).step_by(2).collect();
            period *= 2.0;
        }
        Tempo::Double => {
            let mut f = Vec::with_capacity(frames.len() * 2);
            for w in frames.windows(2) {
                f.push(w[0]);
                f.push((w[0] + w[1]) / 2);
            }
            f.extend(frames.last());
            frames = f;
            period /= 2.0;
        }
    }
    let (frames, beats): (Vec<usize>, Vec<i64>) = frames.iter().map(|&t| (t, tr.attack(t))).filter(|&(_, b)| b >= 0 && b < len).unzip();
    if beats.len() < 4 {
        return Err(RemixError::NoBeat);
    }
    let bpb = beats_per_bar.max(1);
    let downbeat = if bpb > 1 { downbeat_phase(&mono, sample_rate, &tr, &frames, &beats, bpb) } else { 0 };
    let period = period * tr.hop as f64;
    Ok(Beats { sample_rate, beats, period, bpm: 60.0 * sample_rate as f64 / period, beats_per_bar: bpb, downbeat })
}

fn z_scores(v: &[f32]) -> Vec<f32> {
    let n = v.len().max(1) as f32;
    let mean = v.iter().sum::<f32>() / n;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n).sqrt();
    if sd > 1e-9 { v.iter().map(|x| (x - mean) / sd).collect() } else { vec![0.0; v.len()] }
}

/// The bar phase (0 … bpb − 1) whose beats carry the most harmonic change and low-end attack.
fn downbeat_phase(mono: &[f32], sr: u32, tr: &Tracked, frames: &[usize], beats: &[i64], bpb: usize) -> usize {
    let chroma = beat_chroma(mono, sr, beats);
    let mut change = vec![0f32; beats.len()];
    for i in 1..beats.len() {
        change[i] = 1.0 - cos(&chroma[i - 1], &chroma[i]);
    }
    let low_env = low_flux(mono, sr, tr.n, tr.hop);
    let low: Vec<f32> = frames.iter().map(|&t| if low_env.is_empty() { 0.0 } else { local_max(&low_env, t.min(low_env.len() - 1), 2) }).collect();
    let (zc, zl) = (z_scores(&change), z_scores(&low));
    let mut best = (f32::MIN, 0usize);
    for p in 0..bpb {
        let (mut s, mut n) = (0f32, 0f32);
        // the first beat has no harmonic change to measure
        for i in (p..beats.len()).step_by(bpb).filter(|&i| i > 0) {
            s += zc[i] + 0.5 * zl[i];
            n += 1.0;
        }
        let m = if n > 0.0 { s / n } else { f32::MIN };
        if m > best.0 {
            best = (m, p);
        }
    }
    best.1
}

/// Log-magnitude spectral flux below 200 Hz per onset hop (same frames as the onset envelope).
fn low_flux(x: &[f32], sr: u32, n: usize, hop: usize) -> Vec<f32> {
    let kmax = ((200.0 * n as f64 / sr as f64).floor() as usize).max(2);
    let mut env = Vec::new();
    let mut prev: Vec<f32> = Vec::new();
    stft_each(x, n, hop, |_, m| {
        let lg: Vec<f32> = m[1..=kmax.min(m.len() - 1)].iter().map(|v| (1.0 + 1000.0 * v).ln()).collect();
        env.push(if prev.is_empty() { 0.0 } else { lg.iter().zip(&prev).map(|(a, b)| (a - b).max(0.0)).sum() });
        prev = lg;
    });
    env
}

/// Normalised 12-bin chroma of each beat (from its attack to the next), from an STFT twice the
/// onset frame (finer pitch resolution), streamed.
fn beat_chroma(x: &[f32], sr: u32, beats: &[i64]) -> Vec<[f32; 12]> {
    let n = (onset_frame(sr) * 2).max(1024);
    let hop = n / 2;
    let bins = n / 2 + 1;
    let pc: Vec<Option<usize>> = (0..bins)
        .map(|k| {
            let f = k as f64 * sr as f64 / n as f64;
            (55.0..=5000.0).contains(&f).then(|| (((12.0 * (f / 440.0).log2()).round() as i64 + 9).rem_euclid(12)) as usize)
        })
        .collect();
    let mut out = vec![[0f32; 12]; beats.len()];
    let end = x.len() as i64;
    let mut i = 0usize;
    stft_each(x, n, hop, |t, m| {
        let centre = (t * hop + n / 2) as i64;
        while i + 1 < beats.len() && centre >= beats[i + 1] {
            i += 1;
        }
        if beats.is_empty() || centre < beats[0] || centre >= end {
            return;
        }
        for (k, v) in m.iter().enumerate() {
            if let Some(p) = pc[k] {
                out[i][p] += v * v;
            }
        }
    });
    for c in &mut out {
        let norm = c.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            c.iter_mut().for_each(|v| *v /= norm);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remix::test_music;

    fn detect_music(sr: u32, bpm: f64, bars: usize, skip_beats: usize) -> (Beats, Vec<i64>) {
        detect_tempo(sr, bpm, bars, skip_beats, Tempo::Auto)
    }

    fn detect_tempo(sr: u32, bpm: f64, bars: usize, skip_beats: usize, tempo: Tempo) -> (Beats, Vec<i64>) {
        let (m, truth) = test_music(sr, bpm, bars, 3);
        let cut = truth[skip_beats] as usize;
        let refs: Vec<&[f32]> = m.iter().map(|c| &c[cut..]).collect();
        let b = detect(&refs, sr, 4, tempo).expect("beats");
        (b, truth.iter().map(|t| t - cut as i64).filter(|&t| t >= 0).collect())
    }

    #[test]
    fn beats_sit_on_the_attacks() {
        for (bpm, sr) in [(95.0, 22_050), (120.0, 48_000), (128.0, 44_100), (100.0, 24_000)] {
            let (b, truth) = detect_music(sr, bpm, 16, 0);
            assert!((b.bpm - bpm).abs() < 1.0, "{} BPM vs {bpm}", b.bpm);
            let tol = (0.012 * sr as f64) as i64;
            let worst = b.beats.iter().map(|x| truth.iter().map(|t| (t - x).abs()).min().unwrap_or(i64::MAX)).max().unwrap_or(0);
            assert!(worst <= tol, "{bpm} BPM at {sr} Hz: worst error {:.1} ms", worst as f64 * 1000.0 / sr as f64);
            assert!(b.beats.len() + 2 >= truth.len(), "{} beats of {}", b.beats.len(), truth.len());
        }
    }

    #[test]
    fn finds_the_one_of_the_bar() {
        // cut the music 0–3 beats into a bar: the first detected beat is then beat 1, 4, 3 or 2
        for skip in 0..4 {
            for (bpm, sr) in [(100.0, 22_050), (124.0, 48_000)] {
                let (b, truth) = detect_music(sr, bpm, 16, skip);
                let tol = (0.02 * sr as f64) as i64;
                // true downbeats: the bar starts that remain after the cut
                let bar_starts: Vec<i64> = truth.iter().enumerate().filter(|(k, _)| (k + skip) % 4 == 0).map(|(_, t)| *t).collect();
                for (i, x) in b.beats.iter().enumerate() {
                    let on_bar = bar_starts.iter().any(|t| (t - x).abs() <= tol);
                    assert_eq!(b.bar_beat(i).1 == 1, on_bar, "skip {skip}, {bpm} BPM: beat {i} at {x} numbered {:?}", b.bar_beat(i));
                }
            }
        }
    }

    #[test]
    fn half_and_double_fix_the_tempo_octave() {
        // a slow song with eighth-note hats reads at twice its tempo
        let (fast, truth) = detect_tempo(24_000, 72.0, 12, 0, Tempo::Auto);
        assert!((fast.bpm - 144.0).abs() < 2.0 || (fast.bpm - 72.0).abs() < 1.0, "{} BPM", fast.bpm);
        let (half, _) = detect_tempo(24_000, 72.0, 12, 0, if fast.bpm > 100.0 { Tempo::Half } else { Tempo::Auto });
        assert!((half.bpm - 72.0).abs() < 1.0, "half: {} BPM", half.bpm);
        // Half keeps the beats on the drums, not the off-beat hats
        let tol = 300;
        for x in &half.beats {
            assert!(truth.iter().any(|t| (t - x).abs() <= tol), "beat {x} is not on a quarter note");
        }
        let (double, _) = detect_tempo(22_050, 100.0, 12, 0, Tempo::Double);
        assert!((double.bpm - 200.0).abs() < 2.0, "double: {} BPM", double.bpm);
    }

    #[test]
    fn numbers_bars_and_pickups() {
        let b = Beats { sample_rate: 48_000, beats: (0..10).map(|i| i * 24_000).collect(), period: 24_000.0, bpm: 120.0, beats_per_bar: 4, downbeat: 2 };
        assert_eq!(b.bar_beat(0), (0, 3));
        assert_eq!(b.bar_beat(1), (0, 4));
        assert_eq!(b.bar_beat(2), (1, 1));
        assert_eq!(b.bar_beat(5), (1, 4));
        assert_eq!(b.bar_beat(6), (2, 1));
        let free = Beats { beats_per_bar: 1, downbeat: 0, ..b };
        assert_eq!(free.bar_beat(7), (8, 1));
    }

    #[test]
    fn silence_and_noise_have_no_beat() {
        let sr = 22_050;
        let silence = vec![0f32; sr as usize * 8];
        assert_eq!(detect(&[&silence], sr, 4, Tempo::Auto), Err(RemixError::NoBeat));
        let mut rng = 1u32;
        let noise: Vec<f32> = (0..sr as usize * 8)
            .map(|_| {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (rng >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        assert!(detect(&[&noise], sr, 4, Tempo::Auto).is_err());
    }
}
