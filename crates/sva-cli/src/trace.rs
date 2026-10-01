// Concern: seeds a trace from every entry point and names what the caller asked for | Non-concern: the traversal itself (sva-engine), JSON shape (output.rs) | IO: (dir, target) -> Traced or CliError

use std::path::Path;

use std::collections::BTreeSet;

use sva_ast::{Binds, Expr, Graph, Literal, children, resolve_ref_path};
use sva_core::{CliError, PROBE, define_probe_for, prepared, refuse_unresolved_bars};
use sva_engine::{EngineError, Traced};

pub struct Traceable {
    pub bpm: Option<f64>,
    pub meter: Option<String>,
    pub traced: Traced,
}

/// Every entry point is a root, and an uninstantiated bench refuses. A structure has no
/// time, so an interval the target reads is dropped.
pub fn trace(dir: &Path, target: &str) -> Result<Traceable, CliError> {
    let target = match sva_core::target(target)? {
        sva_core::Target {
            expr,
            interval: Some(_),
        } => expr,
        _ => target.to_string(),
    };
    let target = target.as_str();
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

fn referenced(graph: &Graph) -> BTreeSet<String> {
    let mut reached: BTreeSet<String> = BTreeSet::new();
    for path in graph.paths() {
        if let Some(expr) = graph.expr(path) {
            collect_refs(path, expr, &mut reached);
        }
    }
    reached
}

/// Every node nothing references, bar a library node (one with a free parameter).
pub(crate) fn entry_points(graph: &Graph) -> Vec<String> {
    let reached = referenced(graph);
    graph
        .paths()
        .filter(|p| !reached.contains(*p))
        .filter(|p| !reserved_variable(p))
        .filter(|p| !sva_engine::instantiate::has_free_parameter(graph, p))
        .map(str::to_string)
        .collect()
}

pub(crate) fn reserved_variable(path: &str) -> bool {
    sva_core::RESERVED_VARIABLES.contains(&path.rsplit('/').next().unwrap_or(path))
}

fn collect_refs(from: &str, expr: &Expr, out: &mut BTreeSet<String>) {
    if let Expr::Ref { path, .. } = expr
        && let Some(resolved) = resolve_ref_path(from, path)
    {
        out.insert(resolved);
    }
    for child in children(expr, Binds::Substitute) {
        collect_refs(from, child, out);
    }
}
