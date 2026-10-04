// Concern: states which row of the collapse table each closed form shape takes, and what its label says | Non-concern: the spectral sum under it (sva-formula) | IO: (a ClosedForm) -> Buffer and Label

use crate::helpers::{part, render, whole_second};
use std::f64::consts::{PI, TAU};

use sva_formula::{Body, Bound, C64, ClosedForm, Edge, IndexId, Origin, Part, Series, Unary, Var};
use sva_samples::{CollapseError, Detail, PSYCHOACOUSTIC_V1, Rows, Rule, Source};

const RATE: u32 = 8_192;

/// The terms a geometric series keeps, from a first term bound and a ratio: up to the first
/// whose whole tail stays within half the profile's least significant bit.
fn kept(first: f64, g: f64) -> i32 {
    (0..)
        .find(|n| first * g.powi(*n) / (1.0 - g) <= PSYCHOACOUSTIC_V1.half_lsb())
        .expect("a tail under the precision")
        .max(1)
}

fn form(var: Var, body: Body) -> ClosedForm {
    ClosedForm {
        var,
        body,
        origin: Origin::new(0),
    }
}

fn constant(x: f64) -> Body {
    Body::Const(C64::real(x))
}

/// `cos(2*pi*hz*t)`, the shape every line atom is written from.
fn cosine(hz: f64, amp: f64) -> Body {
    Body::Mul(vec![
        part(constant(amp)),
        part(Body::Apply(
            Unary::Cos,
            part(Body::Mul(vec![part(constant(TAU * hz)), part(Body::Line)])),
        )),
    ])
}

fn sum(parts: Vec<Body>) -> Body {
    Body::Add(parts.into_iter().map(part).collect())
}

/// A sinc tail thins as `1/d^2`, so past a ceiling `d` hertz off one line it integrates to
/// `1/(2*pi^2*d)`, and against the window's own energy that is one more factor of `width`.
fn sinc_tail_db(hz: f64, width: f64) -> f64 {
    let ceiling = f64::from(RATE) / 2.0;
    let spread = 1.0 / (ceiling - hz) + 1.0 / (ceiling + hz);
    10.0 * (spread / (2.0 * PI * PI * width)).log10()
}

#[test]
fn a_line_spectrum_collapse_is_exact_and_lists_dropped() {
    let law = form(
        Var::T,
        sum(vec![
            cosine(256.0, 1.0),
            cosine(1024.0, 0.5),
            cosine(5000.0, 0.25),
        ]),
    );
    let (buffer, label) = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("lines");

    assert_eq!(label.source, Source::Exact);
    assert_eq!(buffer.len(), RATE as usize);
    let Detail::Lines {
        terms,
        dropped,
        dropped_more,
        ..
    } = &label.detail
    else {
        panic!("expected a line label, got {:?}", label.detail);
    };
    assert_eq!(
        *terms,
        Some(2),
        "two kept frequencies, each a conjugate pair folded"
    );
    assert_eq!(*dropped_more, 0);
    assert_eq!(dropped.len(), 1, "5000 Hz is above 8192/2");
    assert!((dropped[0].hz - 5000.0).abs() < 1e-6);
    assert!((dropped[0].db - 20.0 * 0.25f64.log10()).abs() < 1e-9);

    for i in [0usize, 1, 37, 4096, 8191] {
        let t = i as f64 / f64::from(RATE);
        let want = (TAU * 256.0 * t).cos() + 0.5 * (TAU * 1024.0 * t).cos();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-10,
            "sample {i}: {} vs {want}",
            buffer.at(0, i)
        );
    }
}

/// `sum(k, 1, 200000, sin(2*pi*0.1*k*t)/k)`: 0.1 Hz spacing reaches the 20 kHz ceiling at
/// k = 200000. The reference is the sum itself, read at fifty instants rather than at every
/// sample of the run.
#[test]
fn dense_series_and_direct_sum_agree() {
    let k = IndexId(0);
    let law = form(
        Var::T,
        Body::Series(Box::new(Series {
            index: k,
            lo: 1,
            hi: Bound::Finite(200_000),
            term: part(Body::Div(
                part(Body::Apply(
                    Unary::Sin,
                    part(Body::Mul(vec![
                        part(constant(TAU * 0.1)),
                        part(Body::Index(k)),
                        part(Body::Line),
                    ])),
                )),
                part(Body::Index(k)),
            )),
        })),
    );
    let (rate, len) = (44_100u32, 441_000);
    let rows = Rows::of(&law, rate, &PSYCHOACOUSTIC_V1).expect("a dense series");
    let label = rows.label(&PSYCHOACOUSTIC_V1);
    assert_eq!(label.rule(), Rule::LineSpectrumSummed);
    let Detail::Lines {
        summed, dropped, ..
    } = &label.detail
    else {
        panic!("expected a line label");
    };
    assert_eq!(*summed, 199_999, "every line under the 20 kHz ceiling");
    assert_eq!(dropped.len(), 1, "k = 200000 sits exactly on the ceiling");

    let mut worst = 0.0f64;
    let mut seed = 1u64;
    for _ in 0..50 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let i = (seed >> 33) as i64 % len;
        let held = rows.planes(i, i + 1).expect("a sample")[0][0];
        let t = i as f64 / f64::from(rate);
        let direct: f64 = (1..=199_999u32)
            .map(|n| (TAU * 0.1 * f64::from(n) * t).sin() / f64::from(n))
            .sum();
        worst = worst.max((held - direct).abs());
    }
    assert!(
        worst < 1e-10,
        "the rows and the direct sum differ by {worst}"
    );
}

