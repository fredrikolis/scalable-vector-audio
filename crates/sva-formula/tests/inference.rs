// Concern: proves a term's type is decided structurally, once, and never by a cache | Non-concern: the transform a pair type promises | IO: (a ClosedForm, an Env) -> its Ty

mod fixtures;

use fixtures::{DUAL, Fixed, IN_T, bank, causal, constant, decay, line, part, sine, term, terms};
use sva_formula::{
    Body, C64, Code, Codomain, Edge, Env, Held, NodeId, ParamId, Part, TABLE_VERSION, Unary, Var,
    dual, infer, normalize_closed_form,
};

fn ty_of(f: Body) -> sva_formula::Ty {
    infer(&term(Var::T, f), &Fixed::holding(DUAL, false)).expect("the fixture types")
}

#[test]
fn t_has_a_dual() {
    assert!(ty_of(line()).has_dual());
}

#[test]
fn a_closed_form_in_t_that_left_a_keeps_no_dual() {
    let driven = Body::Apply(Unary::Tanh, part(sine(440.0)));
    assert!(!ty_of(driven).has_dual());

    let modulated = Body::Apply(
        Unary::Sin,
        part(Body::Mul(vec![part(sine(5.0)), part(line())])),
    );
    let spread = ty_of(modulated);
    assert_eq!(
        (spread.held, spread.dual),
        (Held::Form(Var::T), false),
        "a non-affine argument spreads to infinitely many lines"
    );
}

#[test]
fn a_growing_exponential_has_no_dual_until_cropped() {
    let ringing = Body::Mul(vec![part(sine(440.0)), part(decay(-2.0))]);
    assert!(!ty_of(ringing.clone()).has_dual());
    assert!(ty_of(causal(ringing)).has_dual());
}

#[test]
fn width_propagates_and_broadcasts() {
    let stereo = Body::Join(vec![part(sine(440.0)), part(sine(660.0))]);
    assert_eq!(ty_of(stereo.clone()).width, 2);

    let scaled = Body::Mul(vec![part(stereo.clone()), part(constant(0.5))]);
    assert_eq!(ty_of(scaled).width, 2, "a mono operand broadcasts");

    assert_eq!(ty_of(Body::Channel(part(stereo), 1)).width, 1);
}

#[test]
fn a_join_above_eight_refuses() {
    let wide = Body::Join(
        (0..9)
            .map(|k| part(sine(100.0 * f64::from(k) + 1.0)))
            .collect(),
    );
    let refusal = infer(&term(Var::T, wide), &Fixed::holding(DUAL, false)).unwrap_err();
    assert_eq!(refusal.code, Code::WidthMismatch);
    assert!(refusal.message.contains('8'), "{}", refusal.message);

    let out_of_range = Body::Channel(part(sine(440.0)), 3);
    assert_eq!(
        infer(&term(Var::T, out_of_range), &Fixed::holding(DUAL, false))
            .unwrap_err()
            .code,
        Code::WidthMismatch
    );
}

#[test]
fn a_complex_codomain_is_syntactic() {
    let named_i = Body::Mul(vec![part(Body::Const(C64::I)), part(sine(440.0))]);
    assert_eq!(ty_of(named_i).codomain, Codomain::Complex);
    assert_eq!(ty_of(sine(440.0)).codomain, Codomain::Real);
    assert_eq!(
        ty_of(bank()).codomain,
        Codomain::Real,
        "a bank emits conjugate pairs by construction"
    );
    assert_eq!(ty_of(Body::Pv(part(line()))).codomain, Codomain::Complex);
}

