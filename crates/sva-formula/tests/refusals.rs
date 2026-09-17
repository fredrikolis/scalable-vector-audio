// Concern: proves each refusal names the subterm that caused it and the repair | Non-concern: what types when nothing refuses (inference.rs) | IO: (a ClosedForm) -> a Refusal code and message

mod fixtures;

use fixtures::{DUAL, Fixed, constant, gaussian, line, part, sine, term};
use sva_formula::{
    Body, Bound, C64, ClosedForm, Code, Codomain, Edge, Env, Held, IndexId, LeftReason, NodeId,
    Origin, ParamId, Part, Series, Ty, Unary, Var, dual, infer, normalize,
};

/// One node, whose type the test picks, so a closed form can be made to meet the wrong thing.
struct OneNode(Ty);

impl Env for OneNode {
    fn node(&self, _: NodeId) -> Ty {
        self.0
    }

    fn param(&self, _: ParamId) -> Ty {
        self.0
    }
}

fn refuse(t: &ClosedForm, env: &dyn Env) -> sva_formula::Refusal {
    infer(t, env).expect_err("this term should refuse")
}

#[test]
fn a_non_affine_delta_refuses() {
    let squared = Body::Delta {
        at: Part::new(Origin::new(7), Body::Pow(part(line()), 2)),
        order: 0,
    };
    let refusal = refuse(&term(Var::T, squared), &Fixed::holding(DUAL, false));
    assert_eq!(refusal.code, Code::NonAffineSingular);
    assert_eq!(refusal.origin, Origin::new(7));
    assert!(
        refusal.message.contains("affine in t"),
        "{}",
        refusal.message
    );
}

#[test]
fn a_non_summable_series_refuses() {
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
    let refusal = refuse(&term(Var::T, geometric), &Fixed::holding(DUAL, false));
    assert_eq!(refusal.code, Code::SeriesNotSummable);
    assert!(
        refusal.message.contains("polynomially bounded"),
        "{}",
        refusal.message
    );
}

#[test]
fn mixing_t_and_f_names_both_repairs() {
    let spectrum = OneNode(Ty::form(Var::F, false, Codomain::Complex));
    let mixed = Body::Mul(vec![part(sine(440.0)), part(Body::Node(NodeId(0)))]);
    let refusal = refuse(&term(Var::T, mixed), &spectrum);
    assert_eq!(refusal.code, Code::DomainMismatch);
    assert!(refusal.message.contains('f') && refusal.message.contains('t'));
}

#[test]
fn mixing_a_closed_form_and_samples_names_sample() {
    let sampled = OneNode(Ty {
        rate: Some(44100),
        ..Ty::discrete(Held::Sampled, Codomain::Real)
    });
    let mixed = Body::Mul(vec![part(sine(440.0)), part(Body::Node(NodeId(0)))]);
    let refusal = refuse(&term(Var::T, mixed), &sampled);
    assert_eq!(refusal.code, Code::SamplesInClosedForm);
    assert!(refusal.message.contains("sample("), "{}", refusal.message);
}

#[test]
fn a_reciprocal_of_a_law_leaves_the_algebra() {
    let inverted = Body::Div(
        part(constant(1.0)),
        Part::new(
            Origin::new(3),
            Body::Add(vec![part(constant(1.0)), part(line())]),
        ),
    );
    assert!(
        !infer(
            &term(Var::T, inverted.clone()),
            &Fixed::holding(DUAL, false)
        )
        .unwrap()
        .has_dual()
    );
    let left = sva_formula::normalize(&inverted, Var::T).unwrap_err();
    assert_eq!(left.origin, Origin::new(3));
    assert_eq!(left.reason, sva_formula::LeftReason::Reciprocal);
    assert_eq!(left.refusal().code, Code::LeftAlgebra);
}

#[test]
fn a_node_reaching_the_spectral_sum_unsubstituted_refuses() {
    let unresolved = Body::Mul(vec![
        part(Body::Const(C64::ONE)),
        Part::new(Origin::new(11), Body::Node(NodeId(4))),
    ]);
    let left = sva_formula::normalize(&unresolved, Var::T).unwrap_err();
    assert_eq!(left.origin, Origin::new(11));
    assert_eq!(left.reason, sva_formula::LeftReason::Unsubstituted);
}

fn blocked(f: &Body) -> sva_formula::Left {
    dual(&normalize(f, Var::T).expect("the atom sum exists")).expect_err("the dual leaves A")
}

#[test]
fn a_blocked_transform_names_the_subterm() {
    let driven = Body::Apply(
        Unary::Tanh,
        part(Body::Mul(vec![part(constant(3.0)), part(sine(440.0))])),
    );
    let left = normalize(
        &Body::Add(vec![part(sine(110.0)), Part::new(Origin::new(42), driven)]),
        Var::T,
    )
    .unwrap_err();
    assert_eq!(left.origin, Origin::new(42));
    assert_eq!(left.reason, LeftReason::Nonlinearity);
    assert!(
        left.refusal().message.contains("left A"),
        "{}",
        left.refusal().message
    );
}

#[test]
fn a_gaussian_times_an_indicator_names_the_error_function() {
    let cropped = Body::Crop {
        of: part(gaussian(1.0, 0.0)),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    };
    let left = blocked(&cropped);
    assert_eq!(left.reason, LeftReason::GaussianTimesIndicator);
    assert!(left.refusal().message.contains("error function"));
    assert!(
        !infer(&term(Var::T, cropped), &Fixed::holding(DUAL, false))
            .unwrap()
            .has_dual(),
        "inference reaches the same verdict without normalizing"
    );
}

#[test]
fn a_pole_times_an_indicator_names_the_exponential_integral() {
    let cropped = Body::Crop {
        of: part(Body::Pv(part(line()))),
        l: Edge::at(1.0),
        r: Edge::at(2.0),
        rise: 0.0,
        fall: 0.0,
    };
    let left = blocked(&cropped);
    assert_eq!(left.reason, LeftReason::PoleTimesIndicator);
    assert!(left.refusal().message.contains("exponential integral"));
    assert!(
        !infer(&term(Var::T, cropped), &Fixed::holding(DUAL, false))
            .unwrap()
            .has_dual()
    );
}

#[test]
fn a_growing_exponential_names_temperedness() {
    let growing = Body::Mul(vec![
        part(sine(440.0)),
        part(Body::Apply(
            Unary::Exp,
            part(Body::Mul(vec![part(constant(2.0)), part(line())])),
        )),
    ]);
    assert_eq!(blocked(&growing).reason, LeftReason::NotTempered);
}