#[test]
fn a_cropped_pair_is_measured_with_tail_energy() {
    let law = form(
        Var::T,
        Body::Crop {
            of: part(cosine(440.0, 1.0)),
            l: Edge::at(0.1),
            r: Edge::at(0.6),
            rise: 0.0,
            fall: 0.0,
        },
    );
    let (buffer, label) = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a crop");

    assert_eq!(label.source, Source::Measured);
    let Detail::Cropped { rule, tail_db } = label.detail else {
        panic!("expected a cropped label, got {:?}", label.detail);
    };
    assert_eq!(rule, Rule::CroppedPair);
    let tail_db = tail_db.expect("a sinc window states its tail");
    assert!(tail_db.is_finite() && tail_db < 0.0, "tail {tail_db}");
    assert_eq!(buffer.at(0, 0), 0.0, "before the window");
    assert!(buffer.at(0, 2_048) != 0.0, "inside it");
    assert_eq!(buffer.at(0, 8_000), 0.0, "after it");
}

#[test]
fn a_ct_collapse_is_point_sampled() {
    let law = form(Var::T, Body::Apply(Unary::Tanh, part(cosine(1000.0, 6.0))));
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a closed form in t");

    assert_eq!(label.source, Source::Measured);
    assert_eq!(label.rule(), Rule::PointSampled);
    assert!(buffer.plane(0).iter().all(|s| s.abs() <= 1.0));
}

#[test]
fn a_delta_in_ct_refuses() {
    let law = form(
        Var::T,
        Body::Delta {
            at: part(Body::Add(vec![part(Body::Line), part(constant(-0.25))])),
            order: 0,
        },
    );
    let refused = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect_err("a delta");
    assert_eq!(refused.code(), "collapse.singular_in_ct");
}

#[test]
fn an_empty_band_refuses() {
    let law = form(Var::T, sum(vec![cosine(6000.0, 1.0), cosine(7000.0, 0.5)]));
    let refused = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect_err("all gone");
    assert_eq!(refused.code(), "collapse.empty_band");
    assert!(matches!(refused, CollapseError::EmptyBand { .. }));
}

/// The ceiling is what an empty band is about, and under 20 kHz a higher rate raises it.
#[test]
fn empty_band_names_the_ceiling_not_the_window() {
    let law = form(Var::T, sum(vec![cosine(6000.0, 1.0), cosine(7000.0, 0.5)]));
    let refused = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect_err("all gone");
    let CollapseError::EmptyBand {
        ceiling, lowest, ..
    } = refused
    else {
        panic!("expected an empty band, got {refused:?}");
    };
    assert_eq!(ceiling, (RATE / 2) as f64, "the ceiling this rate names");
    assert_eq!(lowest, 6000.0, "the line that came closest to clearing it");

    let said = format!("{refused}: {}", refused.help());
    assert!(said.contains("ceiling"), "{said}");
    assert!(
        said.contains("--rate"),
        "under 20 kHz the ceiling is this rate's own half, and a higher rate raises it: {said}"
    );
}

/// A profile's own ceiling is no rate's half: no rate raises it, so the repair never says to.
#[test]
fn an_empty_band_under_the_profiles_ceiling_names_the_profile() {
    let profile = sva_samples::Profile {
        ceiling_hz: 3_000.0,
        ..PSYCHOACOUSTIC_V1
    };
    let law = form(Var::T, cosine(3_500.0, 1.0));
    let refused = render(&law, RATE, whole_second(), &profile).expect_err("all gone");
    let CollapseError::EmptyBand { ceiling, .. } = refused else {
        panic!("expected an empty band, got {refused:?}");
    };
    assert_eq!(
        ceiling, 3_000.0,
        "the profile's ceiling, under this rate's half"
    );
    assert!(
        !refused.help().contains("--rate"),
        "no rate raises the profile's ceiling: {}",
        refused.help()
    );
}

#[test]
fn every_collapse_carries_source_and_profile() {
    let gaussian = Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(constant(-(PI * 1000.0f64).powi(2))),
            part(Body::Pow(part(Body::Line), 2)),
        ])),
    );
    let laws = [
        ("lines", form(Var::T, cosine(256.0, 1.0))),
        ("gaussian", form(Var::T, gaussian)),
        (
            "cropped",
            form(
                Var::T,
                Body::Crop {
                    of: part(cosine(440.0, 1.0)),
                    l: Edge::at(0.1),
                    r: Edge::at(0.6),
                    rise: 0.0,
                    fall: 0.0,
                },
            ),
        ),
        (
            "nonlinear",
            form(Var::T, Body::Apply(Unary::Tanh, part(cosine(300.0, 3.0)))),
        ),
    ];
    for (name, law) in laws {
        let (_, label) = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(label.profile, "psychoacoustic-v1", "{name}");
        assert_eq!(label.rate, RATE, "{name}");
        assert!(
            matches!(label.source, Source::Exact | Source::Measured),
            "{name}"
        );
    }
}

/// A stereo closed form drops the same line on both sides: the label reports the level one channel
/// carries, not the two of them added, and counts the line once.
#[test]
fn a_two_lane_law_reports_one_dropped_line_at_one_channels_level() {
    let law = form(
        Var::T,
        Body::Join(vec![
            part(sum(vec![cosine(256.0, 1.0), cosine(5000.0, 0.25)])),
            part(sum(vec![cosine(256.0, 1.0), cosine(5000.0, 0.25)])),
        ]),
    );
    let (buffer, label) = render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("lines");
    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.plane(0), buffer.plane(1));

    let Detail::Lines { terms, dropped, .. } = &label.detail else {
        panic!("expected a line label, got {:?}", label.detail);
    };
    assert_eq!(dropped.len(), 1);
    assert!(
        (dropped[0].db - 20.0 * 0.25f64.log10()).abs() < 1e-9,
        "{dropped:?}"
    );
    assert_eq!(*terms, Some(1), "one kept frequency, carried by both lanes");
}