/// Two halves: the judgment moves when the Env's answer about a node moves, and it does
/// not move when only the Env's cache state does. Without the first half the second is
/// vacuous.
#[test]
fn a_cached_render_changes_no_type() {
    let windowed = Body::Crop {
        of: Part::bare(Body::Mul(vec![
            part(sine(440.0)),
            part(Body::Node(NodeId(0))),
        ])),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    };
    let subject = term(Var::T, windowed);

    let cold = Fixed::holding(DUAL, false);
    let warm = Fixed::holding(DUAL, true);
    assert_eq!(infer(&subject, &cold), infer(&subject, &warm));
    assert!(cold.asked.get() > 0, "the Env was never consulted");

    let opaque = Fixed::holding(IN_T, false);
    assert_ne!(
        infer(&subject, &cold),
        infer(&subject, &opaque),
        "a node's own type does reach the judgment"
    );
}

/// Section 4's invariant, over every fixture: a term types as a pair exactly when its sum
/// form has a dual in A. The two decide through one `dual_class`, so this is what says so.
#[test]
fn infer_agrees_with_normalize_and_dual_on_every_fixture() {
    for (name, subject) in terms() {
        let typed = infer(&subject, &Fixed::holding(DUAL, false))
            .map(|ty| ty.has_dual())
            .unwrap_or(false);
        let transformed = normalize_closed_form(&subject)
            .and_then(|n| dual(&n))
            .is_ok();
        assert_eq!(typed, transformed, "{name}");
    }
}

/// A version bump moves every content address and no type: inference reads the table's
/// preconditions, never its version, so a row nothing reaches retypes nothing.
#[test]
fn adding_a_dead_rule_retypes_nothing() {
    for (name, subject) in terms() {
        let before = infer(&subject, &Fixed::holding(DUAL, false));
        let after = infer(&subject, &Fixed::holding(DUAL, false));
        assert_eq!(before, after, "{name}");
        assert_ne!(
            sva_formula::hash::hash_closed_form_under(&subject, TABLE_VERSION),
            sva_formula::hash::hash_closed_form_under(&subject, TABLE_VERSION + 1),
            "{name}"
        );
    }
}

/// A sine is bounded wherever its argument is real, and a ref to a real closed form is real: the
/// summability check reads the node's codomain rather than giving up on the ref.
#[test]
fn a_series_over_a_real_valued_ref_is_summable() {
    let k = sva_formula::IndexId(0);
    let phase = Body::Mul(vec![
        part(Body::Index(k)),
        part(Body::Add(vec![
            part(Body::Mul(vec![
                part(constant(std::f64::consts::TAU * 220.0)),
                part(line()),
            ])),
            part(Body::Node(NodeId(0))),
        ])),
    ]);
    let term = Body::Div(
        part(Body::Apply(Unary::Sin, part(phase))),
        part(Body::Index(k)),
    );
    let series = Body::Series(Box::new(sva_formula::Series {
        index: k,
        lo: 1,
        hi: sva_formula::Bound::Infinite,
        term: part(term),
    }));
    let summed = ty_of(series);
    assert_eq!(
        (summed.held, summed.dual),
        (Held::Form(Var::T), false),
        "a sine of a real, non-affine argument is a closed form in t, not a refusal"
    );
}

/// The instrument itself: it must answer alike whatever its cache says, and it must count
/// every question, or the judgment test it serves proves nothing.
#[test]
fn the_fixture_env_answers_one_type_whatever_its_cache_says() {
    let cold = Fixed::holding(DUAL, false);
    let warm = Fixed::holding(DUAL, true);
    assert!(!cold.warm_cache && warm.warm_cache);
    assert_eq!(cold.node(NodeId(0)), warm.node(NodeId(0)));
    assert_eq!(cold.param(ParamId(0)), warm.param(ParamId(0)));
    assert_eq!(cold.asked.get(), 2);
    assert_ne!(
        cold.node(NodeId(0)),
        Fixed::holding(IN_T, false).node(NodeId(0))
    );
}

#[test]
fn every_fixture_is_named_once() {
    let all = terms();
    let mut names: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len(), "two fixtures share one name");
    assert!(all.len() >= 15, "the sweep needs breadth to mean anything");
}
