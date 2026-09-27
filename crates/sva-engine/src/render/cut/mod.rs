// Concern: where each node's extent ends at the declared decay floor, before any sample | Non-concern: each class's bound (bound/), what a cut node's readers compute | IO: (&Render, nodes) -> Cuts

mod gain;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use sva_formula::NodeId;
use sva_samples::Extent;

use super::Render;
use super::bound::{Bounds, Envelope, Forms, Found, Grid, STEP};
use super::extent::{self, Supports};
use crate::error::{Diagnostic, EngineError, Located};
use gain::{Gain, Gains};

/// One node cut, and the grid sample its extent ends at.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub node: String,
    pub at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    Bound,
    Gain,
}

/// One node left uncut for want of a bound on it or on its gain to the output, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Uncut {
    pub node: String,
    pub missing: Missing,
    pub why: String,
}

/// The precision a render is written at, the floor its decaying nodes are cut at, as linear
/// amplitude, and every node cut or left uncut under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cuts {
    pub bits: i32,
    pub floor: f64,
    pub cut: Vec<Cut>,
    pub uncut: Vec<Uncut>,
}

/// What a decision found: where each cut node ends, and why an endless root has no end.
pub(crate) struct Decided {
    pub(crate) at: extent::Cuts,
    pub(crate) report: Cuts,
    pub(crate) passes: u64,
    pub(crate) ops: u128,
    pub(crate) endless: Option<EngineError>,
}

/// A node whose bound times gain is under `floor / N` from T on ends at T, `N` the count that
/// could be cut. With no end, the bounds look twice as far each pass until the root is cut.
pub(crate) fn decide(held: &Render, nodes: &[NodeId], start: i64) -> Result<Decided, EngineError> {
    let (tys, config, root) = (&held.tys, &held.config, held.root);
    let rate = config.rate;
    let resolution = config.profile.half_lsb();
    let floor = config.decay_floor.unwrap_or(resolution);
    if !(floor >= resolution && floor.is_finite()) {
        return Err(below_resolution(held, floor, resolution));
    }
    let supports = Supports::new(tys, rate);
    let root_end = config.range.end.unwrap_or(supports.of(root).end);
    let endless = root_end == i64::MAX;
    let demand = Extent::new(start, root_end.max(start));
    let uncut = extent::decide(held, nodes, &[(root, demand)], &extent::Cuts::new())?;
    let extents: Vec<Extent> = nodes
        .iter()
        .map(|id| uncut.of(*id))
        .filter(|e| !e.is_empty())
        .collect();
    let first = extents.iter().map(|e| e.start).fold(start, i64::min);
    let reach = |extents: &[Extent]| {
        extents
            .iter()
            .map(|e| e.end)
            .filter(|end| *end != i64::MAX)
            .fold(first + 1, i64::max)
    };
    let mut horizon = match endless {
        true => reach(&extents).max(first.saturating_add(i64::from(rate))),
        false => reach(&extents),
    };
    let forms = Forms::new(Cow::Borrowed(tys), Cow::Borrowed(config));
    let level = floor / nodes.len().max(1) as f64;
    let (mut ops, mut passes) = (0u128, 0u64);
    loop {
        passes += 1;
        let grid = Grid {
            first,
            rate: f64::from(rate),
            points: ((horizon - first) as usize).div_ceil(STEP) + 1,
        };
        let measure = |levels: BTreeMap<NodeId, f64>| {
            measured(held, nodes, &forms, &grid, &supports, level, levels)
        };
        let (mut found, mut gains, spent) = measure(BTreeMap::new())?;
        ops += spent;
        let share = floor / candidates(&found, &gains).max(1) as f64;
        let solvers: BTreeMap<NodeId, f64> = gains
            .iter()
            .filter(|(id, _)| matches!(tys.value(**id), crate::typing::Value::Solver(_)))
            .filter_map(|(id, g)| Some((*id, share / *g.as_ref().ok().filter(|g| **g > 1.0)?)))
            .collect();
        if !solvers.is_empty() {
            let spent;
            (found, gains, spent) = measure(solvers)?;
            ops += spent;
        }
        let chosen = Chosen::of(nodes, &found, &gains, &supports, &grid, floor);
        let exhausted = ops > config.flop_budget;
        let root_cut = chosen.at.get(&root).copied();
        let grown = match (endless, root_cut) {
            (false, _) => None,
            (true, Some(end)) => {
                let over = Extent::new(start, end.max(start));
                let decided = extent::decide(held, nodes, &[(root, over)], &chosen.at)?;
                let needed: Vec<Extent> = nodes.iter().map(|id| decided.of(*id)).collect();
                Some(reach(&needed)).filter(|need| *need > horizon)
            }
            (true, None) => {
                let root_found = &found[&root];
                let hopeless =
                    root_found.is_err() || never(root_found, floor / chosen.count.max(1) as f64);
                match exhausted || hopeless {
                    true => None,
                    false => Some(first + (horizon - first).saturating_mul(2)),
                }
            }
        };
        if let Some(next) = grown.filter(|_| !exhausted && next_fits(first, horizon)) {
            horizon = next;
            continue;
        }
        let endless = (endless && root_cut.is_none()).then(|| {
            no_end(
                held,
                &found[&root],
                &grid,
                floor / chosen.count.max(1) as f64,
            )
        });
        let report = Cuts {
            bits: config.profile.precision_bits,
            floor,
            cut: chosen
                .at
                .iter()
                .map(|(id, at)| Cut {
                    node: tys.name(*id).to_string(),
                    at: *at,
                })
                .collect(),
            uncut: uncut_of(held, nodes, &found, &gains),
        };
        return Ok(Decided {
            at: chosen.at,
            report,
            passes,
            ops,
            endless,
        });
    }
}

