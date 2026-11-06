//! Beat tracking for `analyze.beats`: onset strength (half-wave rectified spectral flux of a
//! log-magnitude STFT), a global tempo from the autocorrelation of that envelope, the beat phase
//! that best fits the envelope, and each grid beat snapped to the nearest strong onset.
//!
//! Pure functions on mono samples, so they are tested on synthetic click tracks.

use filmcraft_audio_dsp::fft::Fft;

/// STFT size and hop (at any sample rate; resolution is plenty for cut placement).
const WIN: usize = 1024;
const HOP: usize = 512;
/// Longest input analysed (samples are capped by the caller too): 20 minutes at 48 kHz.
pub const MAX_SAMPLES: usize = 48_000 * 60 * 20;

/// Result of [`analyze`].
#[derive(Clone, Debug, PartialEq)]
pub struct Beats {
    /// Tempo in beats per minute (0 when no periodicity was found).
    pub bpm: f64,
    /// Beat times, seconds from the start of the input.
    pub beats: Vec<f64>,
    /// Downbeats (every fourth beat, starting from the strongest phase), seconds.
    pub downbeats: Vec<f64>,
    /// Strong onsets (transients: hits, plucks, syllables), seconds, with strength 0..1.
    pub onsets: Vec<(f64, f32)>,
    /// How periodic the onset envelope is at the chosen tempo, 0..1.
    pub confidence: f32,
}

/// Options for [`analyze`].
#[derive(Clone, Copy, Debug)]
pub struct BeatOptions {
    pub min_bpm: f64,
    pub max_bpm: f64,
}

impl Default for BeatOptions {
    fn default() -> Self {
        Self { min_bpm: 60.0, max_bpm: 200.0 }
    }
}

/// Onset-strength envelope: one value per hop (`HOP` samples), normalised to 0..1.
pub fn onset_envelope(samples: &[f32]) -> Vec<f32> {
    let samples = samples.get(..samples.len().min(MAX_SAMPLES)).unwrap_or(&[]);
    if samples.len() < WIN {
        return Vec::new();
    }
    let fft = Fft::new(WIN);
    let window: Vec<f32> = (0..WIN).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos()).collect();
    let frames = (samples.len() - WIN) / HOP + 1;
    let bins = WIN / 2;
    let mut prev = vec![0f32; bins];
    let mut env = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0f32; WIN], vec![0f32; WIN]);
    for f in 0..frames {
        let at = f * HOP;
        let Some(chunk) = samples.get(at..at + WIN) else { break };
        for ((r, x), w) in re.iter_mut().zip(chunk).zip(&window) {
            *r = if x.is_finite() { x * w } else { 0.0 };
        }
        im.iter_mut().for_each(|v| *v = 0.0);
        fft.forward(&mut re, &mut im);
        let mut flux = 0f32;
        for (k, p) in prev.iter_mut().enumerate() {
            let (a, b) = (re.get(k).copied().unwrap_or(0.0), im.get(k).copied().unwrap_or(0.0));
            let mag = (1.0 + 1000.0 * (a * a + b * b).sqrt()).ln();
            let d = mag - *p;
            if d > 0.0 {
                flux += d;
            }
            *p = mag;
        }
        env.push(if f == 0 { 0.0 } else { flux });
    }
    // remove the slowly moving local mean, keep the positive part, normalise
    let w = 8usize;
    let mut out = vec![0f32; env.len()];
    for (i, o) in out.iter_mut().enumerate() {
        let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
        let slice = env.get(a..b).unwrap_or(&[]);
        let mean = slice.iter().sum::<f32>() / slice.len().max(1) as f32;
        *o = (env.get(i).copied().unwrap_or(0.0) - mean).max(0.0);
    }
    let max = out.iter().copied().fold(0f32, f32::max);
    if max > 0.0 {
        out.iter_mut().for_each(|v| *v /= max);
    }
    out
}

