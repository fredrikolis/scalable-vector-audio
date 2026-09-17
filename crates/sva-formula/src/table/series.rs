// Concern: duals a series termwise, between a symbolic line and a symbolic delta | Non-concern: how many lines a series yields at a rate (series.rs) | IO: (&Series) -> Series or Left

use std::f64::consts::TAU;

use crate::closed_form::{Body, Part, Series, Unary};
use crate::complex::C64;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::series::mentions_line;

/// `amp * exp(2*pi*i*freq*u)`, with both written as index expressions rather than numbers.
pub struct SymLine {
    pub amp: Body,
    pub freq: Body,
}

/// `weight * delta(u - at)`.
pub struct SymDelta {
    pub weight: Body,
    pub at: Body,
}

pub enum Shape {
    Lines(Vec<SymLine>),
    Deltas(Vec<SymDelta>),
}

pub(crate) fn dual(s: &Series) -> Result<Series, Left> {
    transform(s, dual_shape)
}

pub(crate) fn reflect(s: &Series) -> Result<Series, Left> {
    transform(s, reflect_shape)
}

fn transform(s: &Series, op: impl Fn(Shape) -> Shape) -> Result<Series, Left> {
    let Some(shape) = read(&s.term.body) else {
        return Err(Left::new(
            s.term.origin,
            AtomSketch::of(Factor::Value),
            LeftReason::SeriesNonUniform,
        ));
    };
    Ok(Series {
        term: Part::new(s.term.origin, write(&op(shape))),
        ..s.clone()
    })
}

fn dual_shape(shape: Shape) -> Shape {
    match shape {
        Shape::Lines(lines) => Shape::Deltas(
            lines
                .into_iter()
                .map(|l| SymDelta {
                    weight: l.amp,
                    at: l.freq,
                })
                .collect(),
        ),
        Shape::Deltas(deltas) => Shape::Lines(
            deltas
                .into_iter()
                .map(|d| SymLine {
                    amp: d.weight,
                    freq: negate(d.at),
                })
                .collect(),
        ),
    }
}

fn reflect_shape(shape: Shape) -> Shape {
    match shape {
        Shape::Lines(lines) => Shape::Lines(
            lines
                .into_iter()
                .map(|l| SymLine {
                    freq: negate(l.freq),
                    ..l
                })
                .collect(),
        ),
        Shape::Deltas(deltas) => Shape::Deltas(
            deltas
                .into_iter()
                .map(|d| SymDelta {
                    at: negate(d.at),
                    ..d
                })
                .collect(),
        ),
    }
}

pub(crate) fn write(shape: &Shape) -> Body {
    let parts: Vec<Part> = match shape {
        Shape::Lines(lines) => lines
            .iter()
            .map(|l| {
                Part::bare(Body::Mul(vec![
                    Part::bare(l.amp.clone()),
                    Part::bare(Body::Apply(
                        Unary::Exp,
                        Part::bare(Body::Mul(vec![
                            Part::bare(Body::Const(C64::new(0.0, TAU))),
                            Part::bare(l.freq.clone()),
                            Part::bare(Body::Line),
                        ])),
                    )),
                ]))
            })
            .collect(),
        Shape::Deltas(deltas) => deltas
            .iter()
            .map(|d| {
                Part::bare(Body::Mul(vec![
                    Part::bare(d.weight.clone()),
                    Part::bare(Body::Delta {
                        at: Part::bare(Body::Add(vec![
                            Part::bare(Body::Line),
                            Part::bare(negate(d.at.clone())),
                        ])),
                        order: 0,
                    }),
                ]))
            })
            .collect(),
    };
    Body::Add(parts)
}

pub fn read(term: &Body) -> Option<Shape> {
    read_deltas(term)
        .map(Shape::Deltas)
        .or_else(|| read_lines(term).map(Shape::Lines))
}

