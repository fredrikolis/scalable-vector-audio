// Concern: what periodicity owes a metronome, a tempo sweep, jittered and rubato trains, a kick under hats, silence, refusals | Non-concern: the band filters | IO: (samples) -> asserts

use sva_samples::measure::periodicity::{Group, Periodicity, Track, Window, measure};
use sva_samples::measure::sliding::Unread;

/// A 2 kHz envelope, so 0.5 s is exactly 1000 lags.
const RATE: f64 = 16_000.0;

fn click(out: &mut [f64], at: usize) {
    for (i, s) in out.iter_mut().skip(at).take(32).enumerate() {
        *s += (1.0 - i as f64 / 32.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
    }
}

fn burst(out: &mut [f64], at: usize, hz: f64, decay_secs: f64, secs: f64) {
    let attack = 0.002 * RATE;
    for (i, s) in out
        .iter_mut()
        .skip(at)
        .take((secs * RATE) as usize)
        .enumerate()
    {
        let t = i as f64 / RATE;
        let rise = (i as f64 / attack).min(1.0);
        *s += rise * (-t / decay_secs).exp() * (std::f64::consts::TAU * hz * t).sin();
    }
}

fn jitter(n: usize, spread_secs: f64) -> Vec<f64> {
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2.0 * spread_secs
        })
        .collect()
}

/// Clicks at each instant `at` returns for beat k, over `secs`.
fn train(secs: f64, at: impl Fn(usize) -> f64) -> Vec<f64> {
    let mut out = vec![0.0; (secs * RATE) as usize];
    for k in 0.. {
        let t = at(k);
        if t >= secs - 0.01 {
            break;
        }
        click(&mut out, (t.max(0.0) * RATE).round() as usize);
    }
    out
}

/// A 2 ms 3 kHz burst every `period_secs`, placed between samples: exactly periodic.
fn pulses(secs: f64, period_secs: f64) -> Vec<f64> {
    (0..(secs * RATE) as usize)
        .map(|i| {
            let t = i as f64 / RATE;
            let since = t - (t / period_secs).floor() * period_secs;
            match since < 0.002 {
                true => (-since / 0.0005).exp() * (std::f64::consts::TAU * 3000.0 * since).sin(),
                false => 0.0,
            }
        })
        .collect()
}

fn within(bounds: Option<(f64, f64)>, x: f64) -> bool {
    bounds.is_some_and(|(lo, hi)| lo <= x && x <= hi)
}

fn read(samples: &[f64]) -> Periodicity {
    measure(samples, RATE, 0.0, 0.5, 4.0, false).expect("a 4 s window over a longer target")
}

fn track(p: &Periodicity, group: Group) -> &Track {
    p.tracks
        .iter()
        .find(|t| t.group == group)
        .expect("every group")
}

fn whole<'a>(p: &Periodicity, t: &'a Track) -> Vec<&'a Window> {
    t.windows
        .iter()
        .filter(|w| w.to_secs - w.from_secs == p.window_secs)
        .collect()
}

fn leads(t: &Track) -> Vec<(f64, f64)> {
    t.windows
        .iter()
        .map(|w| {
            let p = w.peaks.first().expect("a train repeats in every window");
            (p.period_secs, p.height)
        })
        .collect()
}

/// 120 bpm on the envelope's own grid leads every window at 0.5 s, inside the bounds of each
/// whole one, a clipped one bounding nothing, within 2% of the biased ceiling 1 - 0.5/span,
/// and drifts by rounding alone.
#[test]
fn a_metronome_reads_its_exact_bpm_with_a_high_peak_and_no_drift() {
    let p = read(&train(10.0, |k| k as f64 * 0.5));
    assert_eq!(p.envelope_hz, 2000.0);
    let broadband = track(&p, Group::Broadband);
    assert_eq!(broadband.windows.len(), 20);
    for w in &broadband.windows {
        let lead = w.peaks[0];
        match w.to_secs - w.from_secs == p.window_secs {
            true => assert!(within(lead.period_bounds_secs, 0.5), "{w:?}"),
            false => assert!(lead.period_bounds_secs.is_none() && lead.bpm_bounds.is_none()),
        }
        assert!((lead.bpm - 120.0).abs() < 1e-3, "{w:?}");
        let ceiling = 1.0 - 0.5 / (w.to_secs - w.from_secs);
        assert!((lead.height - ceiling).abs() < 0.02 * ceiling, "{w:?}");
    }
    let drift = broadband.summary.drift_bpm.expect("windows with a lead");
    assert!(
        drift.min.value.abs().max(drift.max.value.abs()) < 1e-3,
        "{drift:?}"
    );
}

/// 128 bpm, 937.5 lags, reads 128.00 in every whole window, inside bounds under a lag wide.
#[test]
fn a_steady_128_bpm_train_reads_128_inside_its_bounds() {
    let p = read(&pulses(12.0, 60.0 / 128.0));
    let full = whole(&p, track(&p, Group::Broadband));
    assert!(full.len() > 10);
    for w in full {
        let lead = w.peaks[0];
        assert!((lead.bpm - 128.0).abs() < 0.005, "{w:?}");
        assert!(within(lead.bpm_bounds, 128.0), "{w:?}");
        assert!(within(lead.period_bounds_secs, 60.0 / 128.0), "{w:?}");
        let (lo, hi) = lead.period_bounds_secs.expect("a whole window");
        assert!(hi - lo <= p.lag_step_secs * (1.0 + 1e-9), "{w:?}");
    }
}

