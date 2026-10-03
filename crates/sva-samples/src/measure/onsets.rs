// Concern: spectral flux onset detection, each onset placed at the sample its strike rises | Non-concern: a tempo grid, what an onset means musically | IO: (&[f64], rate, start) -> Onsets

use crate::measure::spectrum::{MAX_PINNED_FRAME, magnitudes, pinned_frame};

/// ~23ms: short enough to localize a transient, long enough for low percussion.
const FRAME_SECS: f64 = 0.023;
const HOP_SECS: f64 = FRAME_SECS / 2.0;
/// Below this two novelty peaks are one transient's attack and decay.
const MIN_ONSET_GAP_SECS: f64 = 0.05;
/// Frames either side of a candidate that set its local adaptive threshold.
const ADAPTIVE_WINDOW_FRAMES: usize = 10;
const THRESHOLD_MULTIPLIER: f64 = 1.5;
const THRESHOLD_DELTA: f64 = 1e-6;
const IOI_BUCKET_SECS: f64 = 0.025;
const IOI_MAX_SECS: f64 = 2.0;
/// Under this share of the loudest frame a rise is leakage.
const LEVEL_FRACTION: f64 = 0.03;
const STRIKE_SPAN_SECS: f64 = 0.012;
/// 6 dB over the span before: a swell or a tail rises a few dB in the same time.
const JUMP_ENERGY_RATIO: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    pub t_secs: f64,
    pub strength: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IoiBucket {
    pub lo_secs: f64,
    pub hi_secs: f64,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Onsets {
    pub onsets: Vec<Onset>,
    pub ioi_histogram: Vec<IoiBucket>,
    pub resolution_secs: f64,
}

struct SpectralFrame {
    t_secs: f64,
    mags: Vec<f64>,
    span_secs: f64,
}

fn spectral_frames(samples: &[f64], sample_rate: f64, start_secs: f64) -> Vec<SpectralFrame> {
    let frame_len = pinned_frame(FRAME_SECS, sample_rate).min(MAX_PINNED_FRAME);
    let hop = ((HOP_SECS * sample_rate).round() as usize).max(1);
    (0..samples.len())
        .step_by(hop)
        .map(|start| {
            let end = (start + frame_len).min(samples.len());
            let (mags, ..) = magnitudes(&samples[start..end], sample_rate, Some(FRAME_SECS));
            SpectralFrame {
                t_secs: start_secs + start as f64 / sample_rate,
                mags,
                span_secs: frame_len as f64 / sample_rate,
            }
        })
        .collect()
}

pub fn detect(samples: &[f64], sample_rate: f64, start_secs: f64) -> Onsets {
    let frames = spectral_frames(samples, sample_rate, start_secs);
    let floor = LEVEL_FRACTION * frames.iter().map(|f| total(&f.mags)).fold(0.0, f64::max);
    let flux = flux_of(&frames, floor);
    let onsets = pick_peaks(
        &flux,
        &frames,
        &Energy::of(samples, sample_rate, start_secs),
        floor,
    );
    Onsets {
        ioi_histogram: ioi_histogram(&onsets),
        onsets,
        resolution_secs: HOP_SECS,
    }
}

fn total(mags: &[f64]) -> f64 {
    mags.iter().sum()
}

/// The rise into each frame; nothing precedes the buffer, so one opening above the floor
/// opens on a rise from silence.
fn flux_of(frames: &[SpectralFrame], floor: f64) -> Vec<f64> {
    let opens_on_sound = frames
        .first()
        .is_some_and(|f| floor > 0.0 && total(&f.mags) > floor);
    (0..frames.len())
        .map(|i| match i {
            0 => match opens_on_sound {
                true => total(&frames[0].mags),
                false => 0.0,
            },
            _ => spectral_flux(&frames[i - 1].mags, &frames[i].mags),
        })
        .collect()
}

fn spectral_flux(prev: &[f64], curr: &[f64]) -> f64 {
    prev.iter().zip(curr).map(|(&p, &c)| (c - p).max(0.0)).sum()
}

/// A candidate clears the threshold and the floor, peaks over the gap, and holds a strike;
/// a frame past the buffer's end reads silence.
fn pick_peaks(flux: &[f64], frames: &[SpectralFrame], energy: &Energy, floor: f64) -> Vec<Onset> {
    let mut onsets: Vec<Onset> = Vec::new();
    for i in 0..flux.len() {
        let lo = i.saturating_sub(ADAPTIVE_WINDOW_FRAMES);
        let hi = (i + ADAPTIVE_WINDOW_FRAMES + 1).min(flux.len());
        let local = &flux[lo..hi];
        let mean = local.iter().sum::<f64>() / local.len() as f64;
        let threshold = THRESHOLD_DELTA + THRESHOLD_MULTIPLIER * mean;
        let reach = (MIN_ONSET_GAP_SECS / HOP_SECS).round().max(1.0) as usize;
        let near = &flux[i.saturating_sub(reach)..(i + reach + 1).min(flux.len())];
        if flux[i] <= threshold || flux[i] < floor || near.iter().any(|&v| v > flux[i]) {
            continue;
        }
        let Some(t) = energy.strike(frames[i].t_secs, frames[i].span_secs) else {
            continue;
        };
        if onsets
            .last()
            .is_some_and(|o| t - o.t_secs < MIN_ONSET_GAP_SECS)
        {
            continue;
        }
        onsets.push(Onset {
            t_secs: t,
            strength: flux[i],
        });
    }
    onsets
}

struct Energy {
    running: Vec<f64>,
    sample_rate: f64,
    start_secs: f64,
    reach: usize,
    floor: f64,
}

impl Energy {
    fn of(samples: &[f64], sample_rate: f64, start_secs: f64) -> Energy {
        let mut running = Vec::with_capacity(samples.len() + 1);
        running.push(0.0);
        for s in samples {
            let held = running[running.len() - 1];
            running.push(held + s * s);
        }
        let reach = (STRIKE_SPAN_SECS * sample_rate).round().max(1.0) as usize;
        let loudest = (0..samples.len())
            .map(|n| running[(n + reach).min(samples.len())] - running[n])
            .fold(0.0, f64::max);
        Energy {
            running,
            sample_rate,
            start_secs,
            reach,
            floor: LEVEL_FRACTION * LEVEL_FRACTION * loudest,
        }
    }

    /// The energy of samples `[from, to)`, silence outside the buffer.
    fn over(&self, from: usize, to: usize) -> f64 {
        let last = self.running.len() - 1;
        self.running[to.min(last)] - self.running[from.min(last)]
    }

    /// The sample whose energy after most outweighs the energy before it and the floor, if
    /// by a strike's jump.
    fn strike(&self, from_secs: f64, span_secs: f64) -> Option<f64> {
        let at = |secs: f64| {
            ((secs - self.start_secs) * self.sample_rate)
                .round()
                .max(0.0) as usize
        };
        let (from, to) = (at(from_secs), at(from_secs + span_secs));
        let mut best: Option<(f64, usize)> = None;
        for n in from..to.min(self.running.len() - 1) {
            let before = self.over(n.saturating_sub(self.reach), n) + self.floor;
            let after = self.over(n, n + self.reach);
            if before == 0.0 {
                continue;
            }
            let ratio = after / before;
            if best.is_none_or(|(r, _)| ratio >= r) {
                best = Some((ratio, n));
            }
        }
        let (ratio, n) = best?;
        (ratio >= JUMP_ENERGY_RATIO).then(|| self.start_secs + n as f64 / self.sample_rate)
    }
}

fn ioi_histogram(onsets: &[Onset]) -> Vec<IoiBucket> {
    let n = (IOI_MAX_SECS / IOI_BUCKET_SECS).ceil() as usize + 1;
    let mut counts = vec![0usize; n];
    for w in onsets.windows(2) {
        let ioi = w[1].t_secs - w[0].t_secs;
        counts[((ioi / IOI_BUCKET_SECS) as usize).min(n - 1)] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(i, count)| IoiBucket {
            lo_secs: i as f64 * IOI_BUCKET_SECS,
            hi_secs: if i + 1 == n {
                f64::INFINITY
            } else {
                (i + 1) as f64 * IOI_BUCKET_SECS
            },
            count,
        })
        .collect()
}
