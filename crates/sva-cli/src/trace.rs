// Concern: seeds a trace from every entry point and names what the caller asked for | Non-concern: the traversal itself (sva-engine), JSON shape (output.rs) | IO: (dir, target) -> Traced or CliError

use std::path::Path;

use sva_ast::{Expr, Graph, Literal};
use sva_core::{CliError, PROBE, define_probe_for, prepared, refuse_unresolved_bars};
use sva_engine::{EngineError, Traced};

use crate::lint::{entry_points, referenced};

pub struct Traceable {
    pub bpm: Option<f64>,
    pub meter: Option<String>,
    pub traced: Traced,
}

/// Every entry point is a root: `master` has no privilege, an uninstantiated bench refuses.
pub fn trace(dir: &Path, target: &str) -> Result<Traceable, CliError> {
    let mut graph = prepared(&sva_ast::Dir::at(dir))?;
    refuse_unresolved_bars(&graph)?;

    let mut roots = entry_points(&graph);
    if graph.defines(target)
        && !roots.iter().any(|r| r == target)
        && !referenced(&graph).contains(target)
    {
        roots.push(target.to_string());
    }
    let traced = match sva_engine::trace(&graph, &roots, target) {
        Err(EngineError::UnknownNode(ref name)) if name == target && !graph.defines(target) => {
            probe(&mut graph, target)?
        }
        other => other.map_err(CliError::Engine)?,
    };
    Ok(Traceable {
        bpm: number(graph.global("bpm")),
        meter: text(graph.global("meter")),
        traced,
    })
}

/// `text` is neither node nor instance: argv math, an entry point of its own.
fn probe(graph: &mut Graph, text: &str) -> Result<Traced, CliError> {
    define_probe_for(graph, text)?;
    let roots = entry_points(graph);
    sva_engine::trace(graph, &roots, PROBE).map_err(CliError::Engine)
}

fn number(e: Option<&Expr>) -> Option<f64> {
    match e {
        Some(Expr::Lit(Literal::Num(v))) => Some(*v),
        _ => None,
    }
}

fn text(e: Option<&Expr>) -> Option<String> {
    match e {
        Some(Expr::Lit(Literal::Str(v))) => Some(v.clone()),
        _ => None,
    }
}