/// Tempos 61 to 176 bpm, some a hair off a lag or half lag, lie inside every whole window's bounds.
#[test]
fn the_true_tempo_lies_inside_the_bounds_across_a_sweep() {
    for bpm in [
        61.3, 77.7, 84.05, 90.1, 104.03, 117.2, 133.3, 147.32, 163.6, 175.44,
    ] {
        let period = 60.0 / bpm;
        let p = read(&pulses(12.0, period));
        for w in whole(&p, track(&p, Group::Broadband)) {
            let lead = w.peaks[0];
            assert!(within(lead.period_bounds_secs, period), "{bpm}: {w:?}");
            assert!(within(lead.bpm_bounds, bpm), "{bpm}: {w:?}");
        }
    }
}

/// Each click moved up to 40 ms either way repeats less: every lead stands under the
/// metronome's lowest, and the lead moves between windows.
#[test]
fn a_jittered_train_reads_lower_peaks_and_drifts() {
    let steady = read(&train(10.0, |k| k as f64 * 0.5));
    let floor = leads(track(&steady, Group::Broadband))
        .iter()
        .map(|l| l.1)
        .fold(f64::INFINITY, f64::min);
    let moved = jitter(32, 0.04);
    let loose = read(&train(10.0, |k| k as f64 * 0.5 + moved[k]));
    let broadband = track(&loose, Group::Broadband);
    for (period, height) in leads(broadband) {
        assert!(height < floor, "{period}s at {height} vs {floor}");
    }
    let drift = broadband.summary.drift_bpm.expect("windows with a lead");
    assert!(drift.max.value - drift.min.value > 0.0, "{drift:?}");
}

/// A tempo gliding from 100 to 140 bpm over 16 s leads at periods across that span.
#[test]
fn a_rubato_train_reads_its_tempo_moving() {
    let (from, to, secs) = (100.0f64, 140.0f64, 16.0);
    let rate_at = |t: f64| from + (to - from) * t / secs;
    let mut beats = vec![0.0];
    while *beats.last().expect("one beat") < secs {
        let t = *beats.last().expect("one beat");
        beats.push(t + 60.0 / rate_at(t));
    }
    let p = read(&train(secs, |k| beats[k]));
    let bpm = track(&p, Group::Broadband)
        .summary
        .bpm
        .expect("windows with a lead");
    assert!(bpm.min.value < 108.0 && bpm.max.value > 132.0, "{bpm:?}");
    assert!(bpm.min.t_secs < bpm.max.t_secs, "{bpm:?}");
}

/// A 55 Hz kick on every beat under 5 kHz hats on every eighth: `low` leads at the beat,
/// `rest` at the eighth.
#[test]
fn a_kick_leads_the_low_band_and_hats_the_rest_at_double_rate() {
    let secs = 10.0;
    let mut out = vec![0.0; (secs * RATE) as usize];
    for k in 0..20 {
        burst(&mut out, k * 8000, 55.0, 0.08, 0.3);
    }
    for k in 0..40 {
        burst(&mut out, k * 4000, 5000.0, 0.02, 0.08);
    }
    let p = read(&out);
    let low = track(&p, Group::Low);
    assert!(low.centres_hz.iter().all(|&hz| hz < 150.0) && !low.centres_hz.is_empty());
    for (track, period) in [(low, 0.5), (track(&p, Group::Rest), 0.25)] {
        for w in whole(&p, track) {
            assert!(within(w.peaks[0].period_bounds_secs, period), "{w:?}");
        }
    }
}

/// Silence repeats at no period: no peak, a null curve, no summary.
#[test]
fn silence_reads_no_period() {
    let p = measure(&vec![0.0; (5.0 * RATE) as usize], RATE, 0.0, 0.5, 4.0, true)
        .expect("a 4 s window over 5 s");
    for t in &p.tracks {
        assert_eq!(t.summary.windows_with_peak, 0);
        for w in &t.windows {
            assert!(w.peaks.is_empty());
            assert!(w.curve.as_ref().expect("asked").iter().all(|h| h.is_nan()));
        }
    }
}

#[test]
fn a_window_under_two_periods_and_a_target_under_the_window_refuse() {
    let samples = vec![0.0; (3.0 * RATE) as usize];
    assert_eq!(
        measure(&samples, RATE, 0.0, 0.5, 3.0, false),
        Err(Unread::WindowUnderTwoPeriods {
            window_secs: 3.0,
            least_secs: 4.0
        })
    );
    assert_eq!(
        measure(&samples, RATE, 0.0, 0.5, 4.0, false),
        Err(Unread::TargetShorterThanWindow {
            held_secs: 3.0,
            window_secs: 4.0,
            least_secs: 4.0
        })
    );
}
