// Concern: spectral flux onset detection against the composition's own tempo grid | Non-concern: generative syncopation models, gating a window | IO: (&[f32], rate, tempo) -> Onsets or refusal

use std::cmp::Ordering;

use crate::frame::{SpectralFrame, spectral_frames};
use crate::{AnalysisError, TempoGrid};

/// ~23ms: short enough to localize a transient, long enough for low percussion.
pub const FRAME_SECS: f64 = 0.023;
/// 50% overlap, the usual tradeoff of time resolution against flux stability.
pub const HOP_SECS: f64 = FRAME_SECS / 2.0;
/// Below this two novelty peaks are one transient's attack and decay.
pub const MIN_ONSET_GAP_SECS: f64 = 0.05;
/// Frames either side of a candidate that set its local adaptive threshold.
pub const ADAPTIVE_WINDOW_FRAMES: usize = 10;
pub const THRESHOLD_MULTIPLIER: f64 = 1.5;
pub const THRESHOLD_DELTA: f64 = 1e-6;
pub const IOI_BUCKET_SECS: f64 = 0.025;
pub const IOI_MAX_SECS: f64 = 2.0;
pub const RISE_FRACTION: f64 = 0.1;
/// Under this share of the loudest frame a rise is leakage.
pub const LEVEL_FRACTION: f64 = 0.03;
/// A strike's swing crests within this of leaving the floor.
pub const MAX_RISE_SECS: f64 = 0.012;

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
    pub onsets_per_bar: Option<Vec<usize>>,
    pub syncopation_index: Option<f64>,
    pub resolution_secs: f64,
}

pub fn detect(
    samples: &[f32],
    sample_rate: f64,
    start_secs: f64,
    tempo: Option<TempoGrid>,
) -> Result<Onsets, AnalysisError> {
    let duration_secs = samples.len() as f64 / sample_rate;
    if let Some(t) = tempo {
        if t.seconds_per_bar.partial_cmp(&0.0) != Some(Ordering::Greater) {
            return Err(AnalysisError(format!(
                "a bar of {} seconds is no tempo grid; `onsets` needs a bar of positive length",
                t.seconds_per_bar
            )));
        }
        // More bars than samples leaves bars no sample holds, and a count no buffer can be.
        if duration_secs / t.seconds_per_bar > samples.len() as f64 {
            return Err(AnalysisError(format!(
                "a bar of {:e} seconds divides {} samples into more bars than there are \
                 samples; `onsets` needs a bar at least one sample long",
                t.seconds_per_bar,
                samples.len()
            )));
        }
    }
    let frames = whole_frames(samples, sample_rate, start_secs);
    let floor = LEVEL_FRACTION * frames.iter().map(|f| total(&f.mags)).fold(0.0, f64::max);
    let flux = flux_of(&frames, floor);
    let onsets = pick_peaks(&flux, &frames, samples, sample_rate, start_secs, floor);
    Ok(Onsets {
        ioi_histogram: ioi_histogram(&onsets),
        onsets_per_bar: tempo
            .map(|t| per_bar(&onsets, start_secs, duration_secs, t.seconds_per_bar)),
        syncopation_index: tempo.map(|t| syncopation(&onsets, t)),
        onsets,
        resolution_secs: HOP_SECS,
    })
}

