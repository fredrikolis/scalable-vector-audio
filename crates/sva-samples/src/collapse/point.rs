// Concern: evaluates one closed form at one instant and routes the components it holds | Non-concern: what the samples are labelled (blocks.rs) | IO: (&SpectralSum or &Body, t, component) -> C64

use std::cell::RefCell;

use sva_formula::closed_form::{Fold, Unary, children};
use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{Banded, Body, C64, IndexId, Lane, NodeId, SpectralSum};

use crate::error::CollapseError;
use crate::grid::Grid;

/// How a `Body::Node` reaches a value at one instant, and how many components it holds: a
/// closed form on its own holds no node, so the caller holding the graph answers both.
pub trait Refs {
    fn value(&self, id: NodeId, component: usize, at: At) -> Result<C64, CollapseError>;
    fn width(&self, id: NodeId) -> usize;
}

/// Where a form is evaluated: a free instant, or a grid sample, whose edges the grid decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum At {
    Free(f64),
    Sample(Grid, i64),
}

impl At {
    pub fn t(self) -> f64 {
        match self {
            At::Free(t) => t,
            At::Sample(grid, n) => grid.instant(n),
        }
    }

    fn sample(self) -> Option<(Grid, i64)> {
        match self {
            At::Free(_) => None,
            At::Sample(grid, n) => Some((grid, n)),
        }
    }
}

/// The sample an instant inside an evaluation stands at, where it still is one.
type On = Option<(Grid, i64)>;

type Evaluated = (usize, u64, On, C64);

/// The forms a shared written form reads, each evaluated once per instant and component.
pub(crate) struct Shared<'a> {
    bodies: &'a [Body],
    held: RefCell<Vec<Option<Evaluated>>>,
}

impl<'a> Shared<'a> {
    pub(crate) fn new(bodies: &'a [Body]) -> Shared<'a> {
        Shared {
            bodies,
            held: RefCell::new(vec![None; bodies.len()]),
        }
    }
}

impl Refs for Shared<'_> {
    fn value(&self, id: NodeId, component: usize, at: At) -> Result<C64, CollapseError> {
        let Some(body) = self.bodies.get(id.0 as usize) else {
            return NoRefs.value(id, component, at);
        };
        let key = (component, at.t().to_bits(), at.sample());
        if let Some((c, bits, on, value)) = self.held.borrow()[id.0 as usize]
            && (c, bits, on) == key
        {
            return Ok(value);
        }
        let value = eval_body_on(body, component, at, self)?;
        self.held.borrow_mut()[id.0 as usize] = Some((key.0, key.1, key.2, value));
        Ok(value)
    }

    fn width(&self, id: NodeId) -> usize {
        match self.bodies.get(id.0 as usize) {
            Some(body) => width_of(body, self),
            None => NoRefs.width(id),
        }
    }
}

/// What a caller with no graph behind it answers a node with.
pub(crate) struct NoRefs;

impl Refs for NoRefs {
    fn value(&self, _: NodeId, _: usize, _: At) -> Result<C64, CollapseError> {
        Err(CollapseError::NotEvaluable("a node"))
    }

    fn width(&self, _: NodeId) -> usize {
        1
    }
}

/// The six atom factors in closed form. A delta has no ordinary value, and a principal
/// value has none at its own pole.
pub(crate) fn eval_atom_on(a: &SpectralAtom, t: f64, on: On) -> Result<C64, CollapseError> {
    debug_assert!(
        on.is_none_or(|(grid, n)| grid.instant(n) == t),
        "a sample at its instant"
    );
    if let Singular::Delta { at, order } = a.sing {
        return Err(CollapseError::SingularInCt {
            at,
            order: i32::from(order),
        });
    }
    let held = match (a.ind, on) {
        (Some(ind), Some((grid, n))) => match grid.inside(n, ind.l.value(), ind.r.value()) {
            true => a.smooth_inside(t),
            false => Some(C64::ZERO),
        },
        _ => a.smooth_at(t),
    };
    held.ok_or(CollapseError::SingularInCt {
        at: a.pole.map_or(t, |p| p.at.re),
        order: -1,
    })
}

fn eval_lane_on(lane: &Lane, t: f64, on: On) -> Result<C64, CollapseError> {
    let mut sum = C64::ZERO;
    for a in &lane.atoms {
        sum = sum + eval_atom_on(a, t, on)?;
    }
    with_modal(lane, sum, t, on)
}

