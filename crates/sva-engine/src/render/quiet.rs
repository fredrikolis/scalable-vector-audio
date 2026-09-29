// Concern: finds each instance whose extent runs well past where its bound is under 24 bits | Non-concern: the bound (bound/), what lint says of it | IO: (&Graph, target) -> Vec<QuietTail>

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::{Body, NodeId, closed_form};

use super::bound::Tail;
use super::extent::Supports;
use super::{Render, RenderConfig, prepared, reach};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::schedule;
use crate::time::Q;
use crate::typing::{Value, When};

/// Half a 24-bit step.
pub const QUIET_LEVEL: f64 = 1.0 / (1u64 << 24) as f64;

pub const QUIET_AFTER_SECS: f64 = 1.0;

/// Seconds in the instance's own time.
#[derive(Clone, Debug, PartialEq)]
pub struct QuietTail {
    pub instance: String,
    pub file: String,
    pub quiet_from: f64,
    /// Infinite where its extent never ends.
    pub runs_to: f64,
}

/// A root with no end is read on, as a stream reads it. An instance reading one named here
/// inherits its tail, so only the one it inherits it from is named.
pub fn quiet_tails(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
) -> Result<Vec<QuietTail>, EngineError> {
    let held = prepared(graph, target, config.profile)?;
    let schedule = schedule::plan(&held.tys, &held.order, held.root, &[]);
    let audio = schedule.materialize.clone();
    let mut shell = Render::shell(held.tys.clone(), held.root, config, schedule);
    reach::streamed(&mut shell, &audio)?;
    let extents = extents(&shell);
    let mut out = BTreeMap::new();
    for (path, id) in held.tys.paths().filter(|(p, _)| held.instances.holds(p)) {
        let Some(&(from, to)) = extents.get(&id).filter(|(from, to)| from < to) else {
            continue;
        };
        let Some(tail) = Tail::of(&held.tys, &shell.config, id) else {
            continue;
        };
        if let Some(quiet_from) = quiet(&tail, from.max(0.0), to) {
            let file = held.instances.origin(path).unwrap_or(path).to_string();
            let tail = QuietTail {
                instance: path.to_string(),
                file,
                quiet_from,
                runs_to: to,
            };
            out.insert(id, tail);
        }
    }
    let named: BTreeSet<NodeId> = out.keys().copied().collect();
    Ok(out
        .into_iter()
        .filter(|(id, _)| under(&held.tys, *id).is_disjoint(&named))
        .map(|(_, tail)| tail)
        .collect())
}

type Span = (f64, f64);

const EVERYWHERE: Span = (f64::NEG_INFINITY, f64::INFINITY);

/// Each instance's extent in seconds: a materialized one's as decided, a substituted one's
/// the hull of what its readers ask of it through their crops and shifts; each met with its
/// support. A transform, a warp or a derivative asks its whole support.
fn extents(shell: &Render) -> BTreeMap<NodeId, Span> {
    let tys = &shell.tys;
    let rate = f64::from(shell.lattice());
    let supports = Supports::new(shell);
    let secs = |e: sva_samples::Extent| -> Span {
        let edge = |n: i64| match n {
            i64::MIN => f64::NEG_INFINITY,
            i64::MAX => f64::INFINITY,
            n => n as f64 / rate,
        };
        match e.is_empty() {
            true => (0.0, 0.0),
            false => (edge(e.start), edge(e.end)),
        }
    };
    let mut order = Vec::new();
    readers_last(tys, shell.root, &mut BTreeSet::new(), &mut order);
    let mut asked: BTreeMap<NodeId, Span> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for &id in order.iter().rev() {
        let support = secs(supports.of(id));
        let wanted = match shell.extents.decided.get(&id) {
            Some(decided) => secs(*decided),
            None => asked.get(&id).copied().unwrap_or((0.0, 0.0)),
        };
        let own = met(wanted, support);
        out.insert(id, own);
        let mut ask = |child: NodeId, span: Span| {
            let slot = asked.entry(child).or_insert((0.0, 0.0));
            *slot = hull(*slot, span);
        };
        match tys.value(id) {
            Value::ClosedForm(form) => reads(&form.body, own, 0.0, &mut ask),
            Value::Read {
                source,
                at: When::Time(time),
                ..
            } if time.scale == Q::ONE => {
                let by = time.shift.to_f64();
                ask(*source, (own.0 + by, own.1 + by));
            }
            Value::Read { source, .. } => ask(*source, EVERYWHERE),
            Value::Cast(Cast::Stft { .. } | Cast::Fourier | Cast::IFourier, source) => {
                ask(*source, EVERYWHERE)
            }
            _ => {
                for child in schedule::read_operands(tys, id) {
                    ask(child, own);
                }
            }
        }
    }
    out
}

/// `span` in the reader's own time, and the time at this point of its body `by` past it.
fn reads(body: &Body, span: Span, by: f64, ask: &mut dyn FnMut(NodeId, Span)) {
    match body {
        Body::Node(child) => ask(*child, (span.0 + by, span.1 + by)),
        Body::Crop { of, l, r, .. } => {
            let window = (l.value() - by, r.value() - by);
            reads(&of.body, met(span, window), by, ask);
        }
        Body::Shift { by: moved, of } => reads(&of.body, span, by - moved, ask),
        Body::Warp { .. } | Body::Deriv { .. } => {
            let mut whole = |child: NodeId, _: Span| ask(child, EVERYWHERE);
            for part in closed_form::children(body) {
                reads(&part.body, span, by, &mut whole);
            }
        }
        _ => {
            for part in closed_form::children(body) {
                reads(&part.body, span, by, ask);
            }
        }
    }
}

fn readers_last(
    tys: &crate::typing::Typing,
    id: NodeId,
    seen: &mut BTreeSet<NodeId>,
    out: &mut Vec<NodeId>,
) {
    if !seen.insert(id) {
        return;
    }
    for child in schedule::read_operands(tys, id) {
        readers_last(tys, child, seen, out);
    }
    out.push(id);
}

fn met(a: Span, b: Span) -> Span {
    (a.0.max(b.0), a.1.min(b.1))
}

fn hull(a: Span, b: Span) -> Span {
    match (a.0 < a.1, b.0 < b.1) {
        (false, _) => b,
        (_, false) => a,
        _ => (a.0.min(b.0), a.1.max(b.1)),
    }
}

/// Every node `id` reads, however deep, and not `id` itself.
fn under(tys: &crate::typing::Typing, id: NodeId) -> BTreeSet<NodeId> {
    let (mut seen, mut work) = (BTreeSet::new(), schedule::read_operands(tys, id));
    while let Some(next) = work.pop() {
        if next != id && seen.insert(next) {
            work.extend(schedule::read_operands(tys, next));
        }
    }
    seen
}

/// Where the bound falls under the level, to the millisecond, if more than
/// [`QUIET_AFTER_SECS`] before `to`; a node under it from its start has no tail.
fn quiet(tail: &Tail, from: f64, to: f64) -> Option<f64> {
    let under = |t: f64| tail.from(t) < QUIET_LEVEL;
    let last = match to.is_finite() {
        true => to - QUIET_AFTER_SECS,
        false => (0..24)
            .map(|k| from + f64::from(1 << k))
            .find(|t| under(*t))?,
    };
    if last < from || !under(last) {
        return None;
    }
    let (mut lo, mut hi) = (from, last);
    if under(lo) {
        return None;
    }
    while hi - lo > 1e-3 {
        let mid = 0.5 * (lo + hi);
        match under(mid) {
            true => hi = mid,
            false => lo = mid,
        }
    }
    Some(hi)
}
