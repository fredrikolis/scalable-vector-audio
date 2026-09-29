// Concern: the grid samples each node is computed over, support met with demand | Non-concern: computing them, where the root's demand ends (reach.rs) | IO: (&Render, demands) -> an Extent per node

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, C64, Fold, Held, NodeId, Unary, exp_zero_at};
use sva_samples::{Extent, Grid, NodeRenderer, Round, Slot};

use crate::time::Q;

use super::Render;
use super::pointwise::{self, Point};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::schedule;
use crate::typing::{Typing, Value, When};

/// Where each node can be nonzero, in samples of its own grid: outside its support a node is
/// exactly zero.
pub(crate) struct Supports<'a> {
    tys: &'a Typing,
    held: RefCell<BTreeMap<NodeId, Extent>>,
    open: RefCell<BTreeSet<NodeId>>,
}

impl<'a> Supports<'a> {
    pub(crate) fn new(held: &'a Render) -> Supports<'a> {
        Supports {
            tys: &held.tys,
            held: RefCell::default(),
            open: RefCell::default(),
        }
    }

    fn grid(&self, id: NodeId) -> Grid {
        self.tys.grid(id)
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
        self.open.borrow_mut().remove(&id);
        self.held.borrow_mut().insert(id, found);
        found
    }

    fn fresh(&self, id: NodeId) -> Extent {
        let grid = self.grid(id);
        match self.tys.value(id) {
            Value::ClosedForm(form) => {
                let held = self.body(&form.body, form.var == sva_formula::Var::T, grid);
                match grid.is_rate() {
                    true => held,
                    false => reached(held, (-1, 1)),
                }
            }
            Value::Cast(Cast::Fourier | Cast::IFourier, _) => Extent::EVERYWHERE,
            Value::Cast(_, source) => self.of(*source),
            Value::Op { name, args } => self.operation(name, args, grid, &|arg| self.of(arg)),
            Value::SelfAt { .. } => Extent::NOWHERE,
            Value::Noise(_) => Extent::EVERYWHERE,
            Value::Read {
                source,
                at: When::Nearest(nearest),
                ..
            } => match self.reach(*nearest, grid) {
                Some(reach) => reached(self.of(*source), reach),
                None => Extent::EVERYWHERE,
            },
            Value::Read { source, at, .. } => {
                match at.map(self.tys.grid(id), self.tys.grid(*source)) {
                    Some(map) => map.preimage(self.of(*source)),
                    None => Extent::EVERYWHERE,
                }
            }
            Value::Filter { x, .. } => stateful(self.of(*x)),
            Value::Solver { .. } => Extent::from(0),
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
            Value::SelfAt { .. } => past,
            Value::Op { name, args } => {
                self.operation(
                    name,
                    args,
                    self.grid(id),
                    &|arg| match schedule::holds_self(self.tys, arg, &mut BTreeSet::new()) {
                        true => self.with_past(arg, past),
                        false => self.of(arg),
                    },
                )
            }
            Value::Filter { x, .. } => stateful(self.with_past(*x, past)),
            _ => self.of(id),
        }
    }

    pub(crate) fn reach(&self, nearest: crate::typing::Nearest, grid: Grid) -> Option<(i64, i64)> {
        super::bound::reach(self.tys, nearest, grid)
    }

    fn operation(
        &self,
        name: &str,
        args: &[NodeId],
        grid: Grid,
        of: &dyn Fn(NodeId) -> Extent,
    ) -> Extent {
        let number = |at: usize| {
            args.get(at)
                .and_then(|a| crate::lower::number_of(self.tys, *a))
        };
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
                (Some(l), Some(r)) => of(args[0]).intersect(window(grid, l, r)),
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
    fn body(&self, body: &Body, timed: bool, grid: Grid) -> Extent {
        let each = |parts: &[sva_formula::Part]| -> Vec<Extent> {
            parts
                .iter()
                .map(|p| self.body(&p.body, timed, grid))
                .collect()
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
                    true => held.intersect(self.underflows(&factors(body), grid)),
                    false => held,
                }
            }
            Body::Apply(Unary::Exp, _) if timed => self.underflows(&[body], grid),
            Body::Fold(Fold::Max, parts) if timed => ramp(grid, parts),
            Body::Div(num, den) if matches!(*den.body, Body::Const(c) if c != C64::ZERO) => {
                self.body(&num.body, timed, grid)
            }
            Body::Pow(base, n) if *n > 0 => self.body(&base.body, timed, grid),
            Body::Apply(op, arg) if keeps_zero(*op) => self.body(&arg.body, timed, grid),
            Body::Shift { by, of } => moved(self.body(&of.body, timed, grid), by * grid.sr()),
            Body::Crop { of, l, r, .. } => {
                self.body(&of.body, timed, grid)
                    .intersect(window(grid, l.value(), r.value()))
            }
            Body::Channel(of, _) => self.body(&of.body, timed, grid),
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
            Value::Solver { .. } => Some(0),
            Value::SelfAt { .. } => starting(self.of(owner)),
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
    fn underflows(&self, factors: &[&Body], grid: Grid) -> Extent {
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
        let farthest = i64::MAX as f64 / grid.sr();
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
        let vertex = match exponent[2] {
            a if a < 0.0 => -exponent[1] / (2.0 * a),
            _ if exponent[1] < 0.0 => -farthest,
            _ => return Extent::EVERYWHERE,
        };
        let end = side(vertex, farthest).map_or(i64::MAX, |t| {
            (grid.count(t).ceil() as i64).saturating_add(1)
        });
        let start = match exponent[2] < 0.0 {
            true => side(vertex, -farthest).map_or(i64::MIN, |t| {
                (grid.count(t).floor() as i64).saturating_sub(1)
            }),
            false => i64::MIN,
        };
        match start < end {
            true => Extent::new(start, end),
            false => Extent::NOWHERE,
        }
    }

    /// The largest magnitude `body` reaches over `[lo, hi]` seconds, where one is known.
    fn bound(&self, body: &Body, lo: f64, hi: f64, depth: usize) -> Option<f64> {
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
            Body::Apply(Unary::Tanh | Unary::Sat | Unary::Step, arg) => {
                of(&arg.body).map(|_| 1.0)?
            }
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
fn ramp(grid: Grid, parts: &[sva_formula::Part]) -> Extent {
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
    let at = |t: f64| sva_samples::eval_written_at(v, 0, t, &Unread).map(|x| x.re);
    let zero = |n: i64| {
        let both = [at(grid.instant(n)), at(grid.stepped(n))];
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

/// The samples whose exact instants `(a n + b) / (d rate)` lie in a crop's `[l, r)`, each edge
/// the decimal it was written as.
pub(crate) fn window(grid: Grid, l: f64, r: f64) -> Extent {
    if l.is_nan() || r.is_nan() {
        return Extent::EVERYWHERE;
    }
    let (start, end) = (first_at(grid, l), first_at(grid, r));
    match start < end {
        true => Extent::new(start, end),
        false => Extent::NOWHERE,
    }
}

/// The first sample whose exact instant is at or past `edge`: past the integers, the end the
/// edge's sign names. An edge no decimal of 120 bits spells is read at its binary value, and
/// one under 2^-74 moves only a sample standing exactly at zero.
fn first_at(grid: Grid, edge: f64) -> i64 {
    let beyond = if edge < 0.0 { i64::MIN } else { i64::MAX };
    if edge.is_infinite() {
        return beyond;
    }
    let decimal = || {
        let steps = Q::decimal(edge)?
            .mul(Q::new(grid.d.checked_mul(i128::from(grid.rate))?, 1)?)?
            .sub(Q::new(grid.b, 1)?)?
            .div(Q::new(grid.a, 1)?)?;
        let (num, den) = (steps.num(), steps.den());
        Some(num.div_euclid(den) + i128::from(num.rem_euclid(den) != 0))
    };
    if let Some(n) = decimal() {
        return i64::try_from(n).unwrap_or(beyond);
    }
    if let Some(n) = grid.step_at(edge, Round::Ceil) {
        return n;
    }
    if edge.abs() >= 1.0 {
        return beyond;
    }
    let (b, a) = (-grid.b, grid.a);
    let at = b.div_euclid(a) + i128::from(b.rem_euclid(a) != 0 || edge > 0.0);
    i64::try_from(at).unwrap_or(beyond)
}

/// Every sample whose offsets in `reach` land inside `support`. A form on a moved grid is
/// read by rows of the form moved, whose edges round a step either way.
fn reached(support: Extent, (least, most): (i64, i64)) -> Extent {
    if support.is_empty() || support == Extent::EVERYWHERE {
        return support;
    }
    Extent::new(
        support.start.saturating_sub(most),
        support.end.saturating_sub(least),
    )
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
) -> Result<Extents, EngineError> {
    let supports = Supports::new(held);
    let mut demand: BTreeMap<NodeId, Extent> = BTreeMap::new();
    for (id, asked) in demands {
        let slot = demand.entry(*id).or_insert(Extent::NOWHERE);
        *slot = slot.hull(*asked);
    }
    let mut decided = BTreeMap::new();
    for &id in order.iter().rev() {
        let asked = demand.get(&id).copied().unwrap_or(Extent::NOWHERE);
        let support = supports.of(id);
        let extent = own(held, &supports, id, asked, support)?;
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
    Ok(Extents { support, decided })
}

/// Demand met with support, pulled back to where the node's state starts. A short-time
/// transform takes its whole support, which has to end. A closed form the demand meets is
/// collapsed over the whole demand: a row is chosen by the length it runs over, and one
/// ended at the support would take another row than the one a stream reads.
fn own(
    held: &Render,
    supports: &Supports,
    id: NodeId,
    asked: Extent,
    support: Extent,
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
        _ if held.tys.ty(id).is_closed_form() => asked,
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
            let widths = |r: NodeId| held.buffers.get(&r).map_or(1, |b| b.width);
            let program = super::sampled::program_reading(held, id, &widths)?;
            Ok(program_reads(&program, extent, held.grid(id)))
        }
        _ if schedule::materialized_operands(&held.tys, id).is_empty() => Ok(Vec::new()),
        _ => {
            let Ok(tree) = pointwise::plan(held, id) else {
                return Ok(Vec::new());
            };
            let mut found = Vec::new();
            point_reads(&tree, Some(0.0), &mut found);
            let rate = held.grid(id).sr();
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

/// Every buffer `program` reads while stepping `extent` on `grid`, and the samples of each.
fn program_reads(
    program: &super::sampled::Program,
    extent: Extent,
    grid: Grid,
) -> Vec<(NodeId, Extent)> {
    let mut out = Vec::new();
    windowed(
        &program.renderer,
        extent,
        grid,
        &mut |leaf, over| match leaf {
            NodeRenderer::Read {
                slot: Slot::Read(slot),
                map,
            } => out.push((program.reads[slot.0 as usize], map.image(over))),
            NodeRenderer::Nearest {
                slot: Slot::Read(slot),
                reach: (least, most),
                ..
            } if !over.is_empty() => {
                let near = Extent::new(
                    over.start.saturating_add(*least),
                    over.end.saturating_add(*most),
                );
                out.push((program.reads[slot.0 as usize], near));
            }
            _ => {}
        },
    );
    out
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

/// Every leaf beside the samples it is read over: a crop over no state is skipped where it is
/// shut, so its window narrows what the reads under it are asked for.
fn windowed(
    renderer: &NodeRenderer,
    over: Extent,
    grid: Grid,
    found: &mut dyn FnMut(&NodeRenderer, Extent),
) {
    match renderer {
        NodeRenderer::Crop { x, a, b, .. } if x.stateless() => {
            let inside = over.intersect(window(grid, *a, *b));
            windowed(x, inside, grid, found);
        }
        NodeRenderer::Crop { .. } => leaves(renderer, &mut |leaf| found(leaf, over)),
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => [x, cutoff, q, gain]
            .into_iter()
            .for_each(|p| windowed(p, over, grid, found)),
        NodeRenderer::Add(parts)
        | NodeRenderer::Mul(parts)
        | NodeRenderer::Join(parts)
        | NodeRenderer::Physics { args: parts, .. } => {
            parts.iter().for_each(|p| windowed(p, over, grid, found));
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => {
            windowed(a, over, grid, found);
            windowed(b, over, grid, found);
        }
        NodeRenderer::Map(_, x) | NodeRenderer::Channel { x, .. } => {
            windowed(x, over, grid, found);
        }
        leaf => leaves(leaf, &mut |inner| found(inner, over)),
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
        NodeRenderer::Physics { args, .. } => args.iter().for_each(|p| leaves(p, found)),
        NodeRenderer::Formula { time, .. } | NodeRenderer::Nearest { time, .. } => {
            leaves(time, found);
            found(renderer);
        }
        leaf => found(leaf),
    }
}

/// A short-time transform reads its input whole.
fn unbounded(held: &Render, id: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.unbounded_extent".to_string(),
        message: format!(
            "`{}` reads its input over every instant, and that input never ends",
            held.tys.name(id)
        ),
        location: Located::at(held.tys.name(id), None),
        help: "crop what a short-time transform takes to a window".to_string(),
    })
}
