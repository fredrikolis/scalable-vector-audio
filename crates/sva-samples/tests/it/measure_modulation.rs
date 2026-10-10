// Concern: what modulation owes tremolo, a static tone, a swept saw, silence, refusals | Non-concern: the band filters | IO: (samples) -> asserts

use std::f64::consts::TAU;

use sva_samples::measure::modulation::{Group, Modulation, Track, measure};
use sva_samples::measure::sliding::Unread;

const RATE: f64 = 16_000.0;

fn tone(secs: f64, at: impl Fn(f64) -> f64) -> Vec<f64> {
    (0..(secs * RATE) as usize)
        .map(|i| at(i as f64 / RATE))
        .collect()
}

fn read(samples: &[f64], bpm: Option<f64>) -> Modulation {
    measure(samples, RATE, 0.0, 0.5, 4.0, bpm).expect("a 4 s window over a longer target")
}

fn track(m: &Modulation, group: Group) -> &Track {
    m.tracks
        .iter()
        .find(|t| t.group == group)
        .expect("every group")
}

/// A 1 kHz tone at 1 + 0.5 sin(2 pi 4 t) leads every `mid` window clear of its opening at
/// 4 Hz, its depth the index 0.5, the beat at 120 bpm half of one; its centroid holds still.
#[test]
fn tremolo_at_4_hz_reads_4_hz_at_its_depth() {
    let m = read(
        &tone(8.0, |t| {
            (1.0 + 0.5 * (TAU * 4.0 * t).sin()) * (TAU * 1000.0 * t).sin()
        }),
        Some(120.0),
    );
    let mid = track(&m, Group::Mid);
    let clear: Vec<_> = mid.windows.iter().filter(|w| w.from_secs >= 0.5).collect();
    assert!(!clear.is_empty());
    for w in clear {
        let lead = w.peaks[0];
        assert!((lead.hz - 4.0).abs() < 1e-3, "{w:?}");
        assert!((lead.depth - 0.5).abs() < 1e-3, "{w:?}");
        assert!(
            (lead.period_beats.expect("a bpm") - 0.5).abs() < 1e-3,
            "{w:?}"
        );
        let centroid = w.centroid.as_ref().expect("energy throughout");
        let swing = centroid.peaks.first().map_or(0.0, |p| p.depth);
        assert!(swing < 1.0, "{w:?}");
    }
}

/// A steady 1 kHz tone moves at no rate: every peak clear of its opening is rounding.
#[test]
fn a_static_tone_reads_no_depth_worth_the_name() {
    let m = read(&tone(8.0, |t| (TAU * 1000.0 * t).sin()), None);
    for t in &m.tracks {
        for w in t.windows.iter().filter(|w| w.from_secs >= 0.5) {
            assert!(
                w.peaks.iter().all(|p| p.depth < 1e-9),
                "{}: {w:?}",
                t.group.key()
            );
            assert!(w.peaks.iter().all(|p| p.period_beats.is_none()));
        }
    }
}

/// A 55 Hz saw under a lowpass sweeping 200 Hz to 2 kHz three times a second: `mid` leads at
/// the sweep's 3 Hz, and its centroid swings at 3 Hz by hundreds of Hz.
#[test]
fn a_filter_swept_saw_reads_its_lfo_in_the_mid_band() {
    let m = read(
        &tone(8.0, |t| {
            let cutoff = 200.0 * 10f64.powf(0.5 * (1.0 + (TAU * 3.0 * t).sin()));
            (1..=140)
                .map(|n| {
                    let hz = 55.0 * n as f64;
                    let gain = 1.0 / (1.0 + (hz / cutoff).powi(4)).sqrt();
                    gain * (TAU * hz * t).sin() / n as f64
                })
                .sum::<f64>()
                * 0.3
        }),
        None,
    );
    let mid = track(&m, Group::Mid);
    for w in mid.windows.iter().filter(|w| w.from_secs >= 0.5) {
        assert!((w.peaks[0].hz - 3.0).abs() < 1e-3, "{w:?}");
        let swing = w.centroid.as_ref().expect("energy throughout").peaks[0];
        assert!((swing.hz - 3.0).abs() < 1e-3, "{w:?}");
        assert!(swing.depth > 100.0, "{w:?}");
    }
}

/// Silence holds no level, so no peak and no centroid.
#[test]
fn silence_reads_no_peak_and_no_centroid() {
    let m = read(&vec![0.0; (5.0 * RATE) as usize], None);
    for t in &m.tracks {
        assert_eq!(t.summary.windows_with_peak, 0);
        for w in &t.windows {
            assert_eq!(w.level, 0.0);
            assert!(w.peaks.is_empty() && w.centroid.is_none());
        }
    }
}

#[test]
fn a_window_under_two_periods_and_a_target_under_the_window_refuse() {
    let samples = vec![0.0; (3.0 * RATE) as usize];
    assert_eq!(
        measure(&samples, RATE, 0.0, 0.5, 3.0, None),
        Err(Unread::WindowUnderTwoPeriods {
            window_secs: 3.0,
            least_secs: 4.0
        })
    );
    assert_eq!(
        measure(&samples, RATE, 0.0, 0.5, 4.0, None),
        Err(Unread::TargetShorterThanWindow {
            held_secs: 3.0,
            window_secs: 4.0,
            least_secs: 4.0
        })
    );
}
