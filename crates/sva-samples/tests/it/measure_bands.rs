// Concern: states what the ERB envelope bank owes a signal passing through it | Non-concern: the discrete spectrum (src/measure/spectrum.rs) | IO: (samples) -> asserted tracks

use sva_samples::measure::bands::{BAND_COUNT, BandTrack, Bands, analyze, cam, erb_hz, hz_at_cam};

const SR: f64 = 44100.0;

fn band_at(bands: &Bands, hz: f64) -> &BandTrack {
    bands
        .bands
        .iter()
        .min_by(|a, b| {
            (a.centre_hz - hz)
                .abs()
                .total_cmp(&(b.centre_hz - hz).abs())
        })
        .expect("the bank is not empty")
}

#[test]
fn the_scale_is_the_published_one_and_inverts() {
    assert!((erb_hz(1000.0) - 132.6).abs() < 0.1);
    assert!((erb_hz(100.0) - 35.5).abs() < 0.1);
    assert!((cam(1000.0) - 15.6).abs() < 0.1);
    for hz in [50.0, 100.0, 1000.0, 5000.0, 16000.0] {
        assert!((hz_at_cam(cam(hz)) - hz).abs() < 1e-6, "{hz}");
    }
}

#[test]
fn each_band_sits_at_the_q_its_own_bandwidth_states() {
    let bands = analyze(&[0.0; 64], SR, 0.0);
    assert_eq!(bands.bands.len(), BAND_COUNT);
    for (hz, want) in [(100.0, 2.8), (1000.0, 7.5), (5000.0, 8.9)] {
        let b = band_at(&bands, hz);
        assert!((b.q - want).abs() < 0.3, "{hz} Hz sat at q {}", b.q);
    }
    assert!(bands.rate_hz > 1900.0 && bands.rate_hz < 2100.0);
}

#[test]
fn an_impulse_measures_as_each_bands_own_floor() {
    let mut x = vec![0.0; 44100];
    x[0] = 1.0;
    let bands = analyze(&x, SR, 0.0);
    for b in &bands.bands {
        assert!((b.peak - b.floor.peak).abs() < 1e-9, "{} Hz", b.centre_hz);
        assert!(
            (b.time_to_peak_secs.unwrap() - b.floor.time_to_peak_secs).abs() < 1e-9,
            "{} Hz",
            b.centre_hz
        );
    }
}

#[test]
fn the_floor_lengthens_as_the_band_narrows_and_is_not_the_old_rule_of_thumb() {
    let bands = analyze(&[0.0; 64], SR, 0.0);
    let rise = |hz: f64| band_at(&bands, hz).floor.rise_10_90_secs.unwrap();
    assert!(rise(63.0) > rise(500.0), "a low band must ring longer");
    assert!(rise(500.0) > rise(5000.0));
    for hz in [63.0, 125.0, 250.0, 500.0, 1000.0] {
        assert!(rise(hz) > 0.0 && rise(hz) < 0.5, "{hz} Hz: {}", rise(hz));
    }
}

#[test]
fn an_exponential_decay_falls_at_the_rate_it_was_built_with() {
    let tau = 0.05;
    let hz = 2000.0;
    let x: Vec<f64> = (0..(2.0 * SR) as usize)
        .map(|i| {
            let t = i as f64 / SR;
            (2.0 * std::f64::consts::PI * hz * t).sin() * (-t / tau).exp()
        })
        .collect();
    let bands = analyze(&x, SR, 0.0);
    let b = band_at(&bands, hz);
    let rate = bands.rate_hz;
    let at = |secs: f64| b.rms[(secs * rate) as usize];
    for (a, c) in [(0.1, 0.2), (0.2, 0.3), (0.3, 0.4)] {
        let measured = -(c - a) / (at(c) / at(a)).ln();
        assert!(
            (measured - tau).abs() < 0.004,
            "{a}..{c} s read tau {measured}"
        );
    }
}

#[test]
fn a_swept_sine_peaks_in_each_band_when_it_passes_that_bands_centre() {
    let (secs, lo, hi): (f64, f64, f64) = (4.0, 100.0, 8000.0);
    let k = (hi / lo).ln() / secs;
    let x: Vec<f64> = (0..(secs * SR) as usize)
        .map(|i| {
            let t = i as f64 / SR;
            let phase = 2.0 * std::f64::consts::PI * lo * ((k * t).exp() - 1.0) / k;
            phase.sin()
        })
        .collect();
    let bands = analyze(&x, SR, 0.0);
    let heard: Vec<&BandTrack> = bands
        .bands
        .iter()
        .filter(|b| b.centre_hz > 150.0 && b.centre_hz < 6000.0)
        .collect();
    let mut previous = 0.0;
    for b in &heard {
        let arrived = b.time_to_peak_secs.expect("a swept band peaks");
        let expected = (b.centre_hz / lo).ln() / k;
        assert!(
            (arrived - expected).abs() < 0.06,
            "{} Hz peaked at {arrived}, the sweep reached it at {expected}",
            b.centre_hz
        );
        assert!(arrived > previous, "bands must arrive in order");
        previous = arrived;
    }
}

#[test]
fn a_silent_band_reports_no_time_to_peak_at_all() {
    let bands = analyze(&[0.0; 8192], SR, 0.0);
    for b in &bands.bands {
        assert_eq!(b.peak, 0.0);
        assert_eq!(b.time_to_peak_secs, None);
        assert_eq!(b.rise_10_90_secs, None);
        assert!(b.floor.peak > 0.0, "the floor is measured, not the signal");
    }
}

#[test]
fn a_rate_that_cannot_hold_the_top_bands_reports_the_ones_it_can() {
    let narrow = analyze(&[0.0; 64], 8000.0, 0.0);
    assert!(narrow.bands.len() < BAND_COUNT);
    assert!(narrow.bands.iter().all(|b| b.centre_hz < 3600.0));
}
