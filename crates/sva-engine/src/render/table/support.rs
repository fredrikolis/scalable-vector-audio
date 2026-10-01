// Concern: where each node can be nonzero or is pruned, in whole samples of its own clock | Non-concern: where a reader asks for it, deriving a bound | IO: (NodeId) -> Extent, a state's start, cuts

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, C64, Fold, NodeId, Unary, exp_zero_at};
use sva_samples::{Extent, Grid, Profile, Round};

use crate::cast::Cast;
use crate::render::bound::Tail;
use crate::schedule;
use crate::time::{Affine, Lattice, Q};
use crate::typing::{Step, SumSlot, Typing, Value, When};

/// Where each node can be nonzero, in samples of its own grid: outside its support a node is
/// zero. A node is its own clock: a read's shift moves it by whole samples.
pub(crate) struct Supports<'a> {
    tys: &'a Typing,
    profile: &'a Profile,
    held: RefCell<BTreeMap<NodeId, Extent>>,
    open: RefCell<BTreeSet<NodeId>>,
    cuts: RefCell<BTreeMap<NodeId, i64>>,
}

impl<'a> Supports<'a> {
    pub(crate) fn new(tys: &'a Typing, profile: &'a Profile) -> Supports<'a> {
        Supports {
            tys,
            profile,
            held: RefCell::default(),
            open: RefCell::default(),
            cuts: RefCell::default(),
        }
    }

    pub(crate) fn cuts(&self) -> BTreeMap<NodeId, i64> {
        self.cuts.borrow().clone()
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
            false => self.pruned(id, self.fresh(id)),
        };
        let found = self.retired(id).fold(found, Extent::hull);
        self.open.borrow_mut().remove(&id);
        self.held.borrow_mut().insert(id, found);
        found
    }

    /// Where a stream's note sum was nonzero through the terms it retired: a reader carrying
    /// state from their past rings on as though they still sounded.
    fn retired(&self, id: NodeId) -> impl Iterator<Item = Extent> {
        let slots = self.tys.sum_slots(id).unwrap_or_default().iter();
        slots.filter_map(|slot| match slot {
            SumSlot::Retired(support) => Some(*support),
            SumSlot::Node(_) => None,
        })
    }

    /// The approved exception to exact supports: zero from a sample where its bound over every
    /// later instant is under the prune level; first such sample where the bound falls
    /// monotonically. A node with no such bound keeps its exact support.
    fn pruned(&self, id: NodeId, exact: Extent) -> Extent {
        if exact.is_empty() {
            return exact;
        }
        let grid = self.grid(id);
        let ends = |n: NodeId| self.of(n);
        let Some(tail) = Tail::of(self.tys, (self.profile, grid.rate), id, &ends) else {
            return exact;
        };
        let level = self.profile.prune_level();
        let under = |n: i64| tail.from(grid.instant(n)) < level;
        let last = exact.end.saturating_sub(1);
        let from = exact.start.max(0).min(last);
        let probe = |k: i32| from.saturating_add(grid.count(2f64.powi(k)).ceil() as i64);
        let far = match exact.end {
            i64::MAX => (0..40).map(probe).find(|n| under(*n)),
            _ => under(last).then_some(last),
        };
        let Some(far) = far else {
            return exact;
        };
        let (mut no, mut yes) = (from, far);
        if under(from) {
            yes = from;
        }
        while yes - no > 1 {
            let mid = no + (yes - no) / 2;
            match under(mid) {
                true => yes = mid,
                false => no = mid,
            }
        }
        self.cuts.borrow_mut().insert(id, yes);
        match exact.start < yes {
            true => Extent::new(exact.start, yes),
            false => Extent::NOWHERE,
        }
    }

    /// A closed form a node wrote inside its own body, as a value of its own on `grid`.
    pub(crate) fn formula(&self, body: &Body, grid: Grid) -> Extent {
        self.body(body, true, grid)
    }

    fn fresh(&self, id: NodeId) -> Extent {
        let grid = self.grid(id);
        match self.tys.value(id) {
            Value::ClosedForm(form) => self.body(&form.body, form.var == sva_formula::Var::T, grid),
            Value::Cast(Cast::Fourier | Cast::IFourier, _) => Extent::EVERYWHERE,
            Value::Cast(_, source) => self.of(*source),
            Value::Op { name, args } => self.operation(name, args, grid, &|arg| self.of(arg)),
            Value::SelfAt { .. } => Extent::NOWHERE,
            Value::Noise(_) => Extent::EVERYWHERE,
            Value::Stored(held) => held.support,
            Value::Read {
                source,
                at: When::Step(step),
                ..
            } => match reach(self.tys, step, grid) {
                Some(reach) => reached(self.of(*source), reach),
                None => Extent::EVERYWHERE,
            },
            Value::Read { source, at, .. } => match landed(at, grid) {
                Some(map) => map.preimage(self.of(*source)),
                None => Extent::EVERYWHERE,
            },
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
            Body::Shift { by, of } => match (&*of.body, placed(*by, grid)) {
                (Body::Node(id), Some(map)) => map.preimage(self.of(*id)),
                _ => moved(self.body(&of.body, timed, grid), by * grid.sr()),
            },
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

/// `max(0, v)` with `v` falling in `t` is exactly zero from the first sample whose instant
/// reads `v` at or below zero.
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
    let zero = |n: i64| at(grid.instant(n)).is_ok_and(|x| x <= 0.0);
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

/// The samples whose exact instants lie in a crop's `[l, r)`, each edge the decimal it was
/// written as: `[ceil(l*rate), ceil(r*rate))` on the render's own grid.
pub(crate) fn window(grid: Grid, l: f64, r: f64) -> Extent {
    if l.is_nan() || r.is_nan() {
        return Extent::EVERYWHERE;
    }
    let (start, end) = (grid.edge(l), grid.edge(r));
    match start < end {
        true => Extent::new(start, end),
        false => Extent::NOWHERE,
    }
}

/// The whole-sample read a shift of a node is, `by` seconds rounded to the nearest sample of
/// `grid`, ties to even.
pub(crate) fn placed(by: f64, grid: Grid) -> Option<sva_samples::Map> {
    grid.snapped(shifted(by)?)
}

/// A shift of `by` seconds as the time it reads at.
pub(crate) fn shifted(by: f64) -> Option<Affine> {
    Some(Affine {
        scale: Q::ONE,
        shift: Q::decimal(by)?.neg(),
    })
}

/// Which sample of its source each sample on `grid` reads: a time lands on the nearest sample
/// of the grid it steps its source on, ties to even; `None` where the instant moves.
pub(crate) fn landed(at: &When, grid: Grid) -> Option<sva_samples::Map> {
    match at {
        When::At(time) => grid.snapped(*time),
        When::Index(index) => index.map(grid),
        When::Moving(_) | When::Step(_) => None,
    }
}

/// Every offset from the sample being written that `step` lands at, where constants, maps
/// and clamps by constants bound it; the machine checks each read lands inside it.
pub(crate) fn reach(tys: &Typing, step: &Step, grid: Grid) -> Option<(i64, i64)> {
    match lines(tys, step, grid)? {
        (1, least, most) => Some((least, most)),
        _ => None,
    }
}

/// `(slope, least, most)`: `k - slope*n` lies in `[least, most]`.
fn lines(tys: &Typing, step: &Step, grid: Grid) -> Option<(i64, i64, i64)> {
    let each = |parts: &[Step]| {
        parts
            .iter()
            .map(|p| lines(tys, p, grid))
            .collect::<Option<Vec<_>>>()
    };
    match step {
        Step::Index(index) => {
            let map = index.map(grid)?;
            let slope = i64::try_from(map.a / map.d).ok()?;
            (map.a % map.d == 0).then(|| (slope, map.least(), map.lead()))
        }
        Step::Nearest(time, round) => {
            let (lo, hi) = offset(tys, *time)?;
            let sr = grid.sr();
            let whole = |v: f64| (v.abs() < 2f64.powi(62)).then_some(v as i64);
            // A step each side holds the time's rounding; a non-positive offset reads no later step.
            let most = whole((hi * sr).ceil() + 1.0)?;
            let most = match hi <= 0.0 && *round != Round::Ceil {
                true => most.min(0),
                false => most,
            };
            Some((1, whole((lo * sr).floor() - 1.0)?, most))
        }
        Step::Add(parts) => {
            each(parts)?
                .into_iter()
                .try_fold((0i64, 0i64, 0i64), |(s, l, m), (s2, l2, m2)| {
                    Some((s.checked_add(s2)?, l.checked_add(l2)?, m.checked_add(m2)?))
                })
        }
        Step::Neg(part) => {
            let (s, l, m) = lines(tys, part, grid)?;
            Some((s.checked_neg()?, m.checked_neg()?, l.checked_neg()?))
        }
        Step::Mul(parts) => each(parts)?
            .into_iter()
            .try_fold((0i64, 1i64, 1i64), |held, next| {
                let (c, (s, l, m)) = match (held, next) {
                    ((0, c, d), other) | (other, (0, c, d)) if c == d => (c, other),
                    _ => return None,
                };
                let (a, b) = (l.checked_mul(c)?, m.checked_mul(c)?);
                Some((s.checked_mul(c)?, a.min(b), a.max(b)))
            }),
    }
}

/// `[lo, hi]` holding `time - t` at every instant, where the machine computes it as written.
fn offset(tys: &Typing, time: NodeId) -> Option<(f64, f64)> {
    let form = crate::refs::substituted_closed_form(tys, time)?;
    let mut parts = Vec::new();
    addends(&form.body, &mut parts);
    let line = parts.iter().position(|b| matches!(b, Body::Line))?;
    let (lo, hi) = parts.iter().enumerate().filter(|(k, _)| *k != line).fold(
        (0.0, 0.0),
        |(lo, hi), (_, b)| {
            let (l, h) = span(b);
            (lo + l, hi + h)
        },
    );
    (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
}

fn addends<'a>(body: &'a Body, out: &mut Vec<&'a Body>) {
    match body {
        Body::Add(parts) => parts.iter().for_each(|p| addends(&p.body, out)),
        other => out.push(other),
    }
}

fn span(body: &Body) -> (f64, f64) {
    let each =
        |parts: &[sva_formula::Part]| parts.iter().map(|p| span(&p.body)).collect::<Vec<_>>();
    match body {
        Body::Const(c) if c.im == 0.0 => (c.re, c.re),
        Body::Add(parts) => each(parts)
            .into_iter()
            .fold((0.0, 0.0), |(lo, hi), (l, h)| (lo + l, hi + h)),
        Body::Mul(parts) => each(parts)
            .into_iter()
            .fold((1.0, 1.0), |(lo, hi), (l, h)| match (lo == hi, l == h) {
                (true, _) => scale((l, h), lo),
                (_, true) => scale((lo, hi), l),
                _ => (f64::NEG_INFINITY, f64::INFINITY),
            }),
        Body::Fold(Fold::Min, parts) => each(parts)
            .into_iter()
            .fold((f64::INFINITY, f64::INFINITY), |(lo, hi), (l, h)| {
                (lo.min(l), hi.min(h))
            }),
        Body::Fold(Fold::Max, parts) => each(parts).into_iter().fold(
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
            |(lo, hi), (l, h)| (lo.max(l), hi.max(h)),
        ),
        Body::Apply(Unary::Sin | Unary::Cos | Unary::Tanh | Unary::Sat, _) => (-1.0, 1.0),
        _ => (f64::NEG_INFINITY, f64::INFINITY),
    }
}

fn scale((lo, hi): (f64, f64), c: f64) -> (f64, f64) {
    match c {
        0.0 => (0.0, 0.0),
        c if c > 0.0 => (lo * c, hi * c),
        c => (hi * c, lo * c),
    }
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
