// Concern: proves the symbolic derivative, the Hilbert transform and the envelope stay in A | Non-concern: the transform they are built on (duality.rs) | IO: (a SpectralSum) -> another one

use crate::fixtures::{causal, constant, cosine, decay, line, part, sine};
use sva_formula::{
    Body, C64, Edge, LeftReason, SpectralSum, Var, d_dt, envelope, hilbert, normalize,
};

fn law(f: &Body) -> SpectralSum {
    normalize(f, Var::T).expect("the fixture stays in A")
}

/// An order or degree past `u16` refuses rather than wrapping to a lower one.
#[test]
fn an_order_or_degree_past_its_width_refuses() {
    let pole = law(&Body::Pow(part(line()), -i32::from(u16::MAX)));
    let left = d_dt(&pole).expect_err("the pole order passes u16");
    assert_eq!(left.reason, LeftReason::PoleOrder(u16::MAX));

    let highest = Body::Pow(part(line()), i32::from(u16::MAX));
    assert!(
        normalize(&highest, Var::T).is_ok(),
        "degree u16::MAX itself is held"
    );
    let raised = Body::Mul(vec![
        part(Body::Pow(part(line()), i32::from(u16::MAX))),
        part(line()),
    ]);
    let left = normalize(&raised, Var::T).expect_err("the degree passes u16");
    assert_eq!(left.reason, LeftReason::Overflow);

    let delta = law(&Body::Delta {
        at: part(line()),
        order: u16::MAX,
    });
    let left = d_dt(&delta).expect_err("the delta's order passes u16");
    assert_eq!(left.reason, LeftReason::Overflow);
}

#[test]
fn d_dt_of_a_damped_sinusoid() {
    let ringing = causal(Body::Mul(vec![part(sine(3.0)), part(decay(-2.0))]));
    let derived = d_dt(&law(&ringing)).expect("a derivative");
    let atoms = &derived.lanes[0].atoms;
    assert_eq!(atoms.len(), 2, "the edge deltas cancel at a zero crossing");
    for atom in atoms {
        let e = atom.exp.expect("still a damped line");
        assert!((e.sigma + 2.0).abs() < 1e-12);
        assert!(atom.ind.is_some(), "still causal");
    }

    let shifted = causal(Body::Mul(vec![part(cosine(3.0)), part(decay(-2.0))]));
    let with_edge = d_dt(&law(&shifted)).expect("a derivative");
    assert!(
        with_edge.lanes[0]
            .atoms
            .iter()
            .any(sva_formula::SpectralAtom::is_delta),
        "a step from zero leaves an impulse at the edge"
    );
}

#[test]
fn d_dt_of_an_indicator_is_two_deltas() {
    let window = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(0.0),
        r: Edge::at(2.0),
        rise: 0.0,
        fall: 0.0,
    };
    let derived = d_dt(&law(&window)).expect("a derivative");
    let atoms = &derived.lanes[0].atoms;
    assert_eq!(atoms.len(), 2);
    assert!(atoms.iter().all(sva_formula::SpectralAtom::is_delta));
    assert_eq!(
        atoms.iter().map(|a| a.c).fold(C64::ZERO, |x, y| x + y),
        C64::ZERO
    );
}

#[test]
fn hilbert_of_a_cosine_is_its_sine() {
    let quadrature = hilbert(&law(&cosine(440.0))).expect("a line pair has a Hilbert transform");
    assert_eq!(quadrature, law(&sine(440.0)));
}

/// The step's own dual carries a principal value, and that is where its Hilbert transform
/// ends: `sgn` times a pole has no atom sum. `pv` is a constructor because of this row.
#[test]
fn the_hilbert_transform_of_a_step_needs_pv() {
    let step = law(&causal(constant(1.0)));
    assert!(
        sva_formula::dual(&step).unwrap().lanes[0]
            .atoms
            .iter()
            .any(|a| a.pole.is_some_and(|p| p.pv)),
        "the step's dual is a delta beside a principal value"
    );
    let left = hilbert(&step).expect_err("the principal value is where A ends");
    assert_eq!(left.reason, LeftReason::PoleTimesIndicator);
    assert!(left.sketch.describe().contains("principal value"));
}

#[test]
fn hilbert_of_a_polynomial_refuses() {
    let left = hilbert(&law(&line())).expect_err("a polynomial has no tempered Hilbert transform");
    assert_eq!(left.reason, LeftReason::HilbertOfPolynomial);
}

/// The envelope is the analytic signal against its conjugate, so a single line's is its own
/// squared amplitude: one real atom, and no frame length anywhere.
#[test]
fn the_envelope_of_a_sinusoid_is_its_squared_amplitude() {
    let scaled = Body::Mul(vec![part(constant(0.25)), part(sine(440.0))]);
    let squared = envelope(&law(&scaled))
        .expect("a line pair has an envelope")
        .squared;
    let [atom] = squared.lanes[0].atoms[..] else {
        panic!("one real atom");
    };
    assert!(atom.is_bare() && !atom.is_delta());
    assert!((atom.c.re - 0.0625).abs() < 1e-12, "{:?}", atom.c);
    assert!(atom.c.im.abs() < 1e-15, "a squared modulus is real");

    let ringing = causal(Body::Mul(vec![part(sine(440.0)), part(decay(-2.0))]));
    assert_eq!(
        envelope(&law(&ringing)).unwrap_err().reason,
        LeftReason::PoleTimesIndicator,
        "a causal law's analytic signal is not in A"
    );
}
