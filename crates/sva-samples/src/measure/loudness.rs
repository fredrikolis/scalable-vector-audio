// Concern: measures BS.1770 K-weighted loudness across a node's components | Non-concern: per-component level (envelope.rs), designing a biquad (biquad.rs) | IO: (planes, sample rate) -> Loudness

use crate::biquad::{Coeffs, State};

/// K-weighting as the analog prototypes BS.1770 bilinear-transforms, not the cookbook shapes:
/// stating them as a design is what makes every rate but 48 kHz right too.
const SHELF_HZ: f64 = 1681.97;
const SHELF_Q: f64 = 0.70718;
const SHELF_GAIN_DB: f64 = 3.99984;
const SHELF_MID: f64 = 0.499_666_774_154_541_6;
const HIGHPASS_HZ: f64 = 38.1355;
const HIGHPASS_Q: f64 = 0.50033;

fn shelf(sr: f64) -> Coeffs {
    let k = (std::f64::consts::PI * SHELF_HZ / sr).tan();
    let high = 10f64.powf(SHELF_GAIN_DB / 20.0);
    let mid = high.powf(SHELF_MID);
    let a0 = 1.0 + k / SHELF_Q + k * k;
    Coeffs {
        b0: (high + mid * k / SHELF_Q + k * k) / a0,
        b1: 2.0 * (k * k - high) / a0,
        b2: (high - mid * k / SHELF_Q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / SHELF_Q + k * k) / a0,
    }
}

fn highpass(sr: f64) -> Coeffs {
    let k = (std::f64::consts::PI * HIGHPASS_HZ / sr).tan();
    let a0 = 1.0 + k / HIGHPASS_Q + k * k;
    Coeffs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / HIGHPASS_Q + k * k) / a0,
    }
}

const OFFSET_DB: f64 = -0.691;
const ABSOLUTE_GATE: f64 = -70.0;
const INTEGRATED_GATE_LU: f64 = -10.0;

const RANGE_GATE_LU: f64 = -20.0;

const MOMENTARY_SECS: f64 = 0.4;
const SHORT_TERM_SECS: f64 = 3.0;

const STEP_SECS: f64 = 0.1;

const PEAK_NOTE: &str = "sample peak, not true peak: no oversampling exists here, so an inter-sample peak above \
     this figure is not measured";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessFrame {
    /// The block's START, the global seconds every other representation reports.
    pub t: f64,
    pub lufs: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loudness {
    pub integrated_lufs: Option<f64>,
    pub range_lu: Option<f64>,
    pub momentary_max_lufs: Option<f64>,
    pub short_term_max_lufs: Option<f64>,
    pub sample_peak: f64,
    pub sample_peak_dbfs: Option<f64>,
    pub peak_note: &'static str,
    pub momentary: Vec<LoudnessFrame>,
    pub short_term: Vec<LoudnessFrame>,
}

/// Every component weighs 1.0: BS.1770's 1.41 surround lift needs a channel identity an
/// indexed component does not carry.
pub fn analyze(planes: &[&[f64]], sr: f64, start_secs: f64) -> Loudness {
    let squares: Vec<Vec<f64>> = planes.iter().map(|p| running_squares(p, sr)).collect();
    let samples = planes.first().map_or(0, |p| p.len());

    let momentary = blocks(&squares, samples, sr, start_secs, MOMENTARY_SECS);
    let short_term = blocks(&squares, samples, sr, start_secs, SHORT_TERM_SECS);
    let sample_peak = planes
        .iter()
        .flat_map(|p| p.iter())
        .fold(0.0f64, |acc, v| acc.max(v.abs()));

    Loudness {
        integrated_lufs: gated_mean(&momentary, INTEGRATED_GATE_LU),
        range_lu: range(&short_term),
        momentary_max_lufs: peak_of(&momentary),
        short_term_max_lufs: peak_of(&short_term),
        sample_peak,
        sample_peak_dbfs: (sample_peak > 0.0).then(|| 20.0 * sample_peak.log10()),
        peak_note: PEAK_NOTE,
        momentary,
        short_term,
    }
}

/// A prefix sum, so a block costs two reads rather than a pass of its own.
fn running_squares(plane: &[f64], sr: f64) -> Vec<f64> {
    let shelf = shelf(sr);
    let cut = highpass(sr);
    let mut first = State::default();
    let mut second = State::default();
    let mut out = Vec::with_capacity(plane.len() + 1);
    let mut total = 0.0;
    out.push(0.0);
    for x in plane {
        let k = second.step(&cut, first.step(&shelf, *x));
        total += k * k;
        out.push(total);
    }
    out
}

fn blocks(
    squares: &[Vec<f64>],
    samples: usize,
    sr: f64,
    start_secs: f64,
    block_secs: f64,
) -> Vec<LoudnessFrame> {
    let n = (block_secs * sr).round() as usize;
    let step = ((STEP_SECS * sr).round() as usize).max(1);
    if n == 0 || samples < n {
        return Vec::new();
    }
    (0..=(samples - n))
        .step_by(step)
        .map(|at| LoudnessFrame {
            t: start_secs + at as f64 / sr,
            lufs: level(squares, at, n),
        })
        .collect()
}

fn level(squares: &[Vec<f64>], at: usize, n: usize) -> f64 {
    let power: f64 = squares.iter().map(|s| (s[at + n] - s[at]) / n as f64).sum();
    if power <= 0.0 {
        return f64::NEG_INFINITY;
    }
    OFFSET_DB + 10.0 * power.log10()
}

/// The mean runs on POWER, not on the levels: averaging decibels is not averaging loudness.
fn mean_above(frames: &[LoudnessFrame], threshold: f64) -> Option<f64> {
    let kept: Vec<f64> = frames
        .iter()
        .map(|f| f.lufs)
        .filter(|l| *l > threshold)
        .collect();
    if kept.is_empty() {
        return None;
    }
    let power: f64 = kept
        .iter()
        .map(|l| 10f64.powf((l - OFFSET_DB) / 10.0))
        .sum();
    Some(OFFSET_DB + 10.0 * (power / kept.len() as f64).log10())
}

/// Both relative gates measure down from the ABSOLUTE-gated mean, never from each other.
fn gated_mean(frames: &[LoudnessFrame], relative_lu: f64) -> Option<f64> {
    mean_above(frames, mean_above(frames, ABSOLUTE_GATE)? + relative_lu)
}

/// EBU Tech 3342, whose gate is deliberately not the integrated measure's.
fn range(short_term: &[LoudnessFrame]) -> Option<f64> {
    let floor = mean_above(short_term, ABSOLUTE_GATE)? + RANGE_GATE_LU;
    let mut kept: Vec<f64> = short_term
        .iter()
        .map(|f| f.lufs)
        .filter(|l| *l > ABSOLUTE_GATE && *l > floor)
        .collect();
    if kept.is_empty() {
        return None;
    }
    kept.sort_by(f64::total_cmp);
    Some(percentile(&kept, 0.95) - percentile(&kept, 0.10))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[at.min(sorted.len() - 1)]
}

fn peak_of(frames: &[LoudnessFrame]) -> Option<f64> {
    frames
        .iter()
        .map(|f| f.lufs)
        .filter(|l| l.is_finite())
        .reduce(f64::max)
}