pub(crate) fn under(tys: &crate::typing::Typing, id: NodeId) -> BTreeSet<NodeId> {
    let (mut seen, mut work) = (BTreeSet::new(), vec![id]);
    while let Some(next) = work.pop() {
        if seen.insert(next) {
            work.extend(gain::operands(tys, next));
        }
    }
    seen
}

/// Each node's bound, its gain, and what finding them cost.
type Measured = (BTreeMap<NodeId, Found>, BTreeMap<NodeId, Gain>, u128);

/// Every node's bound on one grid, and its gain to the output. A solver's bound is stepped
/// down to `level`, or to its own in `levels` where its gain asks for a lower one.
fn measured(
    held: &Render,
    nodes: &[NodeId],
    forms: &Forms,
    grid: &Grid,
    supports: &Supports,
    level: f64,
    levels: BTreeMap<NodeId, f64>,
) -> Result<Measured, EngineError> {
    let mut bounds = Bounds::new(forms, grid.clone(), level);
    bounds.levels = levels;
    let mut found = BTreeMap::new();
    for &id in nodes {
        found.insert(id, bounds.of(id)?);
    }
    let perturbable: BTreeSet<NodeId> = found
        .iter()
        .filter(|(id, f)| f.is_ok() && **id != held.root)
        .map(|(id, _)| *id)
        .collect();
    let window = (grid.secs(0), grid.secs(grid.points - 1));
    let gains =
        Gains::new(&held.tys, &mut bounds, supports, &perturbable, window).from(held.root)?;
    Ok((found, gains, bounds.ops))
}

/// Every node a bound and a gain are derived for: what a cut's share divides the floor among.
fn candidates(found: &BTreeMap<NodeId, Found>, gains: &BTreeMap<NodeId, Gain>) -> usize {
    found
        .iter()
        .filter(|(id, f)| f.is_ok() && gains.get(id).is_some_and(|g| g.is_ok()))
        .count()
}

/// The most instants one grid holds, a few hours at any audio rate.
const MAX_POINTS: i64 = 1 << 22;

fn next_fits(first: i64, horizon: i64) -> bool {
    (horizon - first) / STEP as i64 <= MAX_POINTS / 2
}

/// The cut chosen at one grid: each node's first instant under its share of the floor.
/// `count` is every node a bound and a gain are derived for, which no interval changes, so a
/// node is cut at one instant however long a render or a stream of it reads.
struct Chosen {
    at: extent::Cuts,
    count: usize,
}

