// Concern: evaluates one closed form at one instant and routes the components it holds | Non-concern: what the samples are labelled (collapse.rs) | IO: (&SpectralSum or &Body, t, component) -> C64

use sva_formula::closed_form::{Fold, Unary, children};
use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{Body, C64, Lane, NodeId, SpectralSum};

use crate::error::CollapseError;

/// How a `Body::Node` reaches a value at one instant, and how many components it holds: a
/// closed form on its own holds no node, so the caller holding the graph answers both.
pub trait Refs {
    fn value(&self, id: NodeId, component: usize, t: f64) -> Result<C64, CollapseError>;
    fn width(&self, id: NodeId) -> usize;
}

/// What a caller with no graph behind it answers a node with.
pub(crate) struct NoRefs;

impl Refs for NoRefs {
    fn value(&self, _: NodeId, _: usize, _: f64) -> Result<C64, CollapseError> {
        Err(CollapseError::NotEvaluable("a node"))
    }

    fn width(&self, _: NodeId) -> usize {
        1
    }
}

/// The six atom factors in closed form. A delta has no ordinary value, and a principal
/// value has none at its own pole.
pub fn eval_atom(a: &SpectralAtom, t: f64) -> Result<C64, CollapseError> {
    if let Singular::Delta { at, order } = a.sing {
        return Err(CollapseError::SingularInCt {
            at,
            order: i32::from(order),
        });
    }
    a.smooth_at(t).ok_or(CollapseError::SingularInCt {
        at: a.pole.map_or(t, |p| p.at.re),
        order: -1,
    })
}

pub fn eval_lane(lane: &Lane, t: f64) -> Result<C64, CollapseError> {
    let mut sum = C64::ZERO;
    for a in &lane.atoms {
        sum = sum + eval_atom(a, t)?;
    }
    for bank in &lane.modal {
        for a in sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN) {
            sum = sum + eval_atom(&a, t)?;
        }
    }
    Ok(sum)
}

pub fn eval_spectral_sum(n: &SpectralSum, c: usize, t: f64) -> Result<C64, CollapseError> {
    eval_lane(&n.lanes[c.min(n.lanes.len() - 1)], t)
}

/// The fallback for a closed form with no spectral sum at all: `tanh`, `sat`, `abs`, `log`, `sqrt`,
/// a non-integer power, a non-affine `sin`. Nothing here is claimed exact.
pub fn eval_body(
    fm: &Body,
    component: usize,
    t: f64,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    let of = |p: &sva_formula::Part| eval_body(&p.body, component, t, refs);
    let value = match fm {
        Body::Const(c) => *c,
        Body::Line => C64::real(t),
        Body::Add(parts) => parts.iter().try_fold(C64::ZERO, |a, p| Ok(a + of(p)?))?,
        Body::Mul(parts) => parts.iter().try_fold(C64::ONE, |a, p| Ok(a * of(p)?))?,
        Body::Div(a, b) => of(a)? / of(b)?,
        Body::Pow(a, n) => power(of(a)?, *n),
        Body::Apply(op, a) => unary(*op, of(a)?),
        Body::Fold(op, parts) => fold(*op, parts, component, t, refs)?,
        Body::Shift { by, of: inner } => eval_body(&inner.body, component, t - by, refs)?,
        Body::Warp { at, of: inner } => {
            let when = eval_body(&at.body, component, t, refs)?.re;
            eval_body(&inner.body, component, when, refs)?
        }
        Body::Crop {
            of: inner,
            l,
            r,
            rise,
            fall,
        } => match raised_cosine(t, l.value(), r.value(), *rise, *fall) {
            0.0 => C64::ZERO,
            gain => of(inner)?.scale(gain),
        },
        Body::Channel(inner, k) => eval_body(&inner.body, usize::from(*k), t, refs)?,
        Body::Delta { order, .. } => {
            return Err(CollapseError::SingularInCt {
                at: t,
                order: i32::from(*order),
            });
        }
        Body::Pv(_) => return Err(CollapseError::SingularInCt { at: t, order: -1 }),
        Body::Keyed { seed, of: key } => C64::real(sva_formula::draw(*seed, of(key)?.re)),
        Body::Join(parts) => {
            let widths: Vec<usize> = parts.iter().map(|p| width_of(&p.body, refs)).collect();
            let (at, inner) = lane_of(&widths, component)
                .ok_or(CollapseError::NotEvaluable("a component past the width"))?;
            eval_body(&parts[at].body, inner, t, refs)?
        }
        Body::Modal(bank) => {
            let mut sum = C64::ZERO;
            for a in sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN) {
                sum = sum + eval_atom(&a, t)?;
            }
            sum
        }
        Body::Node(id) => refs.value(*id, component, t)?,
        other => return Err(CollapseError::NotEvaluable(sketch(other))),
    };
    match value.is_finite() {
        true => Ok(value),
        false => Err(CollapseError::NotEvaluable(
            "a division or a remainder by zero",
        )),
    }
}

