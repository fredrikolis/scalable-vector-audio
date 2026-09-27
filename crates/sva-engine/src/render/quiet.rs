// Concern: finds each instance computed past where its bound proves it quiet, rendering nothing | Non-concern: the bound itself (silent/), advising a crop | IO: (&Graph, target, level) -> Vec<QuietTail>

use sva_ast::Graph;
use sva_samples::Extent;

use super::extent::{self, Supports};
use super::{Render, RenderConfig, prepared, silent};
use crate::error::EngineError;
use crate::schedule::{self, Schedule};

/// Seconds in the instance's own time, infinite where unbounded.
#[derive(Clone, Debug, PartialEq)]
pub struct QuietTail {
    pub instance: String,
    pub file: String,
    pub quiet_from: Option<f64>,
    pub support: (f64, f64),
    /// `None` where extents refuse at this rate, or only its reader is computed.
    pub computed_until: Option<f64>,
}

/// A node no bound reaches without samples is never quiet here.
pub fn quiet_tails(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    level: f64,
    limit_secs: f64,
) -> Result<Vec<QuietTail>, EngineError> {
    let held = prepared(graph, target)?;
    let (tys, root, rate) = (&held.tys, held.root, config.rate);
    let named: Vec<_> = tys
        .paths()
        .filter(|(path, _)| held.instances.holds(path))
        .collect();
    let ids: Vec<_> = named.iter().map(|(_, id)| *id).collect();
    let quiet = silent::quiet_from(tys, &config, &ids, level, limit_secs);
    let supports = Supports::new(tys, rate);
    let support = supports.of(root);
    let range = Extent::new(
        extent::default_start(support),
        extent::default_end(support).unwrap_or(i64::MAX),
    );
    let shell = Render::shell(tys.clone(), root, config, Schedule::default());
    let order = schedule::dependencies_first(tys, root, &mut Default::default());
    let decided = extent::decide(&shell, &order, &[(root, range)]).ok();
    let secs = |e: Extent| {
        let edge = |n: i64, open: i64, inf: f64| match n == open {
            true => inf,
            false => n as f64 / f64::from(rate),
        };
        (
            edge(e.start, i64::MIN, f64::NEG_INFINITY),
            edge(e.end, i64::MAX, f64::INFINITY),
        )
    };
    Ok(named
        .into_iter()
        .map(|(path, id)| QuietTail {
            instance: path.to_string(),
            file: held.instances.origin(path).unwrap_or(path).to_string(),
            quiet_from: quiet.get(&id).copied(),
            support: secs(supports.of(id)),
            computed_until: decided
                .as_ref()
                .and_then(|d| d.decided.get(&id))
                .filter(|e| !e.is_empty())
                .map(|e| secs(*e).1),
        })
        .collect())
}
