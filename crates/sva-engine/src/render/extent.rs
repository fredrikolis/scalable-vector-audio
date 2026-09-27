// Concern: the grid samples each node is computed over, support met with demand | Non-concern: computing them, where a node is cut (cut/) | IO: (&Render, demands) -> an Extent per node

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, C64, Fold, Held, NodeId, Unary, exp_zero_at};
use sva_samples::{Extent, NodeRenderer};

use super::Render;
use super::pointwise::{self, Point};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::schedule;
use crate::typing::{Typing, Value};

/// Where each node can be nonzero: outside its support a node is exactly zero, and a node
/// cut at a sample is zero from there on.
pub(crate) struct Supports<'a> {
    tys: &'a Typing,
    rate: u32,
    cuts: Cuts,
    held: RefCell<BTreeMap<NodeId, Extent>>,
    open: RefCell<BTreeSet<NodeId>>,
}

/// The sample each cut node's extent ends at.
pub(crate) type Cuts = BTreeMap<NodeId, i64>;

impl<'a> Supports<'a> {
    pub(crate) fn new(tys: &'a Typing, rate: u32) -> Supports<'a> {
        Supports::cut(tys, rate, Cuts::new())
    }

    pub(crate) fn cut(tys: &'a Typing, rate: u32, cuts: Cuts) -> Supports<'a> {
        Supports {
            tys,
            rate,
            cuts,
            held: RefCell::default(),
            open: RefCell::default(),
        }
    }

    pub(crate) fn of(&self, id: NodeId) -> Extent {
        if let Some(held) = self.held.borrow().get(&id) {
            return *held;
        }
        if !self.open.borrow_mut().insert(id) {
            return Extent::EVERYWHERE;
        }
        let found = match schedule::holds_self(self.tys, id, &mut BTreeSet::new()) {
            true => self.looped(id),
            false => self.fresh(id),
        };
        let found = match self.cuts.get(&id) {
            Some(at) => found.intersect(Extent::new(i64::MIN, (*at).max(i64::MIN + 1))),
            None => found,
        };
        self.open.borrow_mut().remove(&id);
        self.held.borrow_mut().insert(id, found);
        found
    }

    fn fresh(&self, id: NodeId) -> Extent {
        match self.tys.value(id) {
            Value::ClosedForm(form) => self.body(&form.body, form.var == sva_formula::Var::T),
            Value::Cast(Cast::Fourier | Cast::IFourier, _) => Extent::EVERYWHERE,
            Value::Cast(_, source) => self.of(*source),
            Value::Op { name, args } => self.operation(name, args, &|arg| self.of(arg)),
            Value::SelfAt(_) => Extent::NOWHERE,
            Value::Grid(count) if *count == 0.0 => Extent::NOWHERE,
            Value::Grid(_) => Extent::EVERYWHERE,
            Value::Read { source, at, .. } => match at.steps_at(self.rate) {
                Ok(steps) => self.of(*source).shifted(-steps),
                Err(count) => moved(self.of(*source), -count),
            },
            Value::Filter { x, .. } => stateful(self.of(*x)),
            Value::Solver(_) => Extent::from(0),
        }
    }

    /// A loop starts where its input does and rings on, as far as a crop around it allows.
    fn looped(&self, id: NodeId) -> Extent {
        let rings = stateful(self.with_past(id, Extent::NOWHERE));
        match rings.is_empty() {
            true => Extent::NOWHERE,
            false => self.with_past(id, rings).intersect(rings),
        }
    }

    /// The loop `id` with its own past nonzero over `past`.
    fn with_past(&self, id: NodeId, past: Extent) -> Extent {
        match self.tys.value(id) {
            Value::SelfAt(_) => past,
            Value::Op { name, args } => self.operation(
                name,
                args,
                &|arg| match schedule::holds_self(self.tys, arg, &mut BTreeSet::new()) {
                    true => self.with_past(arg, past),
                    false => self.of(arg),
                },
            ),
            Value::Filter { x, .. } => stateful(self.with_past(*x, past)),
            _ => self.of(id),
        }
    }

    fn operation(&self, name: &str, args: &[NodeId], of: &dyn Fn(NodeId) -> Extent) -> Extent {
        let number = |at: usize| args.get(at).and_then(|a| constant(self.tys, *a));
        match name {
            "+" | "-" | "join" => args
                .iter()
                .fold(Extent::NOWHERE, |held, a| held.hull(of(*a))),
            "*" => args
                .iter()
                .fold(Extent::EVERYWHERE, |held, a| held.intersect(of(*a))),
            "/" if number(1).is_some_and(|d| d != 0.0) => of(args[0]),
            "pow" if number(1).is_some_and(|n| n > 0.0) => of(args[0]),
            "crop" => match (number(1), number(2)) {
                (Some(l), Some(r)) => of(args[0]).intersect(window(self.rate, l, r)),
                _ => of(args[0]),
            },
            "ch" => of(args[0]),
            other => match Unary::from_name(other) {
                Some(op) if keeps_zero(op) => of(args[0]),
                _ => Extent::EVERYWHERE,
            },
        }
    }

    /// `timed` where the form is in `t`: only there is a factor's zero a zero in time.
    fn body(&self, body: &Body, timed: bool) -> Extent {
        let each = |parts: &[sva_formula::Part]| -> Vec<Extent> {
            parts.iter().map(|p| self.body(&p.body, timed)).collect()
        };
        match body {
            Body::Const(c) if *c == C64::ZERO => Extent::NOWHERE,
            Body::Node(id) => self.of(*id),
            Body::Add(parts) | Body::Join(parts) => {
                each(parts).into_iter().fold(Extent::NOWHERE, Extent::hull)
            }
            Body::Mul(parts) => {
                let held = each(parts)
                    .into_iter()
                    .fold(Extent::EVERYWHERE, Extent::intersect);
                match timed {
                    true => held.intersect(self.underflows(&factors(body))),
                    false => held,
                }
            }
            Body::Apply(Unary::Exp, _) if timed => self.underflows(&[body]),
            Body::Fold(Fold::Max, parts) if timed => ramp(self.rate, parts),
            Body::Div(num, den) if matches!(*den.body, Body::Const(c) if c != C64::ZERO) => {
                self.body(&num.body, timed)
            }
            Body::Pow(base, n) if *n > 0 => self.body(&base.body, timed),
            Body::Apply(op, arg) if keeps_zero(*op) => self.body(&arg.body, timed),
            Body::Shift { by, of } => moved(self.body(&of.body, timed), by * f64::from(self.rate)),
            Body::Crop { of, l, r, .. } => {
                self.body(&of.body, timed)
                    .intersect(window(self.rate, l.value(), r.value()))
            }
            Body::Channel(of, _) => self.body(&of.body, timed),
            _ => Extent::EVERYWHERE,
        }
    }

    /// Where the node `owner`'s program must start so that every state it holds starts
    /// where its input does: a filter's input, a solver at t = 0, a loop at its own support.
    pub(crate) fn state_start(&self, id: NodeId, owner: NodeId) -> Option<i64> {
        let earliest = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match self.tys.value(id) {
            Value::Filter {
                x, cutoff, q, gain, ..
            } => [*x, *cutoff, *q, *gain]
                .into_iter()
                .fold(starting(self.of(id)), |held, arg| {
                    earliest(held, self.state_start(arg, owner))
                }),
            Value::Solver(_) => Some(0),
            Value::SelfAt(_) => starting(self.of(owner)),
            Value::Op { args, .. } => args.iter().fold(None, |held, arg| {
                earliest(held, self.state_start(*arg, owner))
            }),
            _ => None,
        }
    }
}

impl Supports<'_> {
    /// Where a product of exponentials is exactly zero: their exponents add, until every
    /// factor's `exp` or the product of them underflows past the engine's own zero. Every
    /// other factor is bounded there, so zero times it stays zero; the margin covers each
    /// evaluator's own rounding of the exponent, the subnormals included.
    fn underflows(&self, factors: &[&Body]) -> Extent {
        let (mut exponent, mut sizes) = ([0.0f64; 3], [0.0f64; 3]);
        let (mut exps, mut others) = (Vec::new(), Vec::new());
        for factor in factors {
            match exponential(factor) {
                Some(p) => {
                    for k in 0..3 {
                        exponent[k] += p[k];
                        sizes[k] += p[k].abs();
                    }
                    exps.push(*factor);
                }
                None => others.push(*factor),
            }
        }
        let count = exps.len() as f64;
        if count == 0.0 || !exponent.iter().all(|c| c.is_finite()) {
            return Extent::EVERYWHERE;
        }
        let farthest = i64::MAX as f64 / f64::from(self.rate);
        let at = |t: f64| exponent[0] + exponent[1] * t + exponent[2] * t * t;
        let margin =
            |t: f64| 8.0 + 2.0 * count + 1e-9 * (sizes[0] + sizes[1] * t.abs() + sizes[2] * t * t);
        let side = |from: f64, to: f64| -> Option<f64> {
            let zero =
                |t: f64, bound: f64| at(t) + bound.max(1.0).ln() + margin(t) <= exp_zero_at();
            let mut edge = crossing(from, to, |t| zero(t, 1.0))?;
            for _ in 0..2 {
                let (lo, hi) = (edge.min(to), edge.max(to));
                let bound = others.iter().try_fold(1.0f64, |held, f| {
                    Some(held * self.bound(f, lo, hi, 0)?.max(1.0))
                })?;
                edge = crossing(from, to, |t| zero(t, bound))?;
            }
            let (lo, hi) = (edge.min(to), edge.max(to));
            let every = exps.iter().chain(&others);
            let mut reach = every.map(|f| self.bound(f, lo, hi, 0).map(|b| b.max(1.0)));
            let product = reach.try_fold(1.0f64, |held, b| Some(held * b?))?;
            (product < 1e300).then_some(edge)
        };
        let samples = f64::from(self.rate);
        let vertex = match exponent[2] {
            a if a < 0.0 => -exponent[1] / (2.0 * a),
            _ if exponent[1] < 0.0 => -farthest,
            _ => return Extent::EVERYWHERE,
        };
        let end = side(vertex, farthest).map_or(i64::MAX, |t| {
            ((t * samples).ceil() as i64).saturating_add(1)
        });
        let start = match exponent[2] < 0.0 {
            true => side(vertex, -farthest).map_or(i64::MIN, |t| {
                ((t * samples).floor() as i64).saturating_sub(1)
            }),
            false => i64::MIN,
        };
        match start < end {
            true => Extent::new(start, end),
            false => Extent::NOWHERE,
        }
    }

    /// The largest magnitude `body` reaches over `[lo, hi]` seconds, where one is known.
    pub(crate) fn bound(&self, body: &Body, lo: f64, hi: f64, depth: usize) -> Option<f64> {
        let of = |b: &Body| self.bound(b, lo, hi, depth);
        let held = match body {
            Body::Const(c) => c.re.abs() + c.im.abs(),
            Body::Line => lo.abs().max(hi.abs()),
            Body::Add(parts) => parts.iter().try_fold(0.0, |h, p| Some(h + of(&p.body)?))?,
            Body::Join(parts) | Body::Fold(_, parts) => parts
                .iter()
                .try_fold(0.0f64, |h, p| Some(h.max(of(&p.body)?)))?,
            Body::Mul(parts) => parts
                .iter()
                .try_fold(1.0, |h, p| Some(h * of(&p.body)?.max(1.0)))?,
            Body::Div(num, den) => match &*den.body {
                Body::Const(c) if !c.is_zero() => of(&num.body)? / (c.re.abs() + c.im.abs()) * 2.0,
                _ => return None,
            },
            Body::Pow(base, n) if *n >= 0 => of(&base.body)?.max(1.0).powi(*n),
            Body::Apply(Unary::Sin | Unary::Cos, arg) => {
                let [c0, c1, c2] = real_polynomial(&arg.body)?;
                let t = lo.abs().max(hi.abs());
                let reach = c0.abs() + c1.abs() * t + c2.abs() * t * t;
                reach.is_finite().then_some(1.0)?
            }
            Body::Apply(Unary::Tanh | Unary::Sat, arg) => of(&arg.body).map(|_| 1.0)?,
            Body::Apply(Unary::Abs, arg) => of(&arg.body)?,
            Body::Apply(Unary::Exp, arg) => {
                let [c0, c1, c2] = real_polynomial(&arg.body)?;
                let at = |t: f64| c0 + c1 * t + c2 * t * t;
                let mut top = at(lo).max(at(hi));
                if c2 != 0.0 {
                    let vertex = -c1 / (2.0 * c2);
                    if lo < vertex && vertex < hi {
                        top = top.max(at(vertex));
                    }
                }
                (top + 1.0).exp()
            }
            Body::Crop { of: inner, .. } | Body::Channel(inner, _) => of(&inner.body)?,
            Body::Shift { by, of: inner } => self.bound(&inner.body, lo - by, hi - by, depth)?,
            Body::Node(id) if depth < 16 => match self.tys.value(*id) {
                Value::ClosedForm(form) => self.bound(&form.body, lo, hi, depth + 1)?,
                _ => return None,
            },
            _ => return None,
        };
        (held < 1e300).then_some(held * 1.0001)
    }
}

fn factors(body: &Body) -> Vec<&Body> {
    match body {
        Body::Mul(parts) => parts.iter().flat_map(|p| factors(&p.body)).collect(),
        other => vec![other],
    }
}

/// `exp` of a polynomial in `t`, as its exponent's real coefficients.
fn exponential(body: &Body) -> Option<[f64; 3]> {
    match body {
        Body::Apply(Unary::Exp, arg) => {
            let [c0, c1, c2] = sva_formula::affine::polynomial(&arg.body)?
                .iter()
                .map(|c| c.exact())
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .chain(std::iter::repeat(C64::ZERO))
                .take(3)
                .map(|c| c.re)
                .collect::<Vec<_>>()[..]
            else {
                return None;
            };
            Some([c0, c1, c2])
        }
        _ => None,
    }
}

fn real_polynomial(body: &Body) -> Option<[f64; 3]> {
    let held = sva_formula::affine::polynomial(body)?;
    let mut out = [0.0; 3];
    for (slot, c) in out.iter_mut().zip(held) {
        let c = c.exact()?;
        if c.im != 0.0 || !c.re.is_finite() {
            return None;
        }
        *slot = c.re;
    }
    Some(out)
}

/// The first `t` from `from` towards `to` a predicate false then true holds at, to within
/// the doubles' own spacing.
fn crossing(from: f64, to: f64, holds: impl Fn(f64) -> bool) -> Option<f64> {
    if !holds(to) {
        return None;
    }
    let (mut no, mut yes) = (from, to);
    for _ in 0..200 {
        let mid = no + (yes - no) / 2.0;
        if mid == no || mid == yes {
            break;
        }
        match holds(mid) {
            true => yes = mid,
            false => no = mid,
        }
    }
    Some(yes)
}

/// `max(0, v)` with `v` falling in `t` is exactly zero from the first sample every
/// evaluator reads `v` at or below zero.
fn ramp(rate: u32, parts: &[sva_formula::Part]) -> Extent {
    let v = match parts {
        [a, b] if matches!(*a.body, Body::Const(c) if c.re.to_bits() == 0 && c.im == 0.0) => {
            &*b.body
        }
        [a, b] if matches!(*b.body, Body::Const(c) if c.re.to_bits() == 0 && c.im == 0.0) => {
            &*a.body
        }
        _ => return Extent::EVERYWHERE,
    };
    let falls = real_polynomial(v).is_some() && monotone(v, &mut false);
    let slope = sva_formula::affine::exact_affine(v).map(|(a, _)| a.re);
    if !falls || !slope.is_some_and(|a| a < 0.0) {
        return Extent::EVERYWHERE;
    }
    let sr = f64::from(rate);
    let at = |t: f64| sva_samples::eval_written_at(v, 0, t, &Unread).map(|x| x.re);
    let zero = |n: i64| {
        let both = [at(n as f64 / sr), at(n as f64 * (1.0 / sr))];
        both.iter().all(|x| x.as_ref().is_ok_and(|x| *x <= 0.0))
    };
    let reach = 1i64 << 62;
    match zero(reach) {
        false => Extent::EVERYWHERE,
        true => {
            let (mut no, mut yes) = (-reach, reach);
            while i128::from(yes) - i128::from(no) > 1 {
                let mid = ((i128::from(no) + i128::from(yes)) / 2) as i64;
                match zero(mid) {
                    true => yes = mid,
                    false => no = mid,
                }
            }
            Extent::new(i64::MIN, yes)
        }
    }
}

/// Built of steps each rising or falling with `t` alone, so its rounded value does too.
fn monotone(body: &Body, moving: &mut bool) -> bool {
    match body {
        Body::Const(_) => true,
        Body::Line => !std::mem::replace(moving, true),
        Body::Add(parts) | Body::Mul(parts) => parts.iter().all(|p| monotone(&p.body, moving)),
        Body::Div(num, den) => matches!(*den.body, Body::Const(_)) && monotone(&num.body, moving),
        Body::Shift { of, .. } => monotone(&of.body, moving),
        _ => false,
    }
}

struct Unread;

impl sva_samples::Refs for Unread {
    fn value(&self, _: NodeId, _: usize, _: f64) -> Result<C64, sva_samples::CollapseError> {
        Err(sva_samples::CollapseError::NotEvaluable("a node"))
    }