/// Every atom `live` leaves out is exactly zero at sample `m`.
pub(crate) fn eval_among(
    lane: &Lane,
    live: &[usize],
    (grid, m): (Grid, i64),
) -> Result<C64, CollapseError> {
    let (t, on) = (grid.instant(m), Some((grid, m)));
    let mut sum = C64::ZERO;
    for &i in live {
        sum = sum + eval_atom_on(&lane.atoms[i], t, on)?;
    }
    with_modal(lane, sum, t, on)
}

fn with_modal(lane: &Lane, mut sum: C64, t: f64, on: On) -> Result<C64, CollapseError> {
    for bank in &lane.modal {
        for a in sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN) {
            sum = sum + eval_atom_on(&a, t, on)?;
        }
    }
    Ok(sum)
}

pub(crate) fn eval_spectral_sum(n: &SpectralSum, c: usize, t: f64) -> Result<C64, CollapseError> {
    eval_spectral_sum_on(n, c, At::Free(t))
}

pub(crate) fn eval_spectral_sum_on(
    n: &SpectralSum,
    c: usize,
    at: At,
) -> Result<C64, CollapseError> {
    eval_lane_on(&n.lanes[c.min(n.lanes.len() - 1)], at.t(), at.sample())
}

/// A shut crop zeroes a factor beside it that passes a double.
fn product(
    factors: impl Iterator<Item = Result<C64, CollapseError>>,
) -> Result<C64, CollapseError> {
    let mut held = Ok(C64::ONE);
    for factor in factors {
        match (factor, &held) {
            (Ok(v), _) if v.is_zero() => return Ok(C64::ZERO),
            (Ok(v), Ok(acc)) => held = Ok(*acc * v),
            (Err(e), Ok(_)) => held = Err(e),
            (_, Err(_)) => {}
        }
    }
    held
}

/// The fallback for a closed form with no spectral sum at all: `tanh`, `sat`, `abs`, `log`, `sqrt`,
/// a non-integer power, a non-affine `sin`. Nothing here is claimed exact.
pub(crate) fn eval_body_on(
    fm: &Body,
    component: usize,
    at: At,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    eval_at(fm, component, at.t(), (refs, &[], at.sample()))
}

/// Each index a banded series binds, at its value for the term summed, and `t`'s sample.
type Bound<'a> = (&'a dyn Refs, &'a [(IndexId, f64)], On);

fn eval_at(fm: &Body, component: usize, t: f64, at: Bound) -> Result<C64, CollapseError> {
    let (refs, bound, on) = at;
    let moved = (refs, bound, None);
    let of = |p: &sva_formula::Part| eval_at(&p.body, component, t, at);
    let value = match fm {
        Body::Const(c) => *c,
        Body::Line => C64::real(t),
        Body::Add(parts) => parts.iter().try_fold(C64::ZERO, |a, p| Ok(a + of(p)?))?,
        Body::Mul(parts) => product(parts.iter().map(of))?,
        Body::Div(a, b) => of(a)? / of(b)?,
        Body::Pow(a, n) => power(of(a)?, *n),
        Body::Apply(op, a) => unary(*op, of(a)?),
        Body::Fold(op, parts) => fold(*op, parts, component, t, at)?,
        Body::Shift { by, of: inner } => eval_at(&inner.body, component, t - by, moved)?,
        Body::Warp {
            at: when,
            of: inner,
        } => {
            let when = eval_at(&when.body, component, t, at)?.re;
            eval_at(&inner.body, component, when, moved)?
        }
        Body::Crop {
            of: inner,
            l,
            r,
            rise,
            fall,
        } => {
            let (l, r) = (l.value(), r.value());
            let gain = match on {
                Some((grid, n)) if grid.inside(n, l, r) => shoulders(t, l, r, *rise, *fall),
                Some(_) => 0.0,
                None => crop_gain(t, l, r, *rise, *fall),
            };
            match gain {
                0.0 => C64::ZERO,
                gain => of(inner)?.scale(gain),
            }
        }
        Body::Channel(inner, k) => eval_at(&inner.body, usize::from(*k), t, at)?,
        Body::Delta { order, .. } => {
            return Err(CollapseError::SingularInCt {
                at: t,
                order: i32::from(*order),
            });
        }
        Body::Pv(_) => return Err(CollapseError::SingularInCt { at: t, order: -1 }),
        Body::Keyed { seed, of: key } => C64::real(
            sva_formula::draw_nearest(*seed, of(key)?.re)
                .ok_or(CollapseError::NotEvaluable("a key past any step"))?,
        ),
        Body::Join(parts) => {
            let widths: Vec<usize> = parts.iter().map(|p| width_of(&p.body, refs)).collect();
            let (lane, inner) = lane_of(&widths, component)
                .ok_or(CollapseError::NotEvaluable("a component past the width"))?;
            eval_at(&parts[lane].body, inner, t, at)?
        }
        Body::Modal(bank) => {
            let mut sum = C64::ZERO;
            for a in sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN) {
                sum = sum + eval_atom_on(&a, t, on)?;
            }
            sum
        }
        Body::Node(id) => refs.value(
            *id,
            component,
            on.map_or(At::Free(t), |(g, n)| At::Sample(g, n)),
        )?,
        Body::Run(run) => super::run::at(run, t),
        Body::Index(i) if let Some((_, k)) = bound.iter().find(|(j, _)| j == i) => C64::real(*k),
        Body::Banded(b) => banded(b, component, t, at)?,
        other => return Err(CollapseError::NotEvaluable(sketch(other))),
    };
    finite(value)
}