/// A series reaching an instant is truncated once, against the profile's own precision: the
/// terms before its tail rounds away, and no more.
#[test]
fn a_series_under_a_nonlinearity_takes_the_terms_the_precision_leaves() {
    let k = IndexId(1);
    let falling = Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(Body::Index(k)),
            part(constant(0.5f64.ln())),
        ])),
    );
    let series = Body::Series(Box::new(Series {
        index: k,
        lo: 0,
        hi: Bound::Infinite,
        term: part(Body::Mul(vec![part(falling), part(cosine(220.0, 1.0))])),
    }));
    let law = Body::Mul(vec![
        part(series),
        part(Body::Apply(Unary::Tanh, part(Body::Line))),
    ]);
    let (buffer, label) = render(&form(Var::T, law), RATE, (0.0, 0.02), &PSYCHOACOUSTIC_V1)
        .expect("a series has a value once it is truncated");
    assert_eq!(label.source, Source::Measured);
    let taken: f64 = (0..kept(1.0, 0.5)).map(|n| 0.5f64.powi(n)).sum();
    for i in 0..buffer.len() {
        let t = i as f64 / f64::from(RATE);
        let want = taken * (TAU * 220.0 * t).cos() * t.tanh();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// Nothing bounds how many terms of a series an instant needs unless the term carries a
/// coefficient that falls, and a count nothing bounds is a refusal, not a guess.
#[test]
fn a_series_no_coefficient_bounds_refuses_at_a_point() {
    let k = IndexId(1);
    let law = Body::Mul(vec![
        part(Body::Series(Box::new(Series {
            index: k,
            lo: 1,
            hi: Bound::Infinite,
            term: part(Body::Fold(
                sva_formula::Fold::Mod,
                vec![part(Body::Line), part(Body::Index(k))],
            )),
        }))),
        part(Body::Apply(Unary::Tanh, part(Body::Line))),
    ]);
    let refused = render(&form(Var::T, law), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect_err("no coefficient bounds the count");
    assert_eq!(refused.code(), "collapse.not_evaluable");
}

/// `sum(cos(2*pi*440*t) / k)` piles every term onto one line and its tail sums to no bound,
/// so no count of terms is its value: it refuses rather than stop where one term is quiet.
#[test]
fn a_line_series_whose_tail_no_bound_sums_refuses() {
    let k = IndexId(1);
    let law = Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(part(cosine(440.0, 1.0)), part(Body::Index(k)))),
    }));
    let refused = render(&form(Var::T, law), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect_err("no bound sums the tail");
    assert_eq!(refused.code(), "collapse.not_evaluable");
}

/// A series no line enumeration reads is summed term by term, as many terms as its own
/// coefficient keeps before its tail rounds away.
#[test]
fn a_series_no_line_reads_is_counted_off_its_coefficient() {
    let k = IndexId(1);
    let falling = Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(Body::Index(k)),
            part(constant(0.9f64.ln())),
        ])),
    );
    let turning = Body::Apply(Unary::Tanh, part(Body::Line));
    let law = Body::Mul(vec![
        part(Body::Series(Box::new(Series {
            index: k,
            lo: 0,
            hi: Bound::Infinite,
            term: part(Body::Mul(vec![part(falling), part(turning.clone())])),
        }))),
        part(turning),
    ]);
    let (buffer, _) = render(&form(Var::T, law), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect("a falling coefficient counts the terms");
    let counted = kept(1.0, 0.9);
    let taken: f64 = (0..counted).map(|n| 0.9f64.powi(n)).sum();
    for i in 0..buffer.len() {
        let t = i as f64 / f64::from(RATE);
        let want = taken * t.tanh() * t.tanh();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// A series whose terms leave A one by one still has a value at every instant: the six rows
/// hand it to FORMAT 9.1's no-dual row rather than refusing it.
#[test]
fn a_series_of_warped_terms_takes_the_point_row() {
    let k = IndexId(1);
    let falling = Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(Body::Index(k)),
            part(constant(0.5f64.ln())),
        ])),
    );
    let warped = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![
            part(Body::Index(k)),
            part(Body::Apply(Unary::Exp, part(Body::Line))),
        ])),
    );
    let law = Body::Series(Box::new(Series {
        index: k,
        lo: 0,
        hi: Bound::Infinite,
        term: part(Body::Mul(vec![part(falling), part(warped)])),
    }));
    let (buffer, label) = render(&form(Var::T, law), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect("a warped series is point-sampled, not refused");
    assert_eq!(label.source, Source::Measured);
    assert_eq!(label.rule(), Rule::PointSampled);
    let counted = kept(1.0, 0.5);
    for i in 0..buffer.len() {
        let t = i as f64 / f64::from(RATE);
        let want: f64 = (0..counted)
            .map(|n| 0.5f64.powi(n) * (f64::from(n) * t.exp()).sin())
            .sum();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// Nothing bounds what a series reading a node leaves: it refuses rather than keep a floor.
#[test]
fn a_series_reading_a_node_no_bound_holds_refuses() {
    let k = IndexId(1);
    let turning = Body::Apply(
        Unary::Cos,
        part(Body::Mul(vec![
            part(Body::Node(sva_formula::NodeId(0))),
            part(Body::Line),
        ])),
    );
    let series = Body::Series(Box::new(Series {
        index: k,
        lo: 0,
        hi: Bound::Infinite,
        term: part(Body::Mul(vec![
            part(Body::Apply(
                Unary::Exp,
                part(Body::Mul(vec![
                    part(Body::Index(k)),
                    part(constant(0.5f64.ln())),
                ])),
            )),
            part(turning),
        ])),
    }));
    let band = sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE);
    let refused = sva_samples::truncate_written(&series, band).expect_err("no bound");
    assert_eq!(refused.code(), "collapse.not_evaluable");
}

/// `sum(sin(k*(2*pi*220*t + 10*sin(2*pi*5*t)))/k)`, a saw whose pitch moves between 170 and
/// 270 Hz, sums at each instant every harmonic under the ceiling there and no other; the
/// loudest it may drop is the first the fastest pitch carries past the ceiling.
#[test]
fn a_saw_whose_pitch_moves_sums_every_harmonic_under_the_ceiling_at_each_instant() {
    let k = IndexId(1);
    let wobble = Body::Mul(vec![
        part(constant(10.0)),
        part(Body::Apply(
            Unary::Sin,
            part(Body::Mul(vec![part(constant(TAU * 5.0)), part(Body::Line)])),
        )),
    ]);
    let phase = Body::Add(vec![
        part(Body::Mul(vec![
            part(constant(TAU * 220.0)),
            part(Body::Line),
        ])),
        part(wobble),
    ]);
    let carrier = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![part(Body::Index(k)), part(phase)])),
    );
    let law = Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(part(carrier), part(Body::Index(k)))),
    }));
    let (buffer, label) = render(&form(Var::T, law), RATE, (0.0, 0.25), &PSYCHOACOUSTIC_V1)
        .expect("a moving pitch sums instant by instant");
    let ceiling = PSYCHOACOUSTIC_V1.ceiling(RATE);
    for i in 0..buffer.len() {
        let t = i as f64 / f64::from(RATE);
        let (angle, hz) = (
            TAU * 220.0 * t + 10.0 * (TAU * 5.0 * t).sin(),
            220.0 + 50.0 * (TAU * 5.0 * t).cos(),
        );
        let want: f64 = (1..)
            .take_while(|n| f64::from(*n) * hz < ceiling - 1e-6)
            .map(|n| (f64::from(n) * angle).sin() / f64::from(n))
            .sum();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
    let Detail::Point { tail_db, .. } = label.detail else {
        panic!("a point row, not {:?}", label.detail);
    };
    let first = (1..)
        .find(|n| f64::from(*n) * 270.0 >= ceiling)
        .expect("a harmonic past it");
    let loudest = 20.0 * (1.0 / f64::from(first)).log10();
    let tail = tail_db.expect("a bound on what it drops");
    assert!((tail - loudest).abs() < 1e-9, "{tail} dB against {loudest}");
}

