// Concern: states that a series or spectrum under a crop answers its own law inside the window, nothing outside it | Non-concern: which row a windowed form takes | IO: (a ClosedForm) -> Buffer

use crate::helpers::render;
use sva_formula::{Body, ClosedForm, Edge, Origin, Part, Var, noise};
use sva_samples::{Extent, PSYCHOACOUSTIC_V1, Rows, Source};

const RATE: u32 = 48_000;

fn hiss() -> Body {
    Body::Series(Box::new(noise(1, 0.5, 0.0)))
}

fn form(body: Body) -> ClosedForm {
    ClosedForm {
        var: Var::T,
        body,
        origin: Origin::new(0),
    }
}

fn cropped(of: Body, to_secs: f64) -> Body {
    Body::Crop {
        of: Part::bare(of),
        l: Edge::at(0.0),
        r: Edge::at(to_secs),
        rise: 0.0,
        fall: 0.0,
    }
}

#[test]
fn a_cropped_series_is_its_own_law_inside_the_window() {
    let law = form(cropped(hiss(), 2.0));
    let sum = sva_samples::truncate_spectral_sum(
        &sva_formula::normalize_closed_form(&law).expect("a spectral sum"),
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let (held, _) =
        render(&law, RATE, (0.0, 4.0), &PSYCHOACOUSTIC_V1).expect("a cropped noise series");

    for i in [0usize, 1, 4_001, 95_999] {
        let want = sva_samples::eval_spectral_sum_at(&sum, 0, at(i))
            .expect("a value")
            .re;
        assert!(
            (held.at(0, i) - want).abs() <= 1e-9 * want.abs().max(1.0),
            "sample {i} inside the window: {} against {want}",
            held.at(0, i)
        );
    }
}

fn sine(hz: f64) -> Body {
    Body::Apply(
        sva_formula::Unary::Sin,
        Part::bare(Body::Mul(vec![
            Part::bare(Body::Const(sva_formula::C64::real(
                std::f64::consts::TAU * hz,
            ))),
            Part::bare(Body::Line),
        ])),
    )
}

/// A line beside a windowed series is one more group: each answers its own law.
#[test]
fn a_windowed_series_plus_a_line_is_its_own_law() {
    let horizon = (0.0, 4.0);
    let law = form(Body::Add(vec![
        Part::bare(cropped(hiss(), 4.0)),
        Part::bare(sine(440.37)),
    ]));
    let sum = sva_samples::truncate_spectral_sum(
        &sva_formula::normalize_closed_form(&law).expect("a spectral sum"),
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let (held, _) = render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("the sum");
    for i in [0usize, 1, 4_001, 95_999] {
        let want = sva_samples::eval_spectral_sum_at(&sum, 0, at(i))
            .expect("a value")
            .re;
        assert!(
            (held.at(0, i) - want).abs() <= 1e-9 * want.abs().max(1.0),
            "sample {i}: {} against {want}",
            held.at(0, i)
        );
    }
}

fn shouldered(of: Body, to_secs: f64, ramp: f64) -> Body {
    Body::Crop {
        of: Part::bare(of),
        l: Edge::at(0.0),
        r: Edge::at(to_secs),
        rise: ramp,
        fall: ramp,
    }
}

/// A shouldered crop is a window like any other: a series under one is read rather than
/// refused, and the ramp shapes its samples.
#[test]
fn a_shouldered_noise_series_is_shaped_by_its_ramp() {
    let horizon = (0.0, 4.0);
    let law = form(shouldered(hiss(), 4.0, 1.0));
    let sum = sva_formula::normalize_closed_form(&law)
        .expect("a shouldered crop over a series has a form");
    let truncated = sva_samples::truncate_spectral_sum(
        &sum,
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let (held, label) =
        render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("a shouldered series");
    assert_eq!(label.source, Source::Measured, "a window is measured");
    for i in [0usize, 1, 24_000, 96_000, 168_000] {
        let want = sva_samples::eval_spectral_sum_at(&truncated, 0, at(i))
            .expect("a value")
            .re;
        assert!(
            (held.at(0, i) - want).abs() <= 1e-9 * want.abs().max(1.0),
            "sample {i} under the ramp: {} against {want}",
            held.at(0, i)
        );
    }
    let level = |from: usize, to: usize| {
        (from..to).map(|i| held.at(0, i).abs()).sum::<f64>() / (to - from) as f64
    };
    assert!(
        level(0, RATE as usize / 4) < level(RATE as usize * 2, RATE as usize * 2 + 12_000) / 4.0,
        "the ramp is on the samples, not only in the label"
    );
}

fn decaying(rate_per_sec: f64) -> Body {
    Body::Apply(
        sva_formula::Unary::Exp,
        Part::bare(Body::Mul(vec![
            Part::bare(Body::Const(sva_formula::C64::real(-rate_per_sec))),
            Part::bare(Body::Line),
        ])),
    )
}

/// A decaying lane is swept, and a swept lane is read where its own indicator is 1: past the
/// window it costs nothing and writes zero.
#[test]
fn a_swept_lane_is_read_only_inside_the_window_it_carries() {
    let horizon = (0.0, 4.0);
    let extent = Extent::secs(RATE, horizon.0, horizon.1);
    let len = extent.len();
    let law = form(cropped(
        Body::Mul(vec![Part::bare(decaying(3.0)), Part::bare(sine(440.0))]),
        1.0,
    ));
    let rows = Rows::of(&law, sva_samples::Grid::of(RATE), &PSYCHOACOUSTIC_V1).expect("rows");
    let quarter = len as i64 / 4;
    assert!(rows.work(0, quarter).0 > 0, "the window is read");
    assert_eq!(rows.work(quarter, len as i64).0, 0, "past it nothing is");

    let (held, _) = render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("a windowed decay");
    assert!(held.at(0, 100) != 0.0, "inside the window");
    assert_eq!(held.at(0, len - 1), 0.0, "outside it");
}

/// Sample `i`'s instant at `RATE`, and the sample it is.
fn at(i: usize) -> sva_samples::At {
    sva_samples::At::Sample(sva_samples::Grid::of(RATE), i as i64)
}