fn whole_frames(samples: &[f32], sample_rate: f64, start_secs: f64) -> Vec<SpectralFrame> {
    let mut frames = spectral_frames(samples, sample_rate, start_secs, FRAME_SECS, HOP_SECS);
    frames.truncate(frames.iter().take_while(|f| f.filled).count());
    frames
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

/// A candidate clears the threshold, the floor and both rise bounds, peaks over the gap.
fn pick_peaks(
    flux: &[f64],
    frames: &[SpectralFrame],
    samples: &[f32],
    sample_rate: f64,
    start_secs: f64,
    floor: f64,
) -> Vec<Onset> {
    let mut onsets: Vec<Onset> = Vec::new();
    for i in 0..flux.len() {
        let lo = i.saturating_sub(ADAPTIVE_WINDOW_FRAMES);
        let hi = (i + ADAPTIVE_WINDOW_FRAMES + 1).min(flux.len());
        let local = &flux[lo..hi];
        let mean = local.iter().sum::<f64>() / local.len() as f64;
        let threshold = THRESHOLD_DELTA + THRESHOLD_MULTIPLIER * mean;
        // A peak stands over the gap two onsets need, not the threshold's own window.
        let reach = (MIN_ONSET_GAP_SECS / HOP_SECS).round().max(1.0) as usize;
        let near = &flux[i.saturating_sub(reach)..(i + reach + 1).min(flux.len())];
        if flux[i] <= threshold || near.iter().any(|&v| v > flux[i]) {
            continue;
        }
        let span = frames[i].span_secs;
        if flux[i] < floor || climb_secs(flux, i, floor) > span + HOP_SECS {
            continue;
        }
        // A frame says which transient; its samples say when, and how fast it rose.
        let from = frames[i].t_secs - start_secs;
        let Some((at, rise_secs)) = attack(samples, sample_rate, from, span) else {
            continue;
        };
        if rise_secs > MAX_RISE_SECS {
            continue;
        }
        let t = start_secs + at;
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

/// How long the flux has been climbing; a swell climbs every frame it crosses.
fn climb_secs(flux: &[f64], i: usize, floor: f64) -> f64 {
    let mut from = i;
    while from > 0 && flux[from - 1] >= floor {
        from -= 1;
    }
    (i + 1 - from) as f64 * HOP_SECS
}

/// When the swing left its window's floor, and its rise from there to the crest.
fn attack(samples: &[f32], sample_rate: f64, from_secs: f64, span_secs: f64) -> Option<(f64, f64)> {
    let at = |secs: f64| ((secs * sample_rate).round().max(0.0) as usize).min(samples.len());
    let (from, to) = (at(from_secs), at(from_secs + span_secs));
    let held = samples.get(from..to)?;
    let peak = held.iter().fold(0.0f64, |a, s| a.max(f64::from(*s).abs()));
    if peak == 0.0 {
        return None;
    }
    let above = |bar: f64| held.iter().position(|s| f64::from(*s).abs() >= bar);
    let struck = above(peak * RISE_FRACTION)?;
    let crest = above(peak * (1.0 - RISE_FRACTION)).unwrap_or(struck);
    Some((
        (from + struck) as f64 / sample_rate,
        crest.saturating_sub(struck) as f64 / sample_rate,
    ))
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

fn per_bar(
    onsets: &[Onset],
    start_secs: f64,
    duration_secs: f64,
    seconds_per_bar: f64,
) -> Vec<usize> {
    let bars = ((duration_secs / seconds_per_bar).ceil() as usize).max(1);
    let mut counts = vec![0usize; bars];
    for o in onsets {
        let bar = (((o.t_secs - start_secs) / seconds_per_bar) as usize).min(bars - 1);
        counts[bar] += 1;
    }
    counts
}

/// A position's metrical weight is how many binary halvings of the bar still land on it — the
/// downbeat survives every halving, an off-16th survives none. Odd meters have no such ladder,
/// so they fall back to a coarser on-beat/off-beat read.
fn metrical_weight(k: u32, subdivisions: u32, beats_per_bar: u32) -> f64 {
    if k == 0 {
        return 1.0;
    }
    if subdivisions.is_power_of_two() {
        return (k.trailing_zeros() + 1) as f64 / (subdivisions.trailing_zeros() + 1) as f64;
    }
    let per_beat = (subdivisions / beats_per_bar).max(1);
    if k.is_multiple_of(per_beat) { 0.5 } else { 0.0 }
}

/// Mean off-grid-ness across every onset, 0 (all on the beat grid) to 1 (all off it). A
/// simplified heuristic, not a generative model like Longuet-Higgins and Lee's.
fn syncopation(onsets: &[Onset], tempo: TempoGrid) -> f64 {
    if onsets.is_empty() {
        return 0.0;
    }
    let beats_per_bar = tempo.beats_per_bar.round().max(1.0) as u32;
    let subdivisions = (beats_per_bar * 4).max(1);
    let total: f64 = onsets
        .iter()
        .map(|o| {
            let phase = o.t_secs.rem_euclid(tempo.seconds_per_bar) / tempo.seconds_per_bar;
            let k = (phase * subdivisions as f64).round() as u32 % subdivisions;
            1.0 - metrical_weight(k, subdivisions, beats_per_bar)
        })
        .sum();
    total / onsets.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_track(sr: f64, secs: f64, gap_secs: f64) -> Vec<f32> {
        let n = (secs * sr) as usize;
        let gap = (gap_secs * sr) as usize;
        let mut out = vec![0.0f32; n];
        let mut at = 0;
        while at + 8 < n {
            for (i, s) in out[at..at + 8].iter_mut().enumerate() {
                *s = (1.0 - i as f32 / 8.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
            }
            at += gap;
        }
        out
    }

    #[test]
    fn evenly_spaced_clicks_are_found_at_roughly_their_own_spacing() {
        let sr = 44100.0;
        let gap = 0.25;
        let samples = click_track(sr, 4.0, gap);
        let found = detect(&samples, sr, 0.0, None).expect("no tempo, no grid");
        assert!(found.onsets.len() >= 12, "{}", found.onsets.len());
        for w in found.onsets.windows(2) {
            let ioi = w[1].t_secs - w[0].t_secs;
            assert!((ioi - gap).abs() < 0.03, "ioi {ioi} far from {gap}");
        }
    }

    #[test]
    fn silence_holds_no_onsets() {
        let found = detect(&vec![0.0f32; 44100], 44100.0, 0.0, None).expect("no tempo");
        assert!(found.onsets.is_empty());
        assert!(found.onsets_per_bar.is_none());
        assert!(found.syncopation_index.is_none());
    }

    #[test]
    fn a_close_double_trigger_on_one_transient_is_suppressed() {
        let sr = 44100.0;
        let mut samples = vec![0.0f32; (sr * 0.2) as usize];
        for (i, s) in samples.iter_mut().enumerate().take(200) {
            *s = (1.0 - i as f32 / 200.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        let found = detect(&samples, sr, 0.0, None).expect("no tempo, no grid");
        assert!(found.onsets.len() <= 1, "{:?}", found.onsets);
    }

    #[test]
    fn onsets_squarely_on_the_downbeat_read_as_barely_syncopated() {
        let sr = 44100.0;
        let tempo = TempoGrid {
            seconds_per_bar: 2.0,
            beats_per_bar: 4.0,
        };
        let samples = click_track(sr, 8.0, 2.0);
        let found = detect(&samples, sr, 0.0, Some(tempo)).expect("a two-second bar");
        assert!(
            found.syncopation_index.unwrap() < 0.2,
            "{:?}",
            found.syncopation_index
        );
        assert_eq!(found.onsets_per_bar.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn metrical_weight_favours_the_downbeat_over_an_off_sixteenth() {
        assert!(metrical_weight(0, 16, 4) > metrical_weight(1, 16, 4));
        assert!(metrical_weight(8, 16, 4) > metrical_weight(1, 16, 4));
    }
}