/// Series inside series expand as their counts multiply. Where that product is past what
/// one instant expands, the refusal says how deep the nesting goes and what it would take.
#[test]
fn nested_series_beyond_the_bound_refuse_naming_the_count() {
    let deep = (0..5).fold(cosine(220.0, 1.0), |inner, level| {
        let k = IndexId(level + 1);
        Body::Series(Box::new(Series {
            index: k,
            lo: 0,
            hi: Bound::Infinite,
            term: part(Body::Mul(vec![
                part(Body::Apply(
                    Unary::Exp,
                    part(Body::Mul(vec![
                        part(Body::Index(k)),
                        part(constant(0.82f64.ln())),
                    ])),
                )),
                part(inner),
            ])),
        }))
    });
    let refused = render(&form(Var::T, deep), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect_err("five nested series are past the bound");
    assert_eq!(refused.code(), "collapse.series_nesting");
    let CollapseError::NestedSeries {
        depth,
        terms,
        bound,
    } = refused
    else {
        panic!("the refusal prices the nesting, not {refused:?}");
    };
    assert_eq!(depth, 5, "five series deep");
    let terms = terms.expect("a count a usize holds");
    let whole = 1.0 / (1.0 - 0.82f64);
    let levels: usize = (0..5)
        .map(|level| kept(whole.powi(level), 0.82) as usize)
        .product();
    assert_eq!(
        terms, levels,
        "each level's terms under the sum it holds, multiplied five deep"
    );
    assert!(terms > bound, "{terms} is past the bound {bound}");
    assert!(
        refused.to_string().contains("5 deep"),
        "the message names the depth: {refused}"
    );
}

/// A series' price counts series terms. A term holding no series is one value at an
/// instant however many addends its spelling multiplies out to, so forty terms each a product
/// of sums (a formant weight over a moving pitch, say) take forty terms, not those addends.
#[test]
fn a_finite_series_of_products_of_sums_prices_its_own_terms() {
    let k = IndexId(1);
    // `(k + tanh(t) + 1)` nine times over: 3^9 addends were once read as 19683 terms.
    let factor = || {
        part(Body::Add(vec![
            part(Body::Index(k)),
            part(Body::Apply(Unary::Tanh, part(Body::Line))),
            part(constant(1.0)),
        ]))
    };
    let mut factors: Vec<Part> = (0..9).map(|_| factor()).collect();
    factors.push(part(cosine(220.0, 1e-12)));
    let series = Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Finite(40),
        term: part(Body::Mul(factors)),
    }));
    render(&form(Var::T, series), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect("forty terms are forty terms at one instant");
}

