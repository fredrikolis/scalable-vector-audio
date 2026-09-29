// Concern: states when a rate change leaves audible alias and when none | Non-concern: producing either signal | IO: (base, oversampled) -> dB

use sva_samples::measure::alias::{AUDIBLE_NMR_DB, measure_alias};

fn sine(hz: f64, sr: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 2.0 * std::f64::consts::PI * hz * i as f64 / sr)
        .map(|p| p.sin())
        .collect()
}

/// One sine at two rates is one sine: nothing is left to call alias.
#[test]
fn a_signal_that_survives_the_rate_change_measures_no_alias() {
    let sr = 44100.0;
    let n = 8192;
    let k = 8;
    let m = measure_alias(
        &sine(440.0, sr, n),
        &sine(440.0, sr * k as f64, n * k),
        k,
        sr,
        0.0,
    );
    assert!(m.asr_db < -60.0, "asr {}", m.asr_db);
    assert!(m.nmr_db < AUDIBLE_NMR_DB, "nmr {}", m.nmr_db);
    assert!(!m.audible);
    assert!(m.scored_frames > 0);
}

/// A 6 kHz saw has one harmonic under Nyquist; the rest folded down.
#[test]
fn a_folded_harmonic_reads_as_audible_alias() {
    let sr = 44100.0;
    let n = 8192;
    let k = 8;
    let saw = |rate: f64, len: usize| -> Vec<f64> {
        (0..len)
            .map(|i| {
                let p = 6000.0 * i as f64 / rate;
                2.0 * (p - p.floor()) - 1.0
            })
            .collect()
    };
    let m = measure_alias(&saw(sr, n), &saw(sr * k as f64, n * k), k, sr, 0.0);
    assert!(m.asr_db > -20.0, "asr {}", m.asr_db);
    assert!(m.audible, "nmr {}", m.nmr_db);
}

/// `peak_at_secs` sits beside a window's own `start_secs`, so it counts from the same zero.
#[test]
fn the_loudest_alias_is_named_on_the_renders_own_timeline() {
    let sr = 44100.0;
    let n = 8192;
    let k = 8;
    let saw = |rate: f64, len: usize| -> Vec<f64> {
        (0..len)
            .map(|i| {
                let p = 6000.0 * i as f64 / rate;
                2.0 * (p - p.floor()) - 1.0
            })
            .collect()
    };
    let from_zero = measure_alias(&saw(sr, n), &saw(sr * k as f64, n * k), k, sr, 0.0);
    let from_two = measure_alias(&saw(sr, n), &saw(sr * k as f64, n * k), k, sr, 2.0);
    assert_eq!(
        from_two.peak_at_secs,
        from_zero.peak_at_secs + 2.0,
        "a window starting at 2s names its peak two seconds later"
    );
}
