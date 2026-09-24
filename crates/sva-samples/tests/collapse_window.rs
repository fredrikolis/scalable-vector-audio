// Concern: states that a spectrum or series under any crop costs its unwindowed route and answers the same samples | Non-concern: which row a windowed form takes | IO: (a ClosedForm) -> Buffer

use sva_formula::{Body, ClosedForm, Edge, Origin, Part, Var, noise};
use sva_samples::collapse::{self, AliasScore, Horizon};
use sva_samples::{Buffer, CollapseError, Label, PSYCHOACOUSTIC_V1, Profile, Source};

const RATE: u32 = 48_000;

/// Every row here is asserted with its alias score measured, as a reading that reads one asks.
fn render(
    form: &ClosedForm,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
) -> Result<(Buffer, Label), CollapseError> {
    collapse::render(form, rate, horizon, profile, AliasScore::Asked)
}

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

/// A window does not stop a spectrum's lines being placed; the sweep it replaced took over
/// two minutes on this closed form.
#[test]
fn a_cropped_noise_series_costs_its_uncropped_route() {
    let horizon = Horizon::secs(0.0, 4.0);
    let len = horizon.len(RATE).expect("a horizon");
    let law = form(cropped(hiss(), 4.0));
    let sum = sva_formula::normalize_closed_form(&law).expect("a spectral sum");
    let truncated = sva_samples::truncate_spectral_sum(
        &sum,
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let swept = truncated
        .lanes
        .iter()
        .map(|lane| lane.atoms.len())
        .sum::<usize>() as u128
        * len as u128;
    let cost = collapse::plan::of(&sum, RATE, horizon, &PSYCHOACOUSTIC_V1, len)
        .expect("a row")
        .flops(len);
    assert!(
        cost < swept,
        "a cropped series costs its placed route, not {swept} for a sweep: {cost}"
    );

    let (held, label) =
        render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("a cropped noise series");
    assert_eq!(label.source, Source::Measured, "a window is measured");
    assert_eq!(held.len(), len);
}

#[test]
fn a_cropped_series_is_its_own_law_inside_the_window() {
    let law = form(cropped(hiss(), 2.0));
    let sum = sva_samples::truncate_spectral_sum(
        &sva_formula::normalize_closed_form(&law).expect("a spectral sum"),
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let (held, _) = render(&law, RATE, Horizon::secs(0.0, 4.0), &PSYCHOACOUSTIC_V1)
        .expect("a cropped noise series");

    for i in [0usize, 1, 4_001, 95_999] {
        let want = sva_samples::eval_spectral_sum_at(&sum, 0, i as f64 / f64::from(RATE))
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

/// A line the grid cannot hold is one more group, never a reason to sweep the series it is
/// added to term by term: its own group sums directly and every other group stays placed.
#[test]
fn a_windowed_series_plus_a_line_keeps_its_route() {
    let horizon = Horizon::secs(0.0, 4.0);
    let len = horizon.len(RATE).expect("a horizon");
    let counted = |body: Body| {
        let sum = sva_formula::normalize_closed_form(&form(body)).expect("a spectral sum");
        sva_samples::collapse::plan::of(&sum, RATE, horizon, &PSYCHOACOUSTIC_V1, len)
            .expect("a row")
            .flops(len)
    };
    let sum = || {
        Body::Add(vec![
            Part::bare(cropped(hiss(), 4.0)),
            Part::bare(sine(440.37)),
        ])
    };
    let alone = counted(cropped(hiss(), 4.0));
    let beside = counted(sum());
    assert!(
        beside <= alone + 8 * len as u128,
        "one line costs one line: {beside} against {alone} over {len} samples"
    );

    let law = form(sum());
    let sum = sva_samples::truncate_spectral_sum(
        &sva_formula::normalize_closed_form(&law).expect("a spectral sum"),
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let (held, _) = render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("the sum");
    for i in [0usize, 1, 4_001, 95_999] {
        let want = sva_samples::eval_spectral_sum_at(&sum, 0, i as f64 / f64::from(RATE))
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

/// A shouldered crop is a window like any other: a series under one places its lines rather than
/// refusing or sweeping term by term, and the ramp shapes the samples it placed.
#[test]
fn a_shouldered_noise_series_places_by_its_route() {
    let horizon = Horizon::secs(0.0, 4.0);
    let len = horizon.len(RATE).expect("a horizon");
    let law = form(shouldered(hiss(), 4.0, 1.0));
    let sum = sva_formula::normalize_closed_form(&law)
        .expect("a shouldered crop over a series has a form");
    let truncated = sva_samples::truncate_spectral_sum(
        &sum,
        sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE),
    )
    .expect("a truncated form");
    let swept = truncated
        .lanes
        .iter()
        .map(|lane| lane.atoms.len())
        .sum::<usize>() as u128
        * len as u128;
    let cost = sva_samples::collapse::plan::of(&sum, RATE, horizon, &PSYCHOACOUSTIC_V1, len)
        .expect("a row")
        .flops(len);
    assert!(
        cost < swept,
        "a shouldered series costs its placed route, not {swept} for a sweep: {cost}"
    );

    let (held, label) =
        render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("a shouldered series");
    assert_eq!(label.source, Source::Measured, "a window is measured");
    for i in [0usize, 1, 24_000, 96_000, 168_000] {
        let want = sva_samples::eval_spectral_sum_at(&truncated, 0, i as f64 / f64::from(RATE))
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

/// A decaying lane is swept, and a swept lane is read where its own indicator is 1: the plan
/// counts the window's samples, not the horizon's.
#[test]
fn a_swept_lane_is_read_only_inside_the_window_it_carries() {
    let horizon = Horizon::secs(0.0, 4.0);
    let len = horizon.len(RATE).expect("a horizon");
    let law = form(cropped(
        Body::Mul(vec![Part::bare(decaying(3.0)), Part::bare(sine(440.0))]),
        1.0,
    ));
    let sum = sva_formula::normalize_closed_form(&law).expect("a spectral sum");
    let plan = sva_samples::collapse::plan::of(&sum, RATE, horizon, &PSYCHOACOUSTIC_V1, len)
        .expect("a row");
    let sva_samples::collapse::plan::Plan::Sampled(held) = &plan else {
        panic!("a windowed decay is sampled, not {:?}", plan.rule());
    };
    let [sva_samples::collapse::plan::LanePlan::Sweep { samples, .. }] = held.lanes[..] else {
        panic!("one swept lane, got {}", held.lanes.len());
    };
    assert_eq!(samples, len / 4, "one second of four");

    let (held, _) = render(&law, RATE, horizon, &PSYCHOACOUSTIC_V1).expect("a windowed decay");
    assert!(held.at(0, 100) != 0.0, "inside the window");
    assert_eq!(held.at(0, len - 1), 0.0, "outside it");
}