/// A nesting whose count overflows a `usize` says so, and never prints the saturated word.
#[test]
fn a_nesting_past_any_count_refuses_without_a_saturated_count() {
    let deep = (0..4).fold(cosine(220.0, 1.0), |inner, level| {
        let k = IndexId(level + 1);
        Body::Series(Box::new(Series {
            index: k,
            lo: 1,
            hi: Bound::Finite(1_000_000),
            term: part(Body::Mul(vec![
                part(Body::Apply(Unary::Tanh, part(Body::Index(k)))),
                part(inner),
            ])),
        }))
    });
    let refused = render(&form(Var::T, deep), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect_err("a million terms four deep are past the bound");
    let CollapseError::NestedSeries { depth, terms, .. } = &refused else {
        panic!("the refusal prices the nesting, not {refused:?}");
    };
    assert_eq!(*depth, 4);
    assert_eq!(*terms, None, "10^24 terms is past what a usize counts");
    let said = refused.to_string();
    assert!(
        !said.contains(&usize::MAX.to_string()),
        "no saturated count leaks: {said}"
    );
    assert!(said.contains("4 deep"), "the depth is still named: {said}");
}

/// An indicator is a pointwise factor and distributes over a sum however long, so a cropped
/// series stays a series and every term carries the one window.
#[test]
fn a_cropped_neumann_series_has_a_spectral_sum() {
    let k = IndexId(1);
    let neumann = Body::Series(Box::new(Series {
        index: k,
        lo: 0,
        hi: Bound::Infinite,
        term: part(Body::Mul(vec![
            part(Body::Apply(
                Unary::Exp,
                part(Body::Mul(vec![
                    part(Body::Index(k)),
                    part(constant(0.5f64.ln())),
                ])),
            )),
            part(cosine(220.0, 1.0)),
        ])),
    }));
    let law = Body::Crop {
        of: part(neumann),
        l: Edge::at(0.002),
        r: Edge::at(0.006),
        rise: 0.0,
        fall: 0.0,
    };
    let sum = sva_formula::normalize(&law, Var::T).expect("a cropped series normalizes");
    assert_eq!(
        sum.lanes[0].series.len(),
        1,
        "the crop kept the series rather than expanding it"
    );

    let (buffer, _) = render(&form(Var::T, law), RATE, (0.0, 0.01), &PSYCHOACOUSTIC_V1)
        .expect("a cropped series collapses");
    let counted = kept(1.0, 0.5);
    let taken: f64 = (0..counted).map(|n| 0.5f64.powi(n)).sum();
    for i in 0..buffer.len() {
        let t = i as f64 / f64::from(RATE);
        let inside = (0.002..=0.006).contains(&t);
        let want = if inside {
            taken * (TAU * 220.0 * t).cos()
        } else {
            0.0
        };
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i} at t={t}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// FORMAT 3.1: the indicator is half-open, so two windows that abut fire on disjoint
/// instants and their sum never reaches twice one segment's own height at the seam.
#[test]
fn abutting_crops_do_not_double_at_the_seam() {
    let segment = |from: f64, to: f64, hz: f64| Body::Crop {
        of: part(cosine(hz, 1.0)),
        l: Edge::at(from),
        r: Edge::at(to),
        rise: 0.0,
        fall: 0.0,
    };
    let law = form(
        Var::T,
        sum(vec![segment(0.0, 0.5, 440.0), segment(0.5, 1.0, 660.0)]),
    );
    let (buffer, _) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("two abutting crops");

    let seam = (0.5 * f64::from(RATE)) as usize;
    let (below, above) = ((TAU * 440.0 * 0.5f64).cos(), (TAU * 660.0 * 0.5f64).cos());
    assert!(
        (buffer.at(0, seam) - above).abs() < 1e-9,
        "the seam instant belongs to the later window alone, got {} against {above} and {below}",
        buffer.at(0, seam)
    );
    for i in 0..buffer.len() {
        assert!(
            buffer.at(0, i).abs() <= 1.0 + 1e-9,
            "sample {i} is {}, past either segment's own height",
            buffer.at(0, i)
        );
    }
}

/// FORMAT 9.1 row three: the tail is the energy the window's own dual carries past the
/// ceiling, so a longer window, whose dual is narrower, loses less of it.
#[test]
fn tail_db_falls_as_the_window_lengthens() {
    let cropped = |width: f64| {
        let law = form(
            Var::T,
            Body::Crop {
                of: part(cosine(440.0, 1.0)),
                l: Edge::at(0.0),
                r: Edge::at(width),
                rise: 0.0,
                fall: 0.0,
            },
        );
        let (_, label) =
            render(&law, RATE, (0.0, 2.0), &PSYCHOACOUSTIC_V1).expect("a cropped pair");
        let Detail::Cropped { tail_db, .. } = label.detail else {
            panic!("expected a cropped label, got {:?}", label.detail);
        };
        tail_db.expect("a sinc window states its tail")
    };
    let mut held = f64::INFINITY;
    for width in [0.05, 0.1, 0.2, 0.4, 0.8, 1.6] {
        let found = cropped(width);
        assert!(
            (found - sinc_tail_db(440.0, width)).abs() < 0.05,
            "a {width}s window states {found} dB where the sinc tail is {} dB",
            sinc_tail_db(440.0, width)
        );
        assert!(
            found < held - 2.9,
            "a {width}s window states {found} dB against {held} dB for the shorter one"
        );
        held = found;
    }
}

/// A window whose plateau closes to nothing is still a finite amount of energy in a finite
/// window, and states a tail below unity rather than a positive one.
#[test]
fn a_short_shoulder_has_a_finite_negative_tail() {
    let law = form(
        Var::T,
        Body::Crop {
            of: part(cosine(440.0, 1.0)),
            l: Edge::at(0.0),
            r: Edge::at(0.045),
            rise: 0.005,
            fall: 0.04,
        },
    );
    let (_, label) = render(&law, RATE, (0.0, 3.0), &PSYCHOACOUSTIC_V1).expect("a shouldered pair");
    let Detail::Cropped { tail_db, .. } = label.detail else {
        panic!("expected a cropped label, got {:?}", label.detail);
    };
    let tail_db = tail_db.expect("a raised cosine is a sum of sinc windows");
    // An energy-weighted mean over the window's pieces sits between its two shoulders' own.
    assert!(
        tail_db > sinc_tail_db(440.0, 0.04) - 1.0 && tail_db < sinc_tail_db(440.0, 0.005) + 1.0,
        "a 45 ms window states {tail_db} dB, outside the {} to {} dB its two shoulders bound",
        sinc_tail_db(440.0, 0.04),
        sinc_tail_db(440.0, 0.005)
    );
}

/// Half of a sinc sits above its own line, so a cropped tone at the ceiling loses half its
/// energy, one just inside loses less, and one outside the band keeps almost nothing.
#[test]
fn a_cropped_tone_states_its_tail_on_either_side_of_the_ceiling() {
    let cropped = |hz: f64, width: f64| {
        let law = form(
            Var::T,
            Body::Crop {
                of: part(cosine(hz, 1.0)),
                l: Edge::at(0.0),
                r: Edge::at(width),
                rise: 0.0,
                fall: 0.0,
            },
        );
        let (_, label) =
            render(&law, RATE, (0.0, 2.0), &PSYCHOACOUSTIC_V1).expect("a cropped pair");
        let Detail::Cropped { tail_db, .. } = label.detail else {
            panic!("expected a cropped label, got {:?}", label.detail);
        };
        tail_db.expect("a sinc window states its tail")
    };
    const HALF_DB: f64 = -3.0103;

    let ceiling = f64::from(RATE) / 2.0;
    let at = cropped(ceiling, 0.25);
    assert!(
        (at - HALF_DB).abs() < 0.02,
        "a line on the ceiling states {at} dB, not the half a sinc splits"
    );

    let past = cropped(ceiling + 500.0, 0.25);
    assert!(
        past > HALF_DB && past < 0.0,
        "a line 500 Hz past the ceiling states {past} dB"
    );

    let inside = cropped(ceiling - 1.0, 0.3);
    assert!(
        inside < HALF_DB && inside > -10.0,
        "a line 1 Hz inside the ceiling states {inside} dB"
    );
}

/// A Gaussian under a window duals to an error function, not a sinc, so its tail is no
/// integral this states and the label carries no number rather than a wrong one.
#[test]
fn a_window_whose_dual_is_no_sinc_states_no_tail() {
    let bell = Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(constant(-100.0)),
            part(Body::Line),
            part(Body::Line),
        ])),
    );
    let law = form(
        Var::T,
        Body::Crop {
            of: part(Body::Mul(vec![part(bell), part(cosine(440.0, 1.0))])),
            l: Edge::at(0.0),
            r: Edge::at(0.5),
            rise: 0.0,
            fall: 0.0,
        },
    );
    let (_, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a cropped Gaussian");
    let Detail::Cropped { tail_db, .. } = label.detail else {
        panic!("expected a cropped label, got {:?}", label.detail);
    };
    assert_eq!(tail_db, None, "a cropped Gaussian names no sinc tail");
}