fn read_deltas(term: &Body) -> Option<Vec<SymDelta>> {
    match term {
        Body::Add(parts) => {
            let mut out = Vec::new();
            for p in parts {
                out.extend(read_deltas(&p.body)?);
            }
            Some(out)
        }
        other => read_delta(other).map(|d| vec![d]),
    }
}

fn read_delta(term: &Body) -> Option<SymDelta> {
    let (weight, singular) = match term {
        Body::Mul(parts) => match parts.as_slice() {
            [weight, singular] => ((*weight.body).clone(), &*singular.body),
            _ => return None,
        },
        // A strike written without a coefficient still weighs one.
        Body::Delta { .. } => (Body::Const(C64::ONE), term),
        _ => return None,
    };
    let Body::Delta { at, order: 0 } = singular else {
        return None;
    };
    let Body::Add(offset) = &*at.body else {
        return None;
    };
    let [line, shifted] = offset.as_slice() else {
        return None;
    };
    matches!(*line.body, Body::Line).then(|| SymDelta {
        weight,
        at: negate((*shifted.body).clone()),
    })
}

fn read_lines(term: &Body) -> Option<Vec<SymLine>> {
    read_wave(term, Body::Const(C64::ONE))
}

/// Peels the index-only factors off a term until one wave in the free variable is left.
fn read_wave(term: &Body, coeff: Body) -> Option<Vec<SymLine>> {
    match term {
        // Written associativity must not decide whether a series reads.
        Body::Add(parts) => {
            let mut out = Vec::new();
            for p in parts {
                out.extend(read_wave(&p.body, coeff.clone())?);
            }
            Some(out)
        }
        Body::Mul(parts) => {
            let mut core = None;
            let mut acc = coeff;
            for p in parts {
                if mentions_line(&p.body) {
                    if core.is_some() {
                        return None;
                    }
                    core = Some((*p.body).clone());
                } else {
                    acc = product(acc, (*p.body).clone());
                }
            }
            read_wave(&core?, acc)
        }
        // A shift written round a term is a shift of the term's own free variable.
        Body::Shift { by, of } => read_wave(&crate::closed_form::shift_line(&of.body, *by), coeff),
        Body::Div(num, den) if !mentions_line(&den.body) => read_wave(
            &num.body,
            Body::Div(Part::bare(coeff), Part::bare((*den.body).clone())),
        ),
        Body::Apply(op, arg) => {
            let (slope, phase) = split_argument(&arg.body)?;
            Some(match op {
                Unary::Exp => vec![SymLine {
                    amp: product(coeff, exponential(phase)),
                    freq: divide(slope, C64::new(0.0, TAU)),
                }],
                Unary::Cos => quadrature(coeff, slope, phase, C64::real(0.5), C64::real(0.5)),
                Unary::Sin => {
                    quadrature(coeff, slope, phase, C64::new(0.0, -0.5), C64::new(0.0, 0.5))
                }
                _ => return None,
            })
        }
        _ => None,
    }
}

fn quadrature(coeff: Body, slope: Body, phase: Body, up: C64, down: C64) -> Vec<SymLine> {
    let turn = Body::Mul(vec![
        Part::bare(Body::Const(C64::I)),
        Part::bare(phase.clone()),
    ]);
    let back = Body::Mul(vec![Part::bare(Body::Const(-C64::I)), Part::bare(phase)]);
    let freq = divide(slope, C64::real(TAU));
    vec![
        SymLine {
            amp: product(product(coeff.clone(), Body::Const(up)), exponential(turn)),
            freq: freq.clone(),
        },
        SymLine {
            amp: product(product(coeff, Body::Const(down)), exponential(back)),
            freq: negate(freq),
        },
    ]
}

