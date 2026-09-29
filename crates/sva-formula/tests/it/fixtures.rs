// Concern: the terms every invariant test iterates, and the Env they read node types through | Non-concern: what any one test asserts about them | IO: none -> Vec<(name, ClosedForm)>

// One module, nine suites: each takes the fixtures it needs and leaves the rest.
#![allow(dead_code)]

use std::cell::Cell;

use sva_formula::{
    Body, Bound, C64, ClosedForm, Codomain, Edge, Env, IndexId, ModalBank, Mode, NodeId, Origin,
    ParamId, Part, Rational, Series, Ty, Unary, Var,
};

pub const DUAL: Ty = Ty::form(Var::T, true, Codomain::Real);
pub const IN_T: Ty = Ty::form(Var::T, false, Codomain::Real);

/// Reports one type per node, counts the questions asked, and carries a cache flag that no
/// answer depends on — the instrument `a_cached_render_changes_no_type` reads.
pub struct Fixed {
    pub ty: Ty,
    pub warm_cache: bool,
    pub asked: Cell<u32>,
}

impl Env for Fixed {
    fn node(&self, _: NodeId) -> Ty {
        self.asked.set(self.asked.get() + 1);
        self.ty
    }

    fn param(&self, _: ParamId) -> Ty {
        self.asked.set(self.asked.get() + 1);
        self.ty
    }
}

impl Fixed {
    pub fn holding(ty: Ty, warm_cache: bool) -> Fixed {
        Fixed {
            ty,
            warm_cache,
            asked: Cell::new(0),
        }
    }
}

pub fn part(body: Body) -> Part {
    Part::bare(body)
}

pub fn term(var: Var, body: Body) -> ClosedForm {
    ClosedForm {
        var,
        body,
        origin: Origin::new(0),
    }
}

pub fn constant(x: f64) -> Body {
    Body::Const(C64::real(x))
}

pub fn line() -> Body {
    Body::Line
}

/// `sin(2*pi*hz*t)` and friends: the affine argument every line atom is written from.
pub fn radians(hz: f64) -> Body {
    Body::Mul(vec![
        part(constant(2.0 * std::f64::consts::PI * hz)),
        part(line()),
    ])
}

pub fn sine(hz: f64) -> Body {
    Body::Apply(Unary::Sin, part(radians(hz)))
}

pub fn cosine(hz: f64) -> Body {
    Body::Apply(Unary::Cos, part(radians(hz)))
}

pub fn decay(sigma: f64) -> Body {
    Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![part(constant(sigma)), part(line())])),
    )
}

pub fn causal(of: Body) -> Body {
    Body::Crop {
        of: part(of),
        l: Edge::at(0.0),
        r: Edge::PosInf,
        rise: 0.0,
        fall: 0.0,
    }
}

pub fn gaussian(a: f64, mu: f64) -> Body {
    Body::Shift {
        by: mu,
        of: part(Body::Apply(
            Unary::Exp,
            part(Body::Mul(vec![
                part(constant(-a)),
                part(Body::Pow(part(line()), 2)),
            ])),
        )),
    }
}

pub fn saw_series(hz: f64) -> Body {
    let k = IndexId(0);
    Body::Series(Box::new(Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(
            part(Body::Apply(
                Unary::Sin,
                part(Body::Mul(vec![
                    part(constant(2.0 * std::f64::consts::PI * hz)),
                    part(Body::Index(k)),
                    part(line()),
                ])),
            )),
            part(Body::Index(k)),
        )),
    }))
}

pub fn bank() -> Body {
    Body::Modal(ModalBank {
        modes: vec![Mode {
            omega: 2.0 * std::f64::consts::PI * 220.0,
            tau: 0.8,
            amp: 1.0,
            phase: 0.0,
        }],
        excite: sva_formula::Excitation::Impulse { t0: 0.0 },
    })
}

/// Every written shape the invariants sweep. A fixture earns its place by reaching a row of
/// the rule table no other one reaches.
pub fn terms() -> Vec<(&'static str, ClosedForm)> {
    vec![
        ("a constant", term(Var::T, constant(0.5))),
        ("the free variable", term(Var::T, line())),
        ("a sinusoid", term(Var::T, sine(440.0))),
        ("a cosine", term(Var::T, cosine(440.0))),
        (
            "a product of sinusoids",
            term(Var::T, Body::Mul(vec![part(sine(440.0)), part(sine(3.0))])),
        ),
        (
            "a sum of sinusoids",
            term(
                Var::T,
                Body::Add(vec![part(sine(440.0)), part(sine(660.0))]),
            ),
        ),
        (
            "a Gaussian",
            term(Var::T, gaussian(std::f64::consts::PI, 0.0)),
        ),
        ("a shifted Gaussian", term(Var::T, gaussian(2.0, 0.25))),
        (
            "an impulse",
            term(
                Var::T,
                Body::Delta {
                    at: part(Body::Add(vec![part(line()), part(constant(-1.0))])),
                    order: 0,
                },
            ),
        ),
        (
            "an impulse derivative",
            term(
                Var::T,
                Body::Delta {
                    at: part(line()),
                    order: 2,
                },
            ),
        ),
        (
            "a finite window",
            term(
                Var::T,
                Body::Crop {
                    of: part(constant(1.0)),
                    l: Edge::at(0.0),
                    r: Edge::at(2.0),
                    rise: 0.0,
                    fall: 0.0,
                },
            ),
        ),
        ("a step", term(Var::T, causal(constant(1.0)))),
        (
            "a causal decaying sinusoid",
            term(
                Var::T,
                causal(Body::Mul(vec![part(sine(440.0)), part(decay(-2.0))])),
            ),
        ),
        ("a principal value", term(Var::T, Body::Pv(part(line())))),
        (
            "a second-order rational",
            term(
                Var::F,
                Body::Rational(Rational {
                    zeros: Vec::new(),
                    poles: vec![C64::new(-1.0, 2.0), C64::new(-1.0, -2.0)],
                    gain: C64::ONE,
                }),
            ),
        ),
        ("a band-limited saw", term(Var::T, saw_series(220.0))),
        ("a modal bank", term(Var::T, bank())),
        (
            "a shouldered window",
            term(
                Var::T,
                Body::Crop {
                    of: part(sine(440.0)),
                    l: Edge::at(0.0),
                    r: Edge::at(1.0),
                    rise: 0.01,
                    fall: 0.05,
                },
            ),
        ),
        (
            "a stereo join",
            term(
                Var::T,
                Body::Join(vec![part(sine(440.0)), part(sine(660.0))]),
            ),
        ),
    ]
}