/// Two lines under windows of their own state one tail between them: the energy each window
/// carries weights it, so a long clean partial neither hides nor inherits a short one's loss.
#[test]
fn two_windows_state_one_energy_weighted_tail() {
    let segment = |hz: f64, width: f64| Body::Crop {
        of: part(cosine(hz, 1.0)),
        l: Edge::at(0.0),
        r: Edge::at(width),
        rise: 0.0,
        fall: 0.0,
    };
    let law = form(Var::T, sum(vec![segment(440.0, 1.6), segment(880.0, 0.05)]));
    let (_, label) = render(&law, RATE, (0.0, 2.0), &PSYCHOACOUSTIC_V1).expect("two crops");
    let Detail::Cropped { tail_db, .. } = label.detail else {
        panic!("expected a cropped label, got {:?}", label.detail);
    };
    let found = tail_db.expect("two sinc windows state their tail");

    let ratio = |hz: f64, width: f64| 10f64.powf(sinc_tail_db(hz, width) / 10.0);
    let weighted = 10.0 * ((1.6 * ratio(440.0, 1.6) + 0.05 * ratio(880.0, 0.05)) / 1.65).log10();
    assert!(
        (found - weighted).abs() < 0.05,
        "two windows state {found} dB where their energies weigh to {weighted} dB"
    );
    assert!(
        found > sinc_tail_db(440.0, 1.6) && found < sinc_tail_db(880.0, 0.05),
        "the pair states {found} dB, outside what either window states alone"
    );
}

/// Every line of a noise series under the ceiling is summed at each instant.
#[test]
fn noise_alone_sums_its_lines() {
    let law = form(
        Var::T,
        Body::Series(Box::new(sva_formula::noise(1, 0.4, 0.0))),
    );
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a noise series");
    let Detail::Lines { summed, .. } = &label.detail else {
        panic!("expected a line label, got {:?}", label.detail);
    };
    assert_eq!(label.rule(), Rule::LineSpectrumSummed);
    assert!(*summed > 1_000, "{summed} lines under the ceiling");

    let placed = sva_formula::lines(
        &sva_formula::noise(1, 0.4, 0.0),
        4_096.0,
        -20.0,
        PSYCHOACOUSTIC_V1.half_lsb(),
    )
    .expect("noise lines");
    for i in [0usize, 137, 4_001] {
        let t = i as f64 / f64::from(RATE);
        let direct: f64 = placed
            .taken
            .iter()
            .map(|l| (l.amp * C64::new(0.0, TAU * l.hz * t).exp()).re)
            .sum();
        assert!(
            (buffer.at(0, i) - direct).abs() < 1e-6,
            "sample {i} places {} where the lines sum to {direct}",
            buffer.at(0, i)
        );
    }
}

