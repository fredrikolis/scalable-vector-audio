// Concern: proves from which sample on every later one of a target is under a level | Non-concern: bounding one node class (envelope.rs), where a render stops | IO: (target, level) -> a sample

mod envelope;
mod floor;
mod range;
mod ringing;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;

use sva_formula::NodeId;
use sva_samples::{Buffer, Extent};

pub(crate) use envelope::Live;
use envelope::{Bounds, Envelope, Forms, Grid, STEP, Unbounded};

use super::{Lenses, Render, RenderConfig, materialize};
use crate::error::{Diagnostic, EngineError, Located};
use crate::schedule::{self, Schedule};

/// Where a proof found every later sample under its level, and its bound from each instant.
pub(crate) struct Proven {
    pub(crate) at: i64,
    first: i64,
    bounds: Vec<f64>,
}

impl Proven {
    pub(crate) fn from(&self, n: i64) -> f64 {
        let j = ((n - self.first).max(0) as usize / STEP).min(self.bounds.len() - 1);
        self.bounds[j]
    }
}

/// The first instant from `first` on from which the root's bound stays under `level`.
pub(crate) fn proven_at(
    tys: &crate::typing::Typing,
    root: NodeId,
    config: &RenderConfig,
    first: i64,
    level: f64,
    limit: i64,
) -> (Result<Proven, EngineError>, u64) {
    let rate = f64::from(config.rate);
    let grid = Grid {
        first,
        rate,
        points: (limit - first).max(0) as usize / STEP + 1,
    };
    let rendered = |id: NodeId, end: f64| heard_alone(tys, id, config, first, end);
    let mut stepped = level;
    let forms = Forms::new(Cow::Borrowed(tys), Cow::Borrowed(config));
    let mut proofs = 0;
    loop {
        proofs += 1;
        let mut bounds = Bounds::new(&forms, grid.clone(), Some(&rendered), stepped);
        let envelope = match bounded(tys, root, &mut bounds, level) {
            Ok(envelope) => envelope,
            Err(e) => return (Err(e), proofs),
        };
        if let Some(j) = envelope.at.iter().position(|v| *v < level) {
            let at = first + (j * STEP) as i64;
            let bounds = envelope.at;
            return (Ok(Proven { at, first, bounds }), proofs);
        }
        let last = envelope.at.last().copied().unwrap_or(f64::INFINITY);
        if !bounds.held_flat || !last.is_finite() {
            let limit_secs = limit as f64 / rate;
            return (
                Err(not_proven_by(tys, root, Some(last), level, limit_secs)),
                proofs,
            );
        }
        // Each round at least halves the level, so a held bound is stepped past, or none holds.
        stepped *= level / last / 2.0;
    }
}

/// For each of `ids` a bound reaches without a sample computed, the first grid instant from
/// t = 0 on it stays under `level` from, in seconds, within `limit_secs`.
pub(crate) fn quiet_from(
    tys: &crate::typing::Typing,
    config: &RenderConfig,
    ids: &[NodeId],
    level: f64,
    limit_secs: f64,
) -> BTreeMap<NodeId, f64> {
    let rate = f64::from(config.rate);
    let grid = Grid {
        first: 0,
        rate,
        points: (limit_secs * rate) as usize / STEP + 1,
    };
    let forms = Forms::new(Cow::Borrowed(tys), Cow::Borrowed(config));
    let mut bounds = Bounds::new(&forms, grid, None, level);
    let mut out = BTreeMap::new();
    for &id in ids {
        let Ok(Ok(envelope)) = bounds.of(id) else {
            continue;
        };
        if envelope.floor >= level {
            continue;
        }
        if let Some(j) = envelope.at.iter().position(|v| *v < level) {
            out.insert(id, (j * STEP) as f64 / rate);
        }
    }
    out
}

/// What no block changes, kept across a stream's proofs: nodes heard alone, forms compiled.
pub(crate) struct Kept {
    heard: RefCell<BTreeMap<(NodeId, i64, u64), Buffer>>,
    forms: Forms<'static>,
}

