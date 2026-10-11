// Concern: what inharmonicity owes a harmonic series, a stiff string, a lone tone and noise, exact and measured | Non-concern: closed forms | IO: (partials or samples) -> asserts

use crate::helpers::partials::stiff_partial;
use sva_samples::measure::harmony::inharmonicity::{InharmonicFrame, measure};
use sva_samples::measure::harmony::{Framing, Moment, Partial, measured};

const RATE: f64 = 44_100.0;

const EXACT: Framing = Framing {
    hop_secs: 0.05,
    window: None,
};

fn exact(hz: f64, amplitude: f64) -> Partial {
    Partial {
        hz,
        amplitude,
        hz_bounds: (hz, hz),
        amplitude_bounds: (amplitude, amplitude),
    }
}

fn frame_of(partials: Vec<Partial>) -> InharmonicFrame {
    let moment = Moment {
        t_secs: 0.0,
        partials,
    };
    measure(&[moment], EXACT, -40.0).frames.remove(0)
}

fn stiff(b: f64) -> Vec<(f64, f64)> {
    (1..=8)
        .map(|k| (stiff_partial(110.0, b, k), 0.4 / f64::from(k)))
        .collect()
}

fn render(partials: &[(f64, f64)], secs: f64) -> Vec<f64> {
    (0..(RATE * secs) as usize)
        .map(|n| {
            let t = n as f64 / RATE;
            partials
                .iter()
                .map(|(f, a)| a * (std::f64::consts::TAU * f * t).cos())
                .sum()
        })
        .collect()
}

fn noise(n: usize) -> Vec<f64> {
    let mut s: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        })
        .collect()
}

#[test]
fn whole_multiples_are_one_series_at_zero_cents_and_fit_no_stiffness() {
    let read = frame_of(
        (1..=8)
            .map(|n| exact(220.0 * f64::from(n), 0.5 / f64::from(n)))
            .collect(),
    );
    let [series] = &read.series[..] else {
        panic!("one series: {read:?}")
    };
    let ns: Vec<u32> = series.members.iter().map(|m| m.n).collect();
    assert_eq!(ns, (1..=8).collect::<Vec<u32>>());
    assert!(
        series.members.iter().all(|m| m.cents.abs() < 1e-12),
        "{series:?}"
    );
    assert!(read.cents.expect("members past the root").value < 1e-12);
    let fit = series.fit.expect("eight distinct n");
    assert!(
        fit.b.abs() < 1e-15 && (fit.f0_hz - 220.0).abs() < 1e-9,
        "{fit:?}"
    );
    let unexplained = read.unexplained.expect("partials");
    assert_eq!((unexplained.value, unexplained.bounds), (0.0, (0.0, 0.0)));
}

/// Fletcher's string law read back: n f0 sqrt(1 + B n^2) fits its own B and f0 exactly, and
/// each member stands sharp of n f1 by 1200 log2(n sqrt(1 + B n^2) / (n sqrt(1 + B))).
#[test]
fn a_stiff_strings_partials_fit_their_own_b_and_stand_sharp_by_the_law() {
    let b = 2e-4;
    let read = frame_of(stiff(b).iter().map(|(f, a)| exact(*f, *a)).collect());
    let [series] = &read.series[..] else {
        panic!("one series: {read:?}")
    };
    assert_eq!(series.members.len(), 8);
    for m in &series.members {
        let n = f64::from(m.n);
        let law = 1200.0 * ((1.0 + b * n * n) / (1.0 + b)).sqrt().log2();
        assert!(
            (m.cents - law).abs() < 1e-9,
            "n {}: {} vs {law}",
            m.n,
            m.cents
        );
    }
    let fit = series.fit.expect("eight distinct n");
    assert!((fit.b - b).abs() < 1e-9 * b, "{fit:?}");
    assert!((fit.f0_hz - 110.0).abs() < 1e-9, "{fit:?}");
    assert!(read.cents.expect("members").value > 3.0, "{read:?}");
}