/// Track tempo and beats in `samples` (mono, `sample_rate` Hz).
pub fn analyze(samples: &[f32], sample_rate: u32, opts: BeatOptions) -> Beats {
    let empty = Beats { bpm: 0.0, beats: Vec::new(), downbeats: Vec::new(), onsets: Vec::new(), confidence: 0.0 };
    if sample_rate == 0 {
        return empty;
    }
    let env = onset_envelope(samples);
    if env.len() < 16 {
        return empty;
    }
    let hop_s = HOP as f64 / sample_rate as f64;
    let onsets = peaks(&env, hop_s);
    let (min_bpm, max_bpm) = {
        let (a, b) = (opts.min_bpm.clamp(20.0, 400.0), opts.max_bpm.clamp(20.0, 400.0));
        if a <= b { (a, b) } else { (b, a) }
    };
    // lag range in hops
    let lag_lo = ((60.0 / max_bpm) / hop_s).floor().max(1.0) as usize;
    let lag_hi = (((60.0 / min_bpm) / hop_s).ceil() as usize).min(env.len() / 2).max(lag_lo);
    // tempo and phase work on a smoothed envelope: onsets are a hop or two wide, so a period that
    // is not a whole number of hops would otherwise drift off them and favour a multiple
    let env = smooth(&env);
    let energy: f32 = env.iter().map(|v| v * v).sum::<f32>() / env.len().max(1) as f32;
    if energy <= 0.0 {
        return Beats { onsets, ..empty };
    }
    // unbiased: mean product over the overlap, so long lags are not penalised
    let ac = |lag: usize| -> f32 {
        let n = env.len().saturating_sub(lag);
        if n == 0 {
            return 0.0;
        }
        env.iter().zip(env.iter().skip(lag)).map(|(a, b)| a * b).sum::<f32>() / n as f32
    };
    // score each lag by its autocorrelation plus its double (helps pick the beat, not the
    // half-beat), weighted gently toward 120 BPM, the usual perceptual centre
    let mut best = (0usize, f32::MIN);
    for lag in lag_lo..=lag_hi {
        let bpm = 60.0 / (lag as f64 * hop_s);
        let prior = (-0.5 * ((bpm / 120.0).log2() / 1.0).powi(2)).exp() as f32;
        let score = (ac(lag) + 0.5 * ac(lag * 2)) * (0.5 + 0.5 * prior);
        if score > best.1 {
            best = (lag, score);
        }
    }
    let lag = best.0.max(1);
    // refine the period to a fraction of a hop with a parabola through the neighbours
    let (l0, l1, l2) = (ac(lag.saturating_sub(1).max(1)), ac(lag), ac(lag + 1));
    let denom = l0 - 2.0 * l1 + l2;
    let frac = if denom.abs() > 1e-9 { (0.5 * (l0 - l2) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    let period = (lag as f64 + frac as f64).max(1.0);
    let confidence = (l1 / energy).clamp(0.0, 1.0);
    // phase: the offset whose grid collects the most onset strength
    let steps = (period.ceil() as usize).max(1);
    let mut phase = (0f64, f32::MIN);
    for k in 0..steps {
        let mut sum = 0f32;
        let mut t = k as f64;
        while (t as usize) < env.len() {
            sum += env.get(t.round() as usize).copied().unwrap_or(0.0);
            t += period;
        }
        if sum > phase.1 {
            phase = (k as f64, sum);
        }
    }
    // grid beats, each snapped to the strongest envelope value within ±12% of a period
    let reach = ((period * 0.12).round() as usize).max(1);
    let mut beats = Vec::new();
    let mut t = phase.0;
    while (t.round() as usize) < env.len() {
        let c = t.round() as usize;
        let (a, b) = (c.saturating_sub(reach), (c + reach + 1).min(env.len()));
        let mut at = c;
        let mut v = env.get(c).copied().unwrap_or(0.0);
        for i in a..b {
            let x = env.get(i).copied().unwrap_or(0.0);
            if x > v * 1.5 {
                (at, v) = (i, x);
            }
        }
        beats.push(at as f64 * hop_s + latency_s(sample_rate));
        t += period;
    }
    // downbeats: the phase (0..4) of every-fourth-beat with the most strength
    let strength = |s: f64| env.get(((s - latency_s(sample_rate)) / hop_s).round().max(0.0) as usize).copied().unwrap_or(0.0);
    let mut db = (0usize, f32::MIN);
    for k in 0..4 {
        let sum: f32 = beats.iter().skip(k).step_by(4).map(|b| strength(*b)).sum();
        if sum > db.1 {
            db = (k, sum);
        }
    }
    let downbeats = beats.iter().skip(db.0).step_by(4).copied().collect();
    let bpm = 60.0 / (period * hop_s);
    Beats { bpm: (bpm * 100.0).round() / 100.0, beats, downbeats, onsets, confidence }
}

/// Binomial blur (radius 3 hops).
fn smooth(env: &[f32]) -> Vec<f32> {
    const K: [f32; 7] = [1.0, 6.0, 15.0, 20.0, 15.0, 6.0, 1.0];
    (0..env.len())
        .map(|i| {
            let (mut sum, mut wsum) = (0f32, 0f32);
            for (k, w) in K.iter().enumerate() {
                if let Some(v) = (i + k).checked_sub(3).and_then(|j| env.get(j)) {
                    sum += v * w;
                    wsum += w;
                }
            }
            if wsum > 0.0 { sum / wsum } else { 0.0 }
        })
        .collect()
}

/// The flux of a hop measures the window that starts there; an onset sits about half a window
/// later.
fn latency_s(sample_rate: u32) -> f64 {
    (WIN as f64 / 2.0) / sample_rate.max(1) as f64
}

/// Local maxima of the envelope above an adaptive threshold, at least 50 ms apart.
fn peaks(env: &[f32], hop_s: f64) -> Vec<(f64, f32)> {
    let min_gap = ((0.05 / hop_s).ceil() as usize).max(1);
    let mut out: Vec<(usize, f32)> = Vec::new();
    for (j, w) in env.windows(3).enumerate() {
        let (i, p, v, n) = (j + 1, w[0], w[1], w[2]);
        if v < 0.15 || v < p || v < n {
            continue;
        }
        match out.last_mut() {
            Some(last) if i - last.0 < min_gap => {
                if v > last.1 {
                    *last = (i, v);
                }
            }
            _ => out.push((i, v)),
        }
    }
    let lat = (WIN as f64 / 2.0) * hop_s / HOP as f64;
    out.into_iter().map(|(i, v)| (i as f64 * hop_s + lat, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: short decaying noise bursts every `60/bpm` s from `offset` s.
    fn clicks(bpm: f64, secs: f64, offset: f64, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f64) as usize;
        let mut v = vec![0f32; n];
        let mut seed = 1u32;
        let period = 60.0 / bpm;
        let mut t = offset;
        while t < secs {
            let at = (t * sr as f64) as usize;
            for k in 0..(sr as usize / 50) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
                if let Some(x) = v.get_mut(at + k) {
                    *x += noise * (-(k as f32) / (sr as f32 / 400.0)).exp();
                }
            }
            t += period;
        }
        v
    }

    #[test]
    fn finds_tempo_and_beat_times_of_a_click_track() {
        let sr = 48_000;
        for bpm in [90.0, 120.0, 128.0, 150.0] {
            let b = analyze(&clicks(bpm, 20.0, 0.25, sr), sr, BeatOptions::default());
            assert!((b.bpm - bpm).abs() < 1.5, "expected {bpm}, got {}", b.bpm);
            // every detected beat lies within 30 ms of a true click
            let period = 60.0 / bpm;
            for t in &b.beats {
                let k = ((t - 0.25) / period).round();
                let err = (t - (0.25 + k * period)).abs();
                assert!(err < 0.03, "{bpm} BPM: beat at {t} is {err} s off the grid");
            }
            assert!(b.beats.len() as f64 > 20.0 * bpm / 60.0 * 0.8, "too few beats: {}", b.beats.len());
            assert!(!b.onsets.is_empty());
            assert!(b.downbeats.len() * 4 >= b.beats.len().saturating_sub(4));
        }
    }

    #[test]
    fn silence_short_and_hostile_input_give_no_beats() {
        assert_eq!(analyze(&[], 48_000, BeatOptions::default()).beats.len(), 0);
        assert_eq!(analyze(&vec![0.0; 48_000 * 3], 48_000, BeatOptions::default()).beats.len(), 0);
        assert_eq!(analyze(&[1.0; 100], 48_000, BeatOptions::default()).bpm, 0.0);
        assert_eq!(analyze(&clicks(120.0, 4.0, 0.0, 48_000), 0, BeatOptions::default()).bpm, 0.0);
        let mut nan = clicks(120.0, 4.0, 0.0, 48_000);
        nan.iter_mut().step_by(7).for_each(|x| *x = f32::NAN);
        let _ = analyze(&nan, 48_000, BeatOptions { min_bpm: 500.0, max_bpm: -3.0 });
    }
}