impl Kept {
    pub(crate) fn new(tys: crate::typing::Typing, config: RenderConfig) -> Kept {
        Kept {
            heard: RefCell::default(),
            forms: Forms::new(Cow::Owned(tys), Cow::Owned(config)),
        }
    }
}

/// A bound on every sample of `root` from `now` on, from the states `live` holds there.
pub(crate) fn bound_from(
    kept: &Kept,
    root: NodeId,
    level: f64,
    live: &dyn Live,
    now: i64,
) -> Result<f64, EngineError> {
    let (tys, config) = (&*kept.forms.tys, &*kept.forms.config);
    let heard = &kept.heard;
    let grid = Grid {
        first: now,
        rate: f64::from(config.rate),
        points: 1,
    };
    let rendered = |id: NodeId, end: f64| {
        let key = (id, now, end.to_bits());
        if let Some(buffer) = heard.borrow().get(&key) {
            return Ok(buffer.clone());
        }
        let buffer = heard_alone(tys, id, config, now, end)?;
        heard.borrow_mut().insert(key, buffer.clone());
        Ok(buffer)
    };
    let mut bounds = Bounds::new(&kept.forms, grid, Some(&rendered), level);
    bounds.live = Some(live);
    Ok(bounded(tys, root, &mut bounds, level)?.at[0])
}

fn bounded(
    tys: &crate::typing::Typing,
    root: NodeId,
    bounds: &mut Bounds,
    level: f64,
) -> Result<Envelope, EngineError> {
    let envelope = match bounds.of(root)? {
        Ok(envelope) => envelope,
        Err(Unbounded { node, class }) => {
            return Err(refusal(
                tys,
                root,
                "engine.no_tail_bound",
                format!(
                    "`{node}` is {class}, and no bound on its tail is derived yet, so no level \
                     is ever proven for it."
                ),
                "give the target's interval an end, or crop it to a window",
            ));
        }
    };
    if envelope.floor >= level {
        return Err(refusal(
            tys,
            root,
            "engine.never_silent",
            format!(
                "it returns to {} forever, at or above {}.",
                dbfs(envelope.floor),
                dbfs(level)
            ),
            "crop it, or give it a release",
        ));
    }
    Ok(envelope)
}

/// `last` is the bound the latest proof found, `None` where none has run.
pub(crate) fn not_proven_by(
    tys: &crate::typing::Typing,
    root: NodeId,
    last: Option<f64>,
    level: f64,
    limit_secs: f64,
) -> EngineError {
    let found = match last {
        Some(last) => format!(
            "its bound at {limit_secs}s is {}, not under {}",
            dbfs(last),
            dbfs(level)
        ),
        None => format!(
            "no block has ended by {limit_secs}s to prove it under {}",
            dbfs(level)
        ),
    };
    refusal(
        tys,
        root,
        "engine.not_silent_by",
        format!("{found}, so it is not proven quiet by then."),
        "if it decays, raise -c proof_limit, or give the interval an end",
    )
}

fn dbfs(v: f64) -> String {
    match v.is_finite() {
        true => format!("{:.1} dBFS", 20.0 * v.log10()),
        false => "unbounded".to_string(),
    }
}

fn refusal(
    tys: &crate::typing::Typing,
    root: NodeId,
    code: &str,
    message: String,
    help: &str,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(tys.name(root), None),
        help: help.to_string(),
    })
}

/// One node on its own from grid sample `first` to `end` seconds.
fn heard_alone(
    tys: &crate::typing::Typing,
    id: NodeId,
    config: &RenderConfig,
    first: i64,
    end: f64,
) -> Result<Buffer, EngineError> {
    let last = (end * f64::from(config.rate)).ceil() as i64;
    let demand = Extent::new(first, last.max(first));
    let mut held = Render::shell(
        tys.clone(),
        id,
        RenderConfig {
            asks: Vec::new(),
            ..config.clone()
        },
        Schedule::default(),
    );
    let order = schedule::dependencies_first(tys, id, &mut Default::default());
    held.extents = super::extent::decide(&held, &order, &[(id, demand)])?;
    materialize(&mut held, id, &Lenses::none())?;
    Ok(held.aligned(id, demand))
}
