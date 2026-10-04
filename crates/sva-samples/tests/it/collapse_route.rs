// Concern: states that a line spectrum summed at each instant returns a constant and a unit sine bit for bit | Non-concern: a non-line form's rows | IO: (a ClosedForm) -> Buffer and Label

use crate::helpers::{part, render, whole_second};
use std::f64::consts::TAU;

use sva_formula::{Body, C64, ClosedForm, Origin, Unary, Var};
use sva_samples::{Detail, PSYCHOACOUSTIC_V1, Rule, Source};

const RATE: u32 = 44_100;

fn form(body: Body) -> ClosedForm {
    ClosedForm {
        var: Var::T,
        body,
        origin: Origin::new(0),
    }
}

fn sine(hz: f64) -> Body {
    Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![
            part(Body::Const(C64::real(TAU * hz))),
            part(Body::Line),
        ])),
    )
}

/// A constant's collapse is the constant, to the last bit.
#[test]
fn a_constant_collapses_bit_exactly() {
    let (buffer, label) = render(
        &form(Body::Const(C64::real(1.0))),
        RATE,
        whole_second(),
        &PSYCHOACOUSTIC_V1,
    )
    .expect("a constant");

    assert_eq!(label.source, Source::Exact);
    assert_eq!(label.rule(), Rule::LineSpectrumSummed);
    let Detail::Lines { summed, .. } = &label.detail else {
        panic!("expected a line label, got {:?}", label.detail);
    };
    assert_eq!(*summed, 1, "one line");
    for i in 0..buffer.len() {
        assert_eq!(
            buffer.at(0, i),
            1.0,
            "sample {i} is not the constant itself"
        );
    }
}

/// 441 Hz over 44100 samples puts a quarter turn on sample 25, where the sine is one.
#[test]
fn a_unit_sine_peaks_at_exactly_one() {
    let (buffer, label) =
        render(&form(sine(441.0)), RATE, whole_second(), &PSYCHOACOUSTIC_V1).expect("one sine");

    assert_eq!(label.source, Source::Exact);
    assert_eq!(label.rule(), Rule::LineSpectrumSummed);
    assert_eq!(buffer.at(0, 25), 1.0, "the quarter turn is one");
    let peak = (0..buffer.len()).fold(0.0f64, |held, i| held.max(buffer.at(0, i).abs()));
    assert_eq!(peak, 1.0, "a unit sine never leaves the unit interval");
}
