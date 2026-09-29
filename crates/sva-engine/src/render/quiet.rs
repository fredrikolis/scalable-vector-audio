// Concern: finds each instance whose extent runs well past where its bound is under 24 bits | Non-concern: the bound (bound/), what lint says of it | IO: (&Graph, target) -> Vec<QuietTail>

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::NodeId;

use super::bound::Tail;
use super::table::Table;
use super::{Ends, Render, RenderConfig, prepared, range_of};
use crate::error::EngineError;
use crate::schedule;

/// Half a 24-bit step.
pub const QUIET_LEVEL: f64 = 1.0 / (1u64 << 24) as f64;

pub const QUIET_AFTER_SECS: f64 = 1.0;

/// Seconds in the instance's own time.
#[derive(Clone, Debug, PartialEq)]
pub struct QuietTail {
    pub instance: String,
    pub file: String,
    pub quiet_from: f64,
    pub runs_to: f64,
}

/// A root with no end is read on, as a stream reads it. An instance inherits the tail of
/// one it reads, so only that one is named.
pub fn quiet_tails(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
) -> Result<Vec<QuietTail>, EngineError> {
    let held = prepared(graph, target, config.rate)?;
    let schedule = schedule::plan(&held.tys, held.root, &[]);
    let shell = Render::shell(held.tys.clone(), held.root, config, schedule);
    let range = range_of(&shell, Ends::Pulled)?;
    let table = Table::build(&shell.tys, shell.root, &[], &shell.config.profile)?;
    let needs = table.demand(range);
    let mut out = BTreeMap::new();
    for (path, id) in held.tys.paths().filter(|(p, _)| held.instances.holds(p)) {
        let Some(at) = table.of(id) else {
            continue;
        };
        let asked = needs[at].hold.hull();
        if asked.is_empty() {
            continue;
        }
        let grid = shell.grid(id);
        let edge = |n: i64| match n {
            i64::MIN => f64::NEG_INFINITY,
            i64::MAX => f64::INFINITY,
            n => grid.instant(n),
        };
        let (from, to) = (edge(asked.start), edge(asked.end));
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

/// Every node `id` reads, however deep.
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
