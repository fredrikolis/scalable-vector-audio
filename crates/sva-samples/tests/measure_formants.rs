// Concern: states what the all-pole model owes resonators whose poles are known | Non-concern: naming a peak (src/measure/pitch.rs) | IO: (samples) -> asserted resonances

use sva_samples::measure::formants::{
    Formant, FormantFrame, MAX_ORDER, analyze, default_order, track,
};

/// A signal whose envelope is known by construction rather than by another measurement.
/// Driven by noise and not an impulse: an envelope models a stationary frame.
fn resonated(x: &[f64], hz: f64, bandwidth_hz: f64, sample_rate: f64) -> Vec<f64> {
    let r = (-std::f64::consts::PI * bandwidth_hz / sample_rate).exp();
    let theta = 2.0 * std::f64::consts::PI * hz / sample_rate;
    let (b1, b2) = (2.0 * r * theta.cos(), -r * r);
    let (mut y1, mut y2) = (0.0f64, 0.0f64);
    x.iter()
        .map(|&sample| {
            let y = sample + b1 * y1 + b2 * y2;
            y2 = y1;
            y1 = y;
            y
        })
        .collect()
}

/// A keyed hash, so a run is deterministic without depending on an evaluator.
fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn drawn(seed: f64, index: i64) -> f64 {
    let bits = splitmix64((index as u64) ^ splitmix64(seed.to_bits()));
    (bits >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

fn noise(seed: f64, samples: usize) -> Vec<f64> {
    (0..samples).map(|n| drawn(seed, n as i64)).collect()
}

fn resonator(hz: f64, bandwidth_hz: f64, sample_rate: f64, samples: usize) -> Vec<f64> {
    resonated(&noise(1.0, samples), hz, bandwidth_hz, sample_rate)
}

fn nearest(frame: &FormantFrame, hz: f64) -> Formant {
    *frame
        .formants
        .iter()
        .min_by(|a, b| (a.hz - hz).abs().total_cmp(&(b.hz - hz).abs()))
        .expect("a resonator has a resonance")
}

#[test]
fn a_two_pole_resonator_reports_the_centre_and_bandwidth_it_was_built_with() {
    let sr = 16000.0;
    for (hz, bandwidth) in [(700.0, 90.0), (1220.0, 110.0), (2600.0, 170.0)] {
        let x = resonator(hz, bandwidth, sr, 32768);
        let frame = analyze(&x, sr, 2, 4);
        assert_eq!(frame.formants.len(), 1, "one pole pair, one formant");
        let found = frame.formants[0];
        assert!(
            (found.hz - hz).abs() < 5.0,
            "asked {hz} Hz, measured {} Hz",
            found.hz
        );
        assert!(
            (found.bandwidth_hz - bandwidth).abs() < 12.0,
            "asked {bandwidth} Hz wide, measured {} Hz",
            found.bandwidth_hz
        );
    }
}

/// Three resonators in cascade is a vowel's tract.
#[test]
fn a_three_resonance_cascade_reports_all_three_at_a_higher_order() {
    let sr = 16000.0;
    let targets = [(730.0, 90.0), (1090.0, 110.0), (2440.0, 170.0)];
    let mut x = noise(2.0, 32768);
    for (hz, bandwidth) in targets {
        x = resonated(&x, hz, bandwidth, sr);
    }
    let frame = analyze(&x, sr, 10, 8);
    for (hz, bandwidth) in targets {
        let found = nearest(&frame, hz);
        assert!(
            (found.hz - hz).abs() < 10.0,
            "asked {hz} Hz, nearest measured {} Hz of {:?}",
            found.hz,
            frame.formants
        );
        assert!(
            (found.bandwidth_hz - bandwidth).abs() < 15.0,
            "asked {bandwidth} Hz wide, measured {} Hz",
            found.bandwidth_hz
        );
    }
    assert!(
        frame.residual < frame.energy,
        "the model predicted something"
    );
}

#[test]
fn the_order_is_the_rule_of_thumb_unless_a_caller_says_otherwise() {
    assert_eq!(default_order(44100.0), 46);
    assert_eq!(default_order(8000.0), 10);
    assert_eq!(default_order(192_000.0), MAX_ORDER, "clamped, not guessed");

    let x = resonator(700.0, 90.0, 16000.0, 512);
    assert_eq!(analyze(&x, 16000.0, 12, 8).order, 12);
    assert_eq!(analyze(&x, 16000.0, 400, 8).order, MAX_ORDER);
    assert_eq!(analyze(&x[..9], 16000.0, 12, 8).order, 8, "a short frame");
}

/// The whole reason this exists: a voiced signal's peaks are the glottal source's harmonics,
/// and the formants sit under them without moving with f0.
#[test]
fn a_voiced_source_through_a_tract_reports_the_tract_and_not_its_harmonics() {
    let sr = 16000.0;
    let targets = [(730.0, 90.0), (1090.0, 110.0), (2440.0, 170.0)];
    for f0 in [98.0, 220.0] {
        let mut x = glottis(f0, sr, 32768);
        for (hz, bandwidth) in targets {
            x = resonated(&x, hz, bandwidth, sr);
        }
        let frame = analyze(&x, sr, 14, 8);
        for (hz, _) in targets {
            let found = nearest(&frame, hz);
            assert!(
                (found.hz - hz).abs() < 60.0,
                "f0 {f0}: asked {hz} Hz, nearest measured {} Hz of {:?}",
                found.hz,
                frame.formants
            );
        }
    }
}

fn glottis(f0: f64, sample_rate: f64, samples: usize) -> Vec<f64> {
    (0..samples)
        .map(|n| {
            let x = (f0 * n as f64 / sample_rate).rem_euclid(1.0) / 0.6;
            (x * x - x * x * x).max(0.0)
        })
        .collect()
}

#[test]
fn silence_and_an_empty_frame_report_no_resonance_rather_than_a_guess() {
    for frame in [
        analyze(&[], 16000.0, 12, 8),
        analyze(&[0.0; 512], 16000.0, 12, 8),
    ] {
        assert!(frame.formants.is_empty());
        assert_eq!(frame.energy, 0.0);
        assert_eq!(frame.residual, 0.0);
    }
}

#[test]
fn a_track_places_each_frame_in_time_and_follows_a_moving_resonance() {
    let sr = 16000.0;
    let mut x = resonator(500.0, 80.0, sr, 8000);
    x.extend(resonated(&noise(3.0, 8000), 1800.0, 80.0, sr));
    let frames = track(&x, sr, 0.25, 0.5, 4, 4);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].t_secs, 0.25);
    assert_eq!(frames[1].t_secs, 0.75);
    assert!((nearest(&frames[0], 500.0).hz - 500.0).abs() < 20.0);
    assert!((nearest(&frames[1], 1800.0).hz - 1800.0).abs() < 20.0);
}
