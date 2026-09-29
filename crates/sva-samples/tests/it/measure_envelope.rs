// Concern: states what a level trace owes a signal of known shape | Non-concern: per-band level (measure_bands.rs), loudness weighting (measure_loudness.rs) | IO: (samples) -> asserted frames

use sva_samples::measure::envelope::{rms, trace};

const RATE: f64 = 44_100.0;

fn tone(hz: f64, secs: f64, amp: f64) -> Vec<f64> {
    let len = (secs * RATE) as usize;
    (0..len)
        .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin())
        .collect()
}

/// A steady tone has one level, and every frame says so at the second it covers.
#[test]
fn a_steady_tone_traces_one_level_at_its_own_seconds() {
    let frames = trace(&tone(440.0, 0.5, 0.5), RATE, 0.0, 0.05);
    assert_eq!(frames.len(), 10, "half a second in 50 ms steps");
    for (n, frame) in frames.iter().enumerate() {
        assert!(
            (frame.t_secs - n as f64 * 0.05).abs() < 1e-9,
            "frame {n} at {}",
            frame.t_secs
        );
        assert!(
            (frame.rms - 0.5 / 2f64.sqrt()).abs() < 0.01,
            "a sine's RMS is its amplitude over root two: {frame:?}"
        );
        assert!((frame.peak - 0.5).abs() < 0.01, "{frame:?}");
    }
}

/// The window decides what second a frame is at, so a trace from 2s names 2s, not zero.
#[test]
fn a_trace_counts_from_the_window_it_was_taken_over() {
    let held = tone(440.0, 0.2, 0.5);
    let from_zero = trace(&held, RATE, 0.0, 0.05);
    let from_two = trace(&held, RATE, 2.0, 0.05);
    assert_eq!(from_zero.len(), from_two.len());
    for (early, late) in from_zero.iter().zip(&from_two) {
        assert!((late.t_secs - early.t_secs - 2.0).abs() < 1e-9, "{late:?}");
        assert_eq!(early.rms, late.rms, "only the clock moved");
    }
}

/// A decay is a falling level, which is the one thing a trace exists to show.
#[test]
fn a_decaying_tone_falls_frame_by_frame() {
    let len = (RATE * 0.5) as usize;
    let decaying: Vec<f64> = (0..len)
        .map(|i| {
            let t = i as f64 / RATE;
            (-t / 0.1).exp() * (std::f64::consts::TAU * 440.0 * t).sin()
        })
        .collect();
    let frames = trace(&decaying, RATE, 0.0, 0.05);
    for pair in frames.windows(2) {
        assert!(
            pair[1].rms < pair[0].rms,
            "every frame quieter than the last: {pair:?}"
        );
    }
    assert!(frames.last().expect("a last frame").rms < 0.01);
}

#[test]
fn silence_measures_zero_and_an_empty_window_measures_nothing() {
    assert_eq!(rms(&[0.0; 128]), 0.0);
    assert_eq!(rms(&[]), 0.0, "no samples is no level, not a division");
    let quiet = trace(&vec![0.0; 4410], RATE, 0.0, 0.05);
    assert!(quiet.iter().all(|f| f.rms == 0.0 && f.peak == 0.0));
}