/// `slope * u + phase`, both index-only, or nothing.
fn split_argument(arg: &Body) -> Option<(Body, Body)> {
    match arg {
        Body::Add(parts) => {
            let mut slope = None;
            let mut phase = Body::Const(C64::ZERO);
            for p in parts {
                if mentions_line(&p.body) {
                    if slope.is_some() {
                        return None;
                    }
                    // A shifted series writes `(u - by) - k*d`: both offsets are phase.
                    slope = Some(match strip_line(&p.body) {
                        Some(found) => found,
                        None => {
                            let (found, offset) = split_argument(&p.body)?;
                            phase = sum(phase, offset);
                            found
                        }
                    });
                } else {
                    phase = sum(phase, (*p.body).clone());
                }
            }
            Some((slope?, phase))
        }
        Body::Mul(parts) => {
            let mut coeff = Body::Const(C64::ONE);
            let mut core = None;
            for p in parts {
                if mentions_line(&p.body) {
                    if core.is_some() {
                        return None;
                    }
                    core = Some((*p.body).clone());
                } else {
                    coeff = product(coeff, (*p.body).clone());
                }
            }
            let (slope, phase) = split_argument(&core?)?;
            Some((product(coeff.clone(), slope), product(coeff, phase)))
        }
        other => Some((strip_line(other)?, Body::Const(C64::ZERO))),
    }
}

/// A shifted argument reaches here as `coeff*(t + offset)`, so the coefficient is peeled
/// before the sum inside it is read.
/// The coefficient of the free variable in a product that names it exactly once.
fn strip_line(f: &Body) -> Option<Body> {
    match f {
        Body::Line => Some(Body::Const(C64::ONE)),
        Body::Mul(parts) => {
            let mut rest = Vec::new();
            let mut seen = false;
            for p in parts {
                match &*p.body {
                    Body::Line if !seen => seen = true,
                    body if mentions_line(body) => return None,
                    body => rest.push(Part::bare(body.clone())),
                }
            }
            seen.then(|| collapse(rest))
        }
        _ => None,
    }
}

fn collapse(mut parts: Vec<Part>) -> Body {
    match parts.len() {
        0 => Body::Const(C64::ONE),
        1 => (*parts.remove(0).body).clone(),
        _ => Body::Mul(parts),
    }
}

fn product(a: Body, b: Body) -> Body {
    match (&a, &b) {
        (Body::Const(x), _) if *x == C64::ONE => b,
        (_, Body::Const(x)) if *x == C64::ONE => a,
        _ => Body::Mul(vec![Part::bare(a), Part::bare(b)]),
    }
}

fn sum(a: Body, b: Body) -> Body {
    match &a {
        Body::Const(x) if x.is_zero() => b,
        _ => Body::Add(vec![Part::bare(a), Part::bare(b)]),
    }
}

fn exponential(argument: Body) -> Body {
    match &argument {
        Body::Const(c) if c.is_zero() => Body::Const(C64::ONE),
        _ => Body::Apply(Unary::Exp, Part::bare(argument)),
    }
}

/// Folding the factor away where it is already written keeps `dual` of `dual` structurally
/// equal to `reflect` rather than merely equal in value.
fn divide(f: Body, by: C64) -> Body {
    if let Body::Mul(parts) = &f
        && let Some(at) = parts
            .iter()
            .position(|p| matches!(&*p.body, Body::Const(c) if *c == by))
    {
        let mut rest = parts.clone();
        rest.remove(at);
        return collapse(rest);
    }
    Body::Div(Part::bare(f), Part::bare(Body::Const(by)))
}

/// An involution, so a frequency negated twice is the formula it started as.
fn negate(f: Body) -> Body {
    if let Body::Mul(parts) = &f
        && let Some(at) = parts
            .iter()
            .position(|p| matches!(&*p.body, Body::Const(c) if *c == C64::real(-1.0)))
    {
        let mut rest = parts.clone();
        rest.remove(at);
        return collapse(rest);
    }
    Body::Mul(vec![
        Part::bare(Body::Const(C64::real(-1.0))),
        Part::bare(f),
    ])
}