/// `eval_body`'s sum from +0 over only `live`: one left out is exact zero at `t`.
pub(crate) fn eval_addends(
    parts: &[&sva_formula::Part],
    live: &[usize],
    component: usize,
    (grid, n): (Grid, i64),
) -> Result<C64, CollapseError> {
    let sum = live.iter().try_fold(C64::ZERO, |held, &i| {
        Ok(held + eval_body_on(&parts[i].body, component, At::Sample(grid, n), &NoRefs)?)
    })?;
    finite(sum)
}

fn finite(value: C64) -> Result<C64, CollapseError> {
    match value.is_finite() {
        true => Ok(value),
        false => Err(CollapseError::NotEvaluable(
            "a division or a remainder by zero, or an infinite value",
        )),
    }
}

/// FORMAT 15.6's window: one over the plateau, a raised cosine over each shoulder, zero out.
pub fn crop_gain(t: f64, l: f64, r: f64, rise: f64, fall: f64) -> f64 {
    if t < l || t >= r {
        return 0.0;
    }
    shoulders(t, l, r, rise, fall)
}

/// The window's gain at an instant already known to lie inside it.
pub fn shoulders(t: f64, l: f64, r: f64, rise: f64, fall: f64) -> f64 {
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
        Unary::Step => C64::real(sva_formula::affine::step(x.re)),
    }
}

fn fold(
    op: Fold,
    parts: &[sva_formula::Part],
    component: usize,
    t: f64,
    at: Bound,
) -> Result<C64, CollapseError> {
    let mut it = parts.iter();
    let head = it.next().expect("a fold holds one part");
    let first = eval_at(&head.body, component, t, at)?;
    it.try_fold(first, |acc, p| {
        let v = eval_at(&p.body, component, t, at)?;
        Ok(C64::real(match op {
            Fold::Max => acc.re.max(v.re),
            Fold::Min => acc.re.min(v.re),
            Fold::Mod => acc.re.rem_euclid(v.re),
        }))
    })
}

/// The terms whose carrier turns under the ceiling at `t`, summed from +0 in index order.
fn banded(b: &Banded, component: usize, t: f64, at: Bound) -> Result<C64, CollapseError> {
    let turned =
        |rate: &SpectralSum| Ok::<f64, CollapseError>(eval_lane_on(&rate.lanes[0], t, at.2)?.re);
    let Some((from, to)) = b.within(turned(&b.slope)?, turned(&b.offset)?) else {
        return Ok(C64::ZERO);
    };
    let (refs, outer, on) = at;
    let mut bound = outer.to_vec();
    bound.push((b.series.index, 0.0));
    let mut sum = C64::ZERO;
    for k in from..=to {
        *bound.last_mut().expect("the index pushed") = (b.series.index, k as f64);
        sum = sum + eval_at(&b.series.term.body, component, t, (refs, &bound, on))?;
    }
    Ok(sum)
}

/// How fast a carrier's angle turns at `t`, in radians a second.
pub(crate) fn turning(rate: &SpectralSum, t: f64) -> Result<f64, CollapseError> {
    Ok(eval_spectral_sum(rate, 0, t)?.re)
}

/// One operation per written subterm, each read of `refs[k]` counting that form's.
pub(crate) fn terms(body: &Body, refs: &[usize]) -> usize {
    match body {
        Body::Node(id) => refs[id.0 as usize],
        Body::Banded(b) => (b.widest.max(0) as usize)
            .saturating_mul(terms(&b.series.term.body, refs))
            .saturating_add(2),
        _ => children(body)
            .iter()
            .fold(1usize, |held, p| held.saturating_add(terms(&p.body, refs))),
    }
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