#[test]
fn a_lone_tone_has_no_deviation_no_fit_and_is_wholly_unexplained() {
    let read = frame_of(vec![exact(440.0, 0.5)]);
    assert_eq!(read.cents, None);
    assert_eq!(read.series[0].fit, None);
    assert_eq!(read.unexplained.expect("a partial").value, 1.0);
    let silent = frame_of(vec![exact(440.0, 0.001)]);
    assert_eq!((silent.unexplained, silent.series.len()), (None, 0));
}

/// The stiff string sampled and measured off Hann frames, where each frame lies wholly inside
/// the samples: its frame cents agree with the exact partials' within 0.15 cents and its B
/// within 1.5%, and the bounds every frame reports hold both.
#[test]
fn a_measured_stiff_string_agrees_with_its_exact_partials_inside_its_bounds() {
    let b = 2e-4;
    let truth = frame_of(stiff(b).iter().map(|(f, a)| exact(*f, *a)).collect());
    let cents = truth.cents.expect("members").value;
    let (moments, framing) = measured::moments(&render(&stiff(b), 0.5), RATE, 0.0, 0.05);
    let read = measure(&moments, framing, -40.0);
    assert_eq!(read.frames.len(), 10);
    for frame in &read.frames[1..9] {
        let [series] = &frame.series[..] else {
            panic!("one series: {frame:?}")
        };
        let found = frame.cents.expect("members");
        assert!((found.value - cents).abs() < 0.15, "{found:?} vs {cents}");
        assert!(
            found.bounds.0 <= cents && cents <= found.bounds.1,
            "{found:?}"
        );
        let fit = series.fit.expect("eight distinct n");
        let (lo, hi) = fit.b_bounds.expect("a positive f0^2 over the box");
        assert!((fit.b - b).abs() < 0.015 * b, "{fit:?}");
        assert!(lo <= b && b <= hi, "{b} in [{lo}, {hi}]");
        assert_eq!(frame.unexplained.expect("partials").value, 0.0);
    }
}

/// Noise holds no harmonic series, yet its many peaks fall within the grouping's tolerance by
/// chance: each frame reads tens of series and some energy in none, where the same frames of
/// a saw read one series and none.
#[test]
fn noise_reads_many_series_and_some_energy_in_none_and_a_saw_one_and_none() {
    let (moments, framing) = measured::moments(&noise(22_050), RATE, 0.0, 0.05);
    let read = measure(&moments, framing, -40.0);
    for frame in &read.frames[1..] {
        assert!(frame.series.len() > 20, "{}", frame.series.len());
        assert!(
            frame.unexplained.expect("partials").value > 0.0,
            "{frame:?}"
        );
    }
    let saw: Vec<(f64, f64)> = (1..=8)
        .map(|n| (220.0 * f64::from(n), 0.5 / f64::from(n)))
        .collect();
    let (moments, framing) = measured::moments(&render(&saw, 0.5), RATE, 0.0, 0.05);
    let read = measure(&moments, framing, -40.0);
    for frame in &read.frames[1..9] {
        assert_eq!(frame.series.len(), 1, "{frame:?}");
        assert_eq!(frame.unexplained.expect("partials").value, 0.0);
        assert!(frame.cents.expect("members").value < 0.05, "{frame:?}");
    }
    assert!(read.unexplained_mean.expect("frames") < 1e-12);
}

#[test]
fn the_curve_reports_its_means_and_where_its_cents_peak() {
    let moments: Vec<Moment> = [0.0, 2e-4, 1e-3]
        .iter()
        .enumerate()
        .map(|(k, b)| Moment {
            t_secs: k as f64 * 0.05,
            partials: (1..=4)
                .map(|n| exact(stiff_partial(110.0, *b, n), 0.4 / f64::from(n)))
                .collect(),
        })
        .collect();
    let read = measure(&moments, EXACT, -40.0);
    let values: Vec<f64> = read
        .frames
        .iter()
        .map(|f| f.cents.expect("members").value)
        .collect();
    assert!(values[0] < values[1] && values[1] < values[2], "{values:?}");
    let max = read.cents_max.expect("frames");
    assert_eq!((max.t_secs, max.value), (0.1, values[2]));
    let mean = read.cents_mean.expect("frames");
    assert!((mean - values.iter().sum::<f64>() / 3.0).abs() < 1e-12);
}