/// FORMAT 15.6's window: one over the plateau, a raised cosine over each shoulder, zero out.
fn raised_cosine(t: f64, l: f64, r: f64, rise: f64, fall: f64) -> f64 {
    if t < l || t >= r {
        return 0.0;
    }
    let opening = shoulder(t - l, rise);
    let closing = shoulder(r - t, fall);
    opening.min(closing)
}

fn shoulder(into: f64, span: f64) -> f64 {
    if span <= 0.0 || into >= span {
        return 1.0;
    }
    0.5 - 0.5 * (std::f64::consts::PI * into / span).cos()
}

/// Which operand of a `join` holds one component of the joined value, and which of its own
/// components that is.
pub fn lane_of(widths: &[usize], component: usize) -> Option<(usize, usize)> {
    let mut left = component;
    for (at, width) in widths.iter().enumerate() {
        if left < *width {
            return Some((at, left));
        }
        left -= width;
    }
    None
}

/// The component count a written subterm answers for. Only `join` widens, and only `ch` and
/// a node narrow back.
pub(crate) fn width_of(f: &Body, refs: &dyn Refs) -> usize {
    match f {
        Body::Join(parts) => parts.iter().map(|p| width_of(&p.body, refs)).sum(),
        Body::Channel(..) => 1,
        Body::Node(id) => refs.width(*id).max(1),
        other => children(other)
            .iter()
            .map(|p| width_of(&p.body, refs))
            .max()
            .unwrap_or(1),
    }
}

fn power(x: C64, n: i32) -> C64 {
    match n {
        0.. => x.powi(n as u32),
        _ => x.powi(n.unsigned_abs()).inv(),
    }
}

pub fn unary(op: Unary, x: C64) -> C64 {
    match op {
        Unary::Exp => x.exp(),
        Unary::Sin => C64::new(x.re.sin() * x.im.cosh(), x.re.cos() * x.im.sinh()),
        Unary::Cos => C64::new(x.re.cos() * x.im.cosh(), -x.re.sin() * x.im.sinh()),
        Unary::Tanh => C64::real(x.re.tanh()),
        Unary::Sat => C64::real(x.re.clamp(-1.0, 1.0)),
        Unary::Abs => C64::real(x.abs()),
        Unary::Log => C64::real(x.re.ln()),
        Unary::Sqrt => C64::real(x.re.sqrt()),
    }
}

fn fold(
    op: Fold,
    parts: &[sva_formula::Part],
    component: usize,
    t: f64,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    let mut it = parts.iter();
    let head = it.next().expect("a fold holds one part");
    let first = eval_body(&head.body, component, t, refs)?;
    it.try_fold(first, |acc, p| {
        let v = eval_body(&p.body, component, t, refs)?;
        Ok(C64::real(match op {
            Fold::Max => acc.re.max(v.re),
            Fold::Min => acc.re.min(v.re),
            Fold::Mod => acc.re.rem_euclid(v.re),
        }))
    })
}

fn sketch(f: &Body) -> &'static str {
    match f {
        Body::Param(_) => "an unsubstituted parameter",
        Body::Index(_) => "a free series index",
        Body::Deriv { .. } => "a derivative",
        Body::Rational(_) => "a rational",
        Body::Series(_) => "a series",
        _ => "this subterm",
    }
}