    fn width(&self, _: NodeId) -> usize {
        1
    }
}

/// Where a range with no stated start starts: t = 0, or earlier where a crop reaches back.
pub(crate) fn default_start(support: Extent) -> i64 {
    match support.is_empty() || support.start == i64::MIN || support.start > 0 {
        true => 0,
        false => support.start,
    }
}

/// Where a range with no stated end ends, if its support ends.
pub(crate) fn default_end(support: Extent) -> Option<i64> {
    (support.end != i64::MAX).then_some(support.end)
}

fn starting(support: Extent) -> Option<i64> {
    (!support.is_empty()).then_some(support.start)
}

/// A stateful node starts where its input does, t = 0 where that has no start, and rings on.
fn stateful(input: Extent) -> Extent {
    match input.is_empty() {
        true => Extent::NOWHERE,
        false if input.start == i64::MIN => Extent::from(0),
        false => Extent::from(input.start),
    }
}

/// `sin`, `tanh`, `abs`, `sqrt` and `sat` hold zero at zero; the rest move it.
fn keeps_zero(op: Unary) -> bool {
    matches!(
        op,
        Unary::Sin | Unary::Tanh | Unary::Abs | Unary::Sqrt | Unary::Sat
    )
}

fn constant(tys: &Typing, id: NodeId) -> Option<f64> {
    match tys.value(id) {
        Value::ClosedForm(form) => crate::lower::constant_value(&form.body, form.var),
        _ => None,
    }
}

/// The samples a crop's `[l, r)` can be nonzero over: every one some evaluator reads as
/// inside it, whether it takes the instant as `n / rate` or as `n * (1 / rate)`.
fn window(rate: u32, l: f64, r: f64) -> Extent {
    if l.is_nan() || r.is_nan() {
        return Extent::EVERYWHERE;
    }
    let sr = f64::from(rate);
    let step = 1.0 / sr;
    let reached = |n: i64, edge: f64| (n as f64 / sr >= edge, n as f64 * step >= edge);
    let first = |edge: f64, any: bool| {
        let mut n = (edge * sr).ceil() as i64;
        let past = |n: i64| {
            let (a, b) = reached(n, edge);
            if any { a || b } else { a && b }
        };
        while past(n - 1) {
            n -= 1;
        }
        while !past(n) {
            n += 1;
        }
        n
    };
    let start = match l {
        l if l == f64::NEG_INFINITY => i64::MIN,
        l if l == f64::INFINITY => return Extent::NOWHERE,
        l => first(l, true),
    };
    let end = match r {
        r if r == f64::INFINITY => i64::MAX,
        r if r == f64::NEG_INFINITY => return Extent::NOWHERE,
        r => first(r, false),
    };
    match start < end {
        true => Extent::new(start, end),
        false => Extent::NOWHERE,
    }
}

/// Moved by a count of samples that need not be whole, widened to the samples either side.
fn moved(support: Extent, count: f64) -> Extent {
    if support.is_empty() || support == Extent::EVERYWHERE {
        return support;
    }
    if count.fract() == 0.0 {
        return support.shifted(count as i64);
    }
    let (early, late) = (count.floor() as i64 - 1, count.ceil() as i64 + 1);
    let start = support.shifted(early).start;
    let end = support.shifted(late).end;
    Extent::new(start, end)
}

/// Every node a render holds, each over its own extent: the root's demand runs down every
/// read, and each node meets it with its support. A stateful node runs from where its state
/// starts, and asks its inputs for the same.
#[derive(Default)]
pub(crate) struct Extents {
    pub(crate) support: BTreeMap<NodeId, Extent>,
    pub(crate) decided: BTreeMap<NodeId, Extent>,
    pub(crate) cuts: Cuts,
}

impl Extents {
    pub(crate) fn support(&self, id: NodeId) -> Extent {
        *self
            .support
            .get(&id)
            .unwrap_or_else(|| panic!("no support was found for node {id:?}"))
    }