/// A line beside a noise series is one more line: the label counts it and the buffer is what
/// each contributes, added.
#[test]
fn noise_plus_a_sine_is_the_sum_of_its_parts() {
    let hiss = Body::Series(Box::new(sva_formula::noise(1, 0.4, 0.0)));
    let tone = cosine(554.365, 0.06);
    let (alone, alone_label) = render(
        &form(Var::T, hiss.clone()),
        RATE,
        whole_second(),
        &PSYCHOACOUSTIC_V1,
    )
    .expect("a noise series");
    let (sine, sine_label) = render(
        &form(Var::T, tone.clone()),
        RATE,
        whole_second(),
        &PSYCHOACOUSTIC_V1,
    )
    .expect("one line");
    assert_eq!(sine_label.rule(), Rule::LineSpectrumSummed);

    let (both, label) = render(
        &form(Var::T, sum(vec![hiss, tone])),
        RATE,
        whole_second(),
        &PSYCHOACOUSTIC_V1,
    )
    .expect("a noise series beside one line");
    assert_eq!(label.rule(), Rule::LineSpectrumSummed);
    let count = |label: &sva_samples::Label| match label.detail {
        Detail::Lines { summed, .. } => summed,
        _ => panic!("expected a line label, got {:?}", label.detail),
    };
    assert_eq!(
        count(&label),
        count(&alone_label) + 1,
        "the sine is one more line"
    );
    for i in [0usize, 137, 4_001] {
        assert!(
            (both.at(0, i) - alone.at(0, i) - sine.at(0, i)).abs() < 1e-9,
            "sample {i} is not what the two routes contribute"
        );
    }
}

/// FORMAT 9.1 row three: a cropped series is the series of cropped terms, so the window
/// stays an indicator on each line rather than a factor point sampling has to discover.
#[test]
fn a_cropped_noise_series_evaluates_only_its_window() {
    let law = form(
        Var::T,
        Body::Crop {
            of: part(Body::Series(Box::new(sva_formula::noise(3, 0.25, 0.0)))),
            l: Edge::at(0.21),
            r: Edge::at(0.29),
            rise: 0.0,
            fall: 0.0,
        },
    );
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a cropped noise series");
    assert_eq!(
        label.rule(),
        Rule::CroppedPair,
        "a windowed line spectrum is a cropped pair, not a law point sampling has to walk"
    );
    let window = |secs: f64| (secs * f64::from(RATE)) as usize;
    for i in [0usize, window(0.2), window(0.3), buffer.len() - 1] {
        assert_eq!(buffer.at(0, i), 0.0, "sample {i} sits outside the window");
    }
    assert!(
        (window(0.21)..window(0.29)).any(|i| buffer.at(0, i) != 0.0),
        "the window itself is not silent"
    );
}

/// FORMAT 15.6: a raised cosine is a pointwise factor, so it multiplies whatever closed form it is
/// written over. Over a line series it distributes term by term, and the window stays an
/// indicator on each line rather than a factor point sampling has to discover.
#[test]
fn a_shoulder_over_a_noise_series_keeps_a_pair() {
    let law = form(
        Var::T,
        Body::Crop {
            of: part(Body::Series(Box::new(sva_formula::noise(3, 0.25, 0.0)))),
            l: Edge::at(0.2),
            r: Edge::at(0.4),
            rise: 0.01,
            fall: 0.01,
        },
    );
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a shouldered noise series");
    assert_eq!(
        label.rule(),
        Rule::CroppedPair,
        "a windowed line spectrum is a cropped pair"
    );
    let at = |secs: f64| (secs * f64::from(RATE)) as usize;
    for i in [0usize, at(0.19), at(0.41), buffer.len() - 1] {
        assert_eq!(buffer.at(0, i), 0.0, "sample {i} sits outside the window");
    }
    assert!(
        (at(0.2)..at(0.4)).any(|i| buffer.at(0, i) != 0.0),
        "the window itself is not silent"
    );
}

/// The same window over a closed form no atom sum reaches: the product is that closed form point sampled,
/// not a refusal that the window has no value at an instant.
#[test]
fn a_shoulder_over_a_ct_is_a_ct() {
    let glide = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![
            part(constant(TAU * 55.0)),
            part(Body::Apply(
                Unary::Exp,
                part(Body::Mul(vec![
                    part(constant(std::f64::consts::LN_2 / 8.0)),
                    part(Body::Line),
                ])),
            )),
            part(Body::Line),
        ])),
    );
    let law = form(
        Var::T,
        Body::Crop {
            of: part(glide),
            l: Edge::at(0.1),
            r: Edge::at(0.9),
            rise: 0.05,
            fall: 0.4,
        },
    );
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a shouldered glide");
    assert_eq!(label.rule(), Rule::PointSampled);
    assert_eq!(label.source, Source::Measured);
    let at = |secs: f64| (secs * f64::from(RATE)) as usize;
    assert_eq!(buffer.at(0, at(0.05)), 0.0, "before the window");
    assert_eq!(buffer.at(0, at(0.95)), 0.0, "after it");
    assert!(
        buffer.at(0, at(0.5)).abs() > 0.0,
        "the plateau carries the glide"
    );
    assert!(
        buffer.at(0, at(0.11)).abs() < buffer.at(0, at(0.5)).abs(),
        "the shoulder opens rather than jumping"
    );
}

