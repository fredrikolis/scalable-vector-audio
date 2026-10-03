// Concern: proves a series is a value, truncated once against a ceiling | Non-concern: summing its lines into a buffer (sva-samples) | IO: (a Series) -> the lines it yields

use crate::fixtures::{DUAL, Fixed, constant, cosine, part, saw_series, sine, term};
use sva_formula::{
    Body, Bound, Code, IndexId, Series, Unary, Var, commensurate, infer, lines, noise,
    normalize_closed_form,
};

fn enumerate(s: &Series, ceiling: f64) -> Option<sva_formula::Lines> {
    lines(s, ceiling, -20.0, 2f64.powi(-24))
}

fn series_of(f: &Body) -> Series {
    let mut sum = normalize_closed_form(&term(Var::T, f.clone())).expect("a series value");
    sum.lanes.remove(0).series.remove(0)
}

#[test]
fn a_sum_to_infinity_is_a_series_value() {
    let sum = normalize_closed_form(&term(Var::T, saw_series(220.0))).unwrap();
    let lane = &sum.lanes[0];
    assert!(lane.atoms.is_empty(), "a series is never unrolled");
    assert_eq!(lane.series.len(), 1);
    assert_eq!(lane.series[0].hi, Bound::Infinite);
    assert_eq!(lane.series[0].lo, 1);
}

#[test]
fn a_saw_has_a_dual_and_a_geometric_growth_does_not() {
    assert!(
        infer(
            &term(Var::T, saw_series(220.0)),
            &Fixed::holding(DUAL, false)
        )
        .unwrap()
        .has_dual()
    );

    let k = IndexId(0);
    let geometric = Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Mul(vec![
            part(Body::Apply(
                Unary::Exp,
                part(Body::Mul(vec![
                    part(Body::Index(k)),
                    part(constant(std::f64::consts::LN_2)),
                ])),
            )),
            part(sine(220.0)),
        ])),
    }));
    assert_eq!(
        infer(&term(Var::T, geometric), &Fixed::holding(DUAL, false))
            .unwrap_err()
            .code,
        Code::SeriesNotSummable
    );
}

#[test]
fn a_series_truncates_against_the_ceiling_and_reports_its_tail() {
    let answer = enumerate(&series_of(&saw_series(220.0)), 22050.0).expect("a saw's lines");
    let highest = answer
        .taken
        .iter()
        .map(|l| l.hz.abs())
        .fold(0.0f64, f64::max);
    assert!(highest < 22050.0, "nothing at or above the ceiling is kept");
    assert!((highest - 220.0 * 100.0).abs() < 1e-6, "{highest}");
    assert!(
        !answer.dropped.is_empty(),
        "the first line out of band is listed"
    );
    assert!(
        answer.tail_db < 0.0,
        "the tail is quieter than what was kept"
    );
}

/// The ceiling is the caller's: a profile reaching past 20 kHz keeps the lines under it.
#[test]
fn a_series_keeps_every_line_under_the_ceiling_it_is_given() {
    let answer = enumerate(&series_of(&saw_series(1_000.0)), 24_000.0).expect("a saw's lines");
    let highest = answer
        .taken
        .iter()
        .map(|l| l.hz.abs())
        .fold(0.0f64, f64::max);
    assert!(
        (highest - 23_000.0).abs() < 1e-6,
        "the 23rd harmonic is under a 24 kHz ceiling: {highest}"
    );
}

/// An offset opposing the slope carries a line back into the band before it leaves, so the
/// walk cannot stop at the first index whose bare slope would clear the ceiling.
#[test]
fn a_frequency_law_that_starts_below_zero_is_walked_to_where_it_leaves_the_band() {
    let k = IndexId(0);
    let ramp = Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Apply(
            Unary::Cos,
            part(Body::Mul(vec![
                part(constant(std::f64::consts::TAU)),
                part(Body::Add(vec![part(Body::Index(k)), part(constant(-50.0))])),
                part(Body::Line),
            ])),
        )),
    };
    let answer = enumerate(&ramp, 100.0).expect("a ramp's lines");
    let highest = answer
        .taken
        .iter()
        .map(|l| l.hz)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((highest - 99.0).abs() < 1e-9, "{highest}");
    assert!(
        answer.dropped.iter().any(|l| l.hz == 100.0),
        "a line on the ceiling is past it"
    );
}