    pub(crate) fn of(&self, id: NodeId) -> Extent {
        *self
            .decided
            .get(&id)
            .unwrap_or_else(|| panic!("no extent was decided for node {id:?}"))
    }
}

/// `order` lists the held nodes dependencies first, so walking it backwards meets every
/// reader of a node before the node. `demands` seeds the root and every node a reading asks.
pub(crate) fn decide(
    held: &Render,
    order: &[NodeId],
    demands: &[(NodeId, Extent)],
    cuts: &Cuts,
) -> Result<Extents, EngineError> {
    let supports = Supports::cut(&held.tys, held.config.rate, cuts.clone());
    let mut demand: BTreeMap<NodeId, Extent> = BTreeMap::new();
    for (id, asked) in demands {
        let slot = demand.entry(*id).or_insert(Extent::NOWHERE);
        *slot = slot.hull(*asked);
    }
    let mut decided = BTreeMap::new();
    for &id in order.iter().rev() {
        let asked = demand.get(&id).copied().unwrap_or(Extent::NOWHERE);
        let support = supports.of(id);
        let extent = own(held, &supports, id, asked, support, cuts.get(&id).copied())?;
        decided.insert(id, extent);
        if extent.is_empty() {
            continue;
        }
        for (source, wants) in reads(held, &supports, id, extent)? {
            debug_assert!(
                !decided.contains_key(&source),
                "a node is read after its extent was decided"
            );
            let slot = demand.entry(source).or_insert(Extent::NOWHERE);
            *slot = slot.hull(wants);
        }
    }
    let support = supports.held.into_inner();
    Ok(Extents {
        support,
        decided,
        cuts: cuts.clone(),
    })
}

/// Demand met with support, pulled back to where the node's state starts. A short-time
/// transform takes its whole support, which has to end. A closed form the demand meets is
/// collapsed over the whole demand up to its cut: a row is chosen by the length it runs
/// over, and one ended at the support would take another row than the one a stream reads.
fn own(
    held: &Render,
    supports: &Supports,
    id: NodeId,
    asked: Extent,
    support: Extent,
    cut: Option<i64>,
) -> Result<Extent, EngineError> {
    let met = asked.intersect(support);
    if met.is_empty() {
        return Ok(Extent::NOWHERE);
    }
    let extent = match (held.tys.value(id), program_state(held, supports, id)) {
        (Value::Cast(Cast::Stft { .. }, _), _) if support.end == i64::MAX => {
            return Err(unbounded(held, id));
        }
        (Value::Cast(Cast::Stft { .. }, _), _) => support,
        _ if held.tys.ty(id).is_closed_form() => match cut {
            Some(at) => asked.intersect(Extent::new(i64::MIN, at.max(i64::MIN + 1))),
            None => asked,
        },
        (_, Some(start)) if start < met.start => Extent::new(start, met.end),
        _ => met,
    };
    match extent.start == i64::MIN {
        true => Err(unbounded(held, id)),
        false => Ok(extent),
    }
}

fn program_state(held: &Render, supports: &Supports, id: NodeId) -> Option<i64> {
    match held.tys.ty(id).held {
        Held::Sampled => supports.state_start(id, id),
        _ => None,
    }
}

/// Every node `id` reads over `extent`, and the samples it reads of each.
fn reads(
    held: &Render,
    supports: &Supports,
    id: NodeId,
    extent: Extent,
) -> Result<Vec<(NodeId, Extent)>, EngineError> {
    match (held.tys.ty(id).held, held.tys.value(id)) {
        (Held::Frames, Value::Cast(_, source)) => Ok(vec![(*source, supports.of(*source))]),
        (Held::Sampled, Value::Cast(Cast::Istft, frames)) => {
            Ok(vec![(*frames, supports.of(*frames))])
        }
        (Held::Sampled, _) => {
            let program = super::sampled::program(held, id)?;
            let mut out = Vec::new();
            leaves(&program.renderer, &mut |leaf| {
                if let NodeRenderer::Buffer { id: slot, shift } = leaf {
                    out.push((program.reads[slot.0 as usize], extent.shifted(*shift)));
                }
            });
            Ok(out)
        }
        _ if schedule::materialized_operands(&held.tys, id).is_empty() => Ok(Vec::new()),
        _ => {
            let Ok(tree) = pointwise::plan(held, id) else {
                return Ok(Vec::new());
            };
            let mut found = Vec::new();
            point_reads(&tree, Some(0.0), &mut found);
            let rate = f64::from(held.config.rate);
            Ok(found
                .into_iter()
                .map(|(source, by)| match by {
                    Some(secs) => (source, moved(extent, secs * rate)),
                    None => (source, supports.of(source)),
                })
                .collect())
        }
    }
}

/// Each buffer a pointwise tree reads and how far from the instant it reads it, in seconds;
/// `None` where the time it reads moves with the instant.
fn point_reads(tree: &Point, by: Option<f64>, out: &mut Vec<(NodeId, Option<f64>)>) {
    match tree {
        Point::Buffer(id) => out.push((*id, by)),
        Point::SpectralSum(_) => {}
        Point::Operation { args, .. } => args.iter().for_each(|a| point_reads(a, by, out)),
        Point::Written { body, refs } => body_reads(body, by, refs, out),
    }
}

fn body_reads(
    body: &Body,
    by: Option<f64>,
    refs: &BTreeMap<NodeId, Point>,
    out: &mut Vec<(NodeId, Option<f64>)>,
) {
    match body {
        Body::Node(id) => point_reads(&refs[id], by, out),
        Body::Shift { by: moved, of } => body_reads(&of.body, by.map(|b| b - moved), refs, out),
        Body::Warp { .. } | Body::Deriv { .. } => sva_formula::closed_form::children(body)
            .iter()
            .for_each(|p| body_reads(&p.body, None, refs, out)),
        _ => sva_formula::closed_form::children(body)
            .iter()
            .for_each(|p| body_reads(&p.body, by, refs, out)),
    }
}

/// Every leaf under a renderer's operators.
pub(crate) fn leaves(renderer: &NodeRenderer, found: &mut dyn FnMut(&NodeRenderer)) {
    match renderer {
        NodeRenderer::Add(parts) | NodeRenderer::Mul(parts) | NodeRenderer::Join(parts) => {
            parts.iter().for_each(|p| leaves(p, found));
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => {
            leaves(a, found);
            leaves(b, found);
        }
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => leaves(x, found),
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .for_each(|p| leaves(p, found)),
        leaf => found(leaf),
    }
}

/// A short-time transform reads its input whole, and a moving read anywhere at all.
fn unbounded(held: &Render, id: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.unbounded_extent".to_string(),
        message: format!(
            "`{}` reads its input over every instant, and that input never ends",
            held.tys.name(id)
        ),
        location: Located::at(held.tys.name(id), None),
        help: "crop what a short-time transform or a moving read takes to a window".to_string(),
    })
}