/// A zero gain over a series leaves a closed form with no line at all. That is silence, not a band
/// every line sat above, and it collapses to zeros rather than refusing.
#[test]
fn a_law_scaled_to_nothing_collapses_to_silence() {
    let law = form(
        Var::T,
        Body::Mul(vec![
            part(constant(0.0)),
            part(Body::Series(Box::new(sva_formula::noise(1, 0.5, 0.0)))),
        ]),
    );
    let (buffer, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a law worth nothing");
    assert_eq!(label.source, Source::Exact);
    assert!(
        (0..buffer.len()).all(|i| buffer.at(0, i) == 0.0),
        "every sample is silence"
    );
}

/// A gain written outside a cropped series is still a gain on each of its lines: the window
/// stays where the collapse reads it rather than burying the line closed form under a product.
#[test]
fn a_gain_outside_a_cropped_series_keeps_the_window() {
    let cropped = Body::Crop {
        of: part(Body::Series(Box::new(sva_formula::noise(3, 0.25, 0.0)))),
        l: Edge::at(0.2),
        r: Edge::at(0.4),
        rise: 0.0,
        fall: 0.0,
    };
    let law = form(
        Var::T,
        Body::Mul(vec![part(constant(0.2)), part(cropped.clone())]),
    );
    let (scaled, label) =
        render(&law, RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("a scaled cropped series");
    assert_eq!(label.rule(), Rule::CroppedPair, "the window is still read");

    let (bare, _) = render(
        &form(Var::T, cropped),
        RATE,
        whole_second(),
        &PSYCHOACOUSTIC_V1,
    )
    .expect("the same series unscaled");
    for i in [0usize, (0.25 * f64::from(RATE)) as usize, bare.len() - 1] {
        assert!(
            (scaled.at(0, i) - 0.2 * bare.at(0, i)).abs() < 1e-12,
            "sample {i} is not the gain times the series"
        );
    }
}

/// A bound a composer wrote is a term count like the floor's own, so a nesting under one is
/// priced and refused by the same name. Under the bound, it collapses.
#[test]
fn a_nested_series_past_the_bound_refuses_or_labels() {
    let nested = |outer: i64| {
        let inner = Body::Series(Box::new(Series {
            index: IndexId(2),
            lo: 1,
            hi: Bound::Infinite,
            term: part(Body::Mul(vec![
                part(Body::Apply(
                    Unary::Exp,
                    part(Body::Mul(vec![
                        part(Body::Index(IndexId(2))),
                        part(constant(0.82f64.ln())),
                    ])),
                )),
                part(cosine(220.0, 1.0)),
            ])),
        }));
        Body::Series(Box::new(Series {
            index: IndexId(1),
            lo: 1,
            hi: Bound::Finite(outer),
            term: part(Body::Mul(vec![
                part(Body::Apply(
                    Unary::Exp,
                    part(Body::Mul(vec![
                        part(Body::Index(IndexId(1))),
                        part(constant(0.5f64.ln())),
                    ])),
                )),
                part(inner),
            ])),
        }))
    };
    let horizon = (0.0, 0.01);
    let run = |body: Body| render(&form(Var::T, body), RATE, horizon, &PSYCHOACOUSTIC_V1);

    let per = kept(0.82, 0.82) as usize;
    let refused =
        run(nested(4_000)).expect_err("a written bound times a nesting is past the bound");
    assert_eq!(refused.code(), "collapse.series_nesting");
    let CollapseError::NestedSeries {
        depth,
        terms,
        bound,
    } = refused
    else {
        panic!("the refusal prices the written bound too, not {refused:?}");
    };
    assert_eq!(depth, 2, "two series deep");
    let terms = terms.expect("a count a usize holds");
    assert_eq!(
        terms,
        4_000 * per,
        "the written bound multiplies the nesting"
    );
    assert!(terms > bound);

    // The addend route is the same route: a sum does not dilute a nesting past the bound.
    let beside = Body::Add(vec![part(nested(4_000)), part(cosine(330.0, 0.5))]);
    assert_eq!(
        run(beside)
            .expect_err("an addend past the bound refuses the sum")
            .code(),
        "collapse.series_nesting"
    );

    run(nested(4)).expect("the same nesting under the bound collapses");
}

/// `t + sin(2*pi*5*t)/(10*pi)` runs at most twice as fast as `t`, so a line series read there
/// keeps the lines at most half the ceiling: none sounds past it at any instant.
#[test]
fn a_warped_line_series_keeps_the_lines_its_steepest_rate_leaves_under_the_ceiling() {
    let k = IndexId(1);
    let line = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![
            part(constant(TAU * 50.0)),
            part(Body::Index(k)),
            part(Body::Line),
        ])),
    );
    let series = Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(part(line), part(Body::Index(k)))),
    }));
    let wobble = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![part(constant(TAU * 5.0)), part(Body::Line)])),
    );
    let at = Body::Add(vec![
        part(Body::Line),
        part(Body::Div(part(wobble), part(constant(10.0 * PI)))),
    ]);
    let warped = Body::Warp {
        at: part(at),
        of: part(series),
    };
    let band = sva_samples::Audible::of(&PSYCHOACOUSTIC_V1, RATE);
    let Body::Warp { of, .. } = sva_samples::truncate_written(&warped, band).expect("a count")
    else {
        panic!("a truncated warp is a warp");
    };
    let Body::Run(run) = &*of.body else {
        panic!("one ladder of lines is one run, not {:?}", of.body);
    };
    let top = run.offset + run.step * (run.first + run.amps.len() as i64 - 1) as f64;
    let half = PSYCHOACOUSTIC_V1.ceiling(RATE) / 2.0;
    assert!(
        top <= half && top + 50.0 > half,
        "the top line {top} Hz sits under {half} Hz"
    );
}