/// `sum(cos(2*pi*440*t) / k^2)` piles every term onto one line, and no ratio bounds `1/k^2`:
/// the walk stops only where everything it drops sums under the floor, and the tail it
/// states is that sum, never one term.
#[test]
fn a_tail_no_ratio_bounds_is_dropped_only_where_its_whole_sum_is_under_the_floor() {
    let k = IndexId(0);
    let series = Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(
            part(cosine(440.0)),
            part(Body::Pow(part(Body::Index(k)), 2)),
        )),
    };
    let answer = enumerate(&series, 20_000.0).expect("a bounded tail");
    let last = answer.taken.len() / 2;
    let kept: f64 = answer.taken.iter().map(|l| l.amp.abs()).sum();
    let dropped = (std::f64::consts::PI.powi(2) / 6.0 - kept) / 2.0;
    let loudest = answer
        .taken
        .iter()
        .map(|l| l.amp.abs())
        .fold(0.0f64, f64::max);
    assert!(
        dropped < loudest * 0.1,
        "the {last} terms kept leave {} dB of tail, over the -20 dB floor",
        20.0 * (dropped / loudest).log10()
    );
    assert!(
        answer.tail_db >= 20.0 * (dropped / loudest).log10(),
        "the stated tail {} dB covers the {} dB dropped",
        answer.tail_db,
        20.0 * (dropped / loudest).log10()
    );
}

/// A tail with no decay any bound proves is not cut on a guess.
#[test]
fn a_tail_no_bound_sums_names_no_lines() {
    let k = IndexId(0);
    let series = Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(part(cosine(440.0)), part(Body::Index(k)))),
    };
    assert_eq!(enumerate(&series, 20_000.0), None);
}

#[test]
fn noise_spacing_is_the_reciprocal_period() {
    let answer = enumerate(&noise(7, 2.0, 0.0), 40.0).expect("noise lines");
    let mut hz: Vec<f64> = answer
        .taken
        .iter()
        .map(|l| l.hz)
        .filter(|h| *h > 0.0)
        .collect();
    hz.sort_by(f64::total_cmp);
    assert!(hz.len() > 4);
    for pair in hz.windows(2) {
        assert!((pair[1] - pair[0] - 0.5).abs() < 1e-9, "{pair:?}");
    }
}

#[test]
fn noise_lines_are_conjugate_symmetric() {
    let answer = enumerate(&noise(7, 2.0, -3.0), 20.0).expect("noise lines");
    for line in &answer.taken {
        let mirror = answer
            .taken
            .iter()
            .find(|other| (other.hz + line.hz).abs() < 1e-9)
            .unwrap_or_else(|| panic!("no mirror for {}", line.hz));
        assert!((mirror.amp - line.amp.conj()).abs() < 1e-12);
    }
}

#[test]
fn periodic_noise_is_commensurate_with_its_own_horizon() {
    for line in enumerate(&noise(3, 0.25, 0.0), 200.0)
        .expect("noise lines")
        .taken
    {
        assert!(
            commensurate(line.hz, 0.25),
            "{} does not close over its own period",
            line.hz
        );
    }
}

#[test]
fn commensurability_is_a_whole_turn_count_and_no_tolerance_widens_it() {
    assert!(commensurate(440.0, 0.25), "110 whole turns");
    assert!(!commensurate(10.03, 1.0), "a third of a turn short");
    assert!(
        !commensurate(440.01, 0.25),
        "a hundredth of a turn short is a quarter of a cent, and still not commensurate"
    );
}