impl Chosen {
    fn of(
        nodes: &[NodeId],
        found: &BTreeMap<NodeId, Found>,
        gains: &BTreeMap<NodeId, Gain>,
        supports: &Supports,
        grid: &Grid,
        floor: f64,
    ) -> Chosen {
        let candidates: Vec<(NodeId, &Envelope, f64)> = nodes
            .iter()
            .filter_map(|id| {
                let envelope = found.get(id)?.as_ref().ok()?;
                let gain = *gains.get(id)?.as_ref().ok()?;
                Some((*id, envelope, gain))
            })
            .collect();
        let count = candidates.len();
        let share = floor / count.max(1) as f64;
        let at = candidates
            .iter()
            .filter_map(|(id, envelope, gain)| {
                let j = envelope.at.iter().position(|b| b * gain <= share)?;
                let at = grid.first + (j * STEP) as i64;
                (at < supports.of(*id).end).then_some((*id, at))
            })
            .collect();
        Chosen { at, count }
    }
}

/// A level the root provably returns to forever, at or above its share of the floor, or a
/// bound with no finite value to fall from.
fn never(root: &Found, share: f64) -> bool {
    root.as_ref()
        .is_ok_and(|e| e.floor >= share || !e.at.last().is_some_and(|v| v.is_finite()))
}

/// Every node the render holds whose bound, or whose gain to the output, is not derived.
fn uncut_of(
    held: &Render,
    nodes: &[NodeId],
    found: &BTreeMap<NodeId, Found>,
    gains: &BTreeMap<NodeId, Gain>,
) -> Vec<Uncut> {
    let mut out: Vec<Uncut> = nodes
        .iter()
        .filter_map(|id| {
            let node = held.tys.name(*id).to_string();
            match (&found[id], gains.get(id)) {
                (Err(unbounded), _) => Some(Uncut {
                    node,
                    missing: Missing::Bound,
                    why: format!("`{}` is {}", unbounded.node, unbounded.class),
                }),
                (Ok(_), Some(Err(why))) => Some(Uncut {
                    node,
                    missing: Missing::Gain,
                    why: why.clone(),
                }),
                _ => None,
            }
        })
        .collect();
    out.sort_by(|a, b| a.node.cmp(&b.node));
    out
}

/// Why an endless root has no end: no bound on it, a level it holds forever, or a bound
/// still over the floor where the budget ran out.
fn no_end(held: &Render, root: &Found, grid: &Grid, share: f64) -> EngineError {
    let name = held.tys.name(held.root);
    let (code, message, help) = match root {
        Err(unbounded) => (
            "render.no_bound",
            format!(
                "`{}` is {}, and no bound is derived for it, so `{name}` is never shown to end",
                unbounded.node, unbounded.class
            ),
            "give the interval an end, as `[0, 2s]`, or crop it",
        ),
        Ok(envelope) if !envelope.at.last().is_some_and(|v| v.is_finite()) => (
            "render.no_bound",
            format!("`{name}` has no finite bound, so it is never shown to end"),
            "give the interval an end, as `[0, 2s]`, or crop it",
        ),
        Ok(envelope) if envelope.floor >= share => (
            "render.never_ends",
            format!(
                "`{name}` returns to {} forever, at or above the decay floor's share {}",
                dbfs(envelope.floor),
                dbfs(share)
            ),
            "crop it, give it a release, or give the interval an end",
        ),
        Ok(envelope) => (
            "render.no_end",
            format!(
                "`{name}`'s bound at {:.3}s is {}, over the decay floor's share {}, and the \
                 budget ends the search there",
                grid.secs(grid.points - 1),
                dbfs(envelope.at.last().copied().unwrap_or(envelope.before)),
                dbfs(share)
            ),
            "raise --flop-budget to look further, raise --decay-floor, or give the interval \
             an end",
        ),
    };
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(name, None),
        help: help.to_string(),
    })
}

fn below_resolution(held: &Render, floor: f64, resolution: f64) -> EngineError {
    let name = held.tys.name(held.root);
    EngineError::refused(Diagnostic {
        code: "render.floor_below_resolution".to_string(),
        message: format!(
            "the decay floor {} is under the {} a {}-bit sample resolves",
            dbfs(floor),
            dbfs(resolution),
            held.config.profile.precision_bits
        ),
        location: Located::at(name, None),
        help: "raise --decay-floor to the resolution or above, or raise --bits".to_string(),
    })
}

pub(crate) fn dbfs(v: f64) -> String {
    match v.is_finite() {
        true => format!("{:.1} dBFS", 20.0 * v.log10()),
        false => "unbounded".to_string(),
    }
}
