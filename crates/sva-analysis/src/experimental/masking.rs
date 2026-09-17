// Concern: per-band signal-to-mask ratio, subtracting the target back out of the mix | Non-concern: onset detection (onsets.rs), the FFT (magnitudes) | IO: (target, mix) -> Masking or AnalysisError

use std::ops::Range;

use sva_samples::{magnitudes, third_octave_edges};

use crate::AnalysisError;
use crate::decibels::to_db;
use crate::stable::onsets::Onset;

/// The span after an onset a gated window scores: long enough for the attack and early
/// sustain, short enough to stay that onset's own rather than sliding into the next.
pub const GATE_WINDOW_SECS: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskingBand {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub target_db: f64,
    pub against_db: f64,
    /// `target_db - against_db`; a silent side leaves no finite ratio and reads `null`. The
    /// two levels beside it say which side was silent.
    pub smr_db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Masking {
    pub gated: bool,
    pub frames_scored: usize,
    pub bands: Vec<MaskingBand>,
}

/// `combined` must be the buffer `target` was mixed additively into (a bus, `song`); this
/// subtracts `target` back out of it before measuring, so the target is never counted as its
/// own masker. `gate` scores only the target's own onset windows rather than the whole buffer.
pub fn analyze(
    target: &[f32],
    combined: &[f32],
    sample_rate: f64,
    start_secs: f64,
    gate: Option<&[Onset]>,
) -> Result<Masking, AnalysisError> {
    if target.len() != combined.len() {
        return Err(AnalysisError(format!(
            "target is {} samples and combined is {}; they must share one window",
            target.len(),
            combined.len()
        )));
    }
    let complement: Vec<f32> = target.iter().zip(combined).map(|(&t, &c)| c - t).collect();
    let windows: Vec<Range<usize>> = match gate {
        None => std::iter::once(0..target.len()).collect(),
        Some(onsets) => onset_windows(onsets, sample_rate, start_secs, target.len()),
    };

    let edges = third_octave_edges(sample_rate);
    let target_power = band_power(target, sample_rate, &edges, &windows);
    let against_power = band_power(&complement, sample_rate, &edges, &windows);
    let bands = edges
        .into_iter()
        .zip(target_power.into_iter().zip(against_power))
        .map(|((lo_hz, hi_hz), (t, a))| {
            let target_db = to_db(t.sqrt());
            let against_db = to_db(a.sqrt());
            MaskingBand {
                lo_hz,
                hi_hz,
                target_db,
                against_db,
                smr_db: target_db - against_db,
            }
        })
        .collect();

    Ok(Masking {
        gated: gate.is_some(),
        frames_scored: windows.len(),
        bands,
    })
}

fn onset_windows(
    onsets: &[Onset],
    sample_rate: f64,
    start_secs: f64,
    len: usize,
) -> Vec<Range<usize>> {
    onsets
        .iter()
        .filter_map(|o| {
            let s = (((o.t_secs - start_secs) * sample_rate).round().max(0.0) as usize).min(len);
            let e = (s + (GATE_WINDOW_SECS * sample_rate).round() as usize).min(len);
            (e > s).then_some(s..e)
        })
        .collect()
}

fn band_power(
    samples: &[f32],
    sample_rate: f64,
    edges: &[(f64, f64)],
    windows: &[Range<usize>],
) -> Vec<f64> {
    let mut sums = vec![0.0; edges.len()];
    let mut scored = 0usize;
    for w in windows {
        let slice = &samples[w.clone()];
        if slice.is_empty() {
            continue;
        }
        let frame: Vec<f64> = slice.iter().map(|&s| f64::from(s)).collect();
        let (mags, bin_hz, ..) = magnitudes(&frame, sample_rate, None);
        for (b, &(lo, hi)) in edges.iter().enumerate() {
            sums[b] += mags
                .iter()
                .enumerate()
                .filter(|&(k, _)| {
                    let hz = k as f64 * bin_hz;
                    hz >= lo && hz < hi
                })
                .map(|(_, m)| m * m)
                .sum::<f64>();
        }
        scored += 1;
    }
    if scored > 0 {
        for s in &mut sums {
            *s /= scored as f64;
        }
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, sr: f64, secs: f64, amp: f32) -> Vec<f32> {
        (0..(secs * sr) as usize)
            .map(|i| amp * (2.0 * std::f64::consts::PI * hz * i as f64 / sr).sin() as f32)
            .collect()
    }

    /// `combined` here IS the target, so a correct exclusion leaves nothing to mask it with.
    #[test]
    fn a_target_reads_above_0_db_against_its_own_true_complement() {
        let sr = 44100.0;
        let target = tone(1000.0, sr, 1.0, 0.5);
        let combined = target.clone();
        let m = analyze(&target, &combined, sr, 0.0, None).unwrap();
        let own_band = m
            .bands
            .iter()
            .find(|b| b.lo_hz <= 1000.0 && 1000.0 < b.hi_hz)
            .unwrap();
        assert!(own_band.smr_db > 0.0, "{:?}", own_band);
        assert!(
            own_band.against_db < -100.0,
            "silence left after subtracting the target out of itself: {:?}",
            own_band
        );
    }

    #[test]
    fn a_target_buried_under_a_much_louder_complement_reads_negative_smr() {
        let sr = 44100.0;
        let target = tone(1000.0, sr, 1.0, 0.05);
        let masker = tone(1000.0, sr, 1.0, 0.9);
        let combined: Vec<f32> = target.iter().zip(&masker).map(|(&t, &m)| t + m).collect();
        let m = analyze(&target, &combined, sr, 0.0, None).unwrap();
        let own_band = m
            .bands
            .iter()
            .find(|b| b.lo_hz <= 1000.0 && 1000.0 < b.hi_hz)
            .unwrap();
        assert!(own_band.smr_db < -10.0, "{:?}", own_band);
    }

    #[test]
    fn mismatched_lengths_refuse_rather_than_panic() {
        let err = analyze(&[0.0; 10], &[0.0; 5], 44100.0, 0.0, None).unwrap_err();
        assert!(err.0.contains("10"));
    }

    #[test]
    fn a_gated_window_scores_only_the_span_after_each_onset() {
        let sr = 44100.0;
        let target = tone(1000.0, sr, 1.0, 0.5);
        let combined = target.clone();
        let onsets = [Onset {
            t_secs: 0.5,
            strength: 1.0,
        }];
        let gated = analyze(&target, &combined, sr, 0.0, Some(&onsets)).unwrap();
        assert!(gated.gated);
        assert_eq!(gated.frames_scored, 1);
    }
}
