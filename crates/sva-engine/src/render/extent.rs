// Concern: the grid samples each node is computed over, support met with demand | Non-concern: computing them, where the root's demand ends (until.rs) | IO: (&Render, demands) -> an Extent per node

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Body, C64, Held, NodeId, Unary};
use sva_samples::{Extent, NodeRenderer};

use super::Render;
use super::pointwise::{self, Point};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::schedule;
use crate::typing::{Typing, Value};

/// Where each node can be nonzero: outside its support a node is exactly zero.
pub(crate) struct Supports<'a> {
    tys: &'a Typing,
    rate: u32,
    held: RefCell<BTreeMap<NodeId, Extent>>,
    open: RefCell<BTreeSet<NodeId>>,
}

impl<'a> Supports<'a> {
    pub(crate) fn new(tys: &'a Typing, rate: u32) -> Supports<'a> {
        Supports {
            tys,
            rate,
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
        self.open.borrow_mut().remove(&id);
        self.held.borrow_mut().insert(id, found);
        found
    }

    fn fresh(&self, id: NodeId) -> Extent {
        match self.tys.value(id) {
            Value::ClosedForm(form) => self.body(&form.body),
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

    fn body(&self, body: &Body) -> Extent {
        let each = |parts: &[sva_formula::Part]| -> Vec<Extent> {
            parts.iter().map(|p| self.body(&p.body)).collect()
        };
        match body {
            Body::Const(c) if *c == C64::ZERO => Extent::NOWHERE,
            Body::Node(id) => self.of(*id),
            Body::Add(parts) | Body::Join(parts) => {
                each(parts).into_iter().fold(Extent::NOWHERE, Extent::hull)
            }
            Body::Mul(parts) => each(parts)
                .into_iter()
                .fold(Extent::EVERYWHERE, Extent::intersect),
            Body::Div(num, den) if matches!(*den.body, Body::Const(c) if c != C64::ZERO) => {
                self.body(&num.body)
            }
            Body::Pow(base, n) if *n > 0 => self.body(&base.body),
            Body::Apply(op, arg) if keeps_zero(*op) => self.body(&arg.body),
            Body::Shift { by, of } => moved(self.body(&of.body), by * f64::from(self.rate)),
            Body::Crop { of, l, r, .. } => {
                self.body(&of.body)
                    .intersect(window(self.rate, l.value(), r.value()))
            }
            Body::Channel(of, _) => self.body(&of.body),
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
    let supports = Supports::new(&held.tys, held.config.rate);
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
/// cut to the support would take another row than the span-by-span one a stream reads.
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
