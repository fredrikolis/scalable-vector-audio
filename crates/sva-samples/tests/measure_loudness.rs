// Concern: states the BS.1770 readings a K-weighted measure owes its own conformance tones | Non-concern: per-component level (src/measure/envelope.rs) | IO: (planes) -> asserted LUFS

use sva_samples::measure::loudness::{Loudness, analyze};

fn sine(hz: f64, sr: f64, secs: f64, amp: f64) -> Vec<f64> {
    (0..(secs * sr) as usize)
        .map(|i| amp * (2.0 * std::f64::consts::PI * hz * i as f64 / sr).sin())
        .collect()
}

fn of(planes: &[&[f64]], sr: f64) -> Loudness {
    analyze(planes, sr, 0.0)
}

/// BS.1770-2 conformance, which pins offset, shelf and highpass at once.
#[test]
fn a_minus_twenty_dbfs_kilohertz_sine_reads_minus_twenty_three_lkfs() {
    let s = sine(1000.0, 48000.0, 5.0, 0.1);
    let l = of(&[&s], 48000.0);
    let integrated = l.integrated_lufs.expect("five seconds is many blocks");
    assert!(
        (integrated - -23.0).abs() < 0.1,
        "read {integrated} LKFS, not -23.0"
    );
    assert!((l.momentary_max_lufs.unwrap() - -23.0).abs() < 0.1);
    assert!((l.short_term_max_lufs.unwrap() - -23.0).abs() < 0.1);
}

/// Why loudness is not reported per component the way level and spectrum are.
#[test]
fn two_identical_components_read_three_decibels_louder_than_one() {
    let s = sine(1000.0, 48000.0, 5.0, 0.1);
    let one = of(&[&s], 48000.0).integrated_lufs.unwrap();
    let two = of(&[&s, &s], 48000.0).integrated_lufs.unwrap();
    assert!((two - one - 3.0103).abs() < 0.01, "{one} -> {two}");
}

#[test]
fn silence_is_gated_out_rather_than_averaged_in() {
    let sr = 48000.0;
    let mut loud = sine(1000.0, sr, 5.0, 0.1);
    let quiet = vec![0.0; (5.0 * sr) as usize];
    let alone = of(&[&loud], sr).integrated_lufs.unwrap();
    loud.extend_from_slice(&quiet);
    let padded = of(&[&loud], sr).integrated_lufs.unwrap();
    assert!(
        (padded - alone).abs() < 0.2,
        "{alone} -> {padded}: only the three blocks straddling the edge are half-full"
    );
}

#[test]
fn the_range_is_the_spread_of_the_short_term_distribution() {
    let sr = 48000.0;
    let steady = sine(1000.0, sr, 20.0, 0.1);
    assert!(of(&[&steady], sr).range_lu.unwrap() < 0.01);

    let mut stepped = sine(1000.0, sr, 20.0, 0.1);
    stepped.extend(sine(1000.0, sr, 20.0, 0.1 * 10f64.powf(-10.0 / 20.0)));
    let range = of(&[&stepped], sr).range_lu.unwrap();
    assert!((range - 10.0).abs() < 0.6, "{range} LU across a 10 dB step");
}

#[test]
fn a_node_shorter_than_a_block_reports_no_level_but_still_a_peak() {
    let s = sine(1000.0, 48000.0, 0.2, 0.5);
    let l = of(&[&s], 48000.0);
    assert!(l.integrated_lufs.is_none());
    assert!(l.momentary.is_empty() && l.short_term.is_empty());
    assert!((l.sample_peak - 0.5).abs() < 0.01);
    assert!((l.sample_peak_dbfs.unwrap() - -6.02).abs() < 0.1);
}

#[test]
fn digital_silence_has_no_loudness_and_no_peak_in_decibels() {
    let s = vec![0.0; 48000 * 5];
    let l = of(&[&s], 48000.0);
    assert_eq!(l.integrated_lufs, None);
    assert_eq!(l.sample_peak, 0.0);
    assert_eq!(l.sample_peak_dbfs, None);
}
