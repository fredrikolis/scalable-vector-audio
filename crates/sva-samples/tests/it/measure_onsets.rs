// Concern: states what onset detection owes a click track, silence and one long transient | Non-concern: the spectrum under the flux (measure_spectrum.rs) | IO: (samples) -> asserted onsets

use sva_samples::measure::onsets::detect;

fn click_track(sr: f64, secs: f64, gap_secs: f64) -> Vec<f64> {
    let n = (secs * sr) as usize;
    let gap = (gap_secs * sr) as usize;
    let mut out = vec![0.0; n];
    let mut at = 0;
    while at + 8 < n {
        for (i, s) in out[at..at + 8].iter_mut().enumerate() {
            *s = (1.0 - i as f64 / 8.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        at += gap;
    }
    out
}

#[test]
fn evenly_spaced_clicks_are_found_at_roughly_their_own_spacing() {
    let (sr, gap) = (44_100.0, 0.25);
    let found = detect(&click_track(sr, 4.0, gap), sr, 0.0);
    assert!(found.onsets.len() >= 12, "{}", found.onsets.len());
    for w in found.onsets.windows(2) {
        let ioi = w[1].t_secs - w[0].t_secs;
        assert!((ioi - gap).abs() < 0.03, "ioi {ioi} far from {gap}");
    }
}

#[test]
fn an_onset_is_placed_on_the_buffers_own_clock() {
    let sr = 8_000.0;
    let from_zero = detect(&click_track(sr, 1.0, 0.25), sr, 0.0);
    let later = detect(&click_track(sr, 1.0, 0.25), sr, 2.0);
    let shifted: Vec<f64> = later.onsets.iter().map(|o| o.t_secs - 2.0).collect();
    let own: Vec<f64> = from_zero.onsets.iter().map(|o| o.t_secs).collect();
    assert!(!own.is_empty());
    for (a, b) in own.iter().zip(&shifted) {
        assert!((a - b).abs() < 1e-9, "{own:?} vs {shifted:?}");
    }
}

#[test]
fn silence_holds_no_onsets() {
    let found = detect(&vec![0.0; 44_100], 44_100.0, 0.0);
    assert!(found.onsets.is_empty());
    assert!(found.ioi_histogram.iter().all(|b| b.count == 0));
}

#[test]
fn a_close_double_trigger_on_one_transient_is_suppressed() {
    let sr = 44_100.0;
    let mut samples = vec![0.0; (sr * 0.2) as usize];
    for (i, s) in samples.iter_mut().enumerate().take(200) {
        *s = (1.0 - i as f64 / 200.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
    }
    let found = detect(&samples, sr, 0.0);
    assert!(found.onsets.len() <= 1, "{:?}", found.onsets);
}
