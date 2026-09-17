// Concern: advises where a written number stands for the rate a render chooses | Non-concern: every other lint rule (lint.rs), what a rate means to a collapse | IO: (&Graph) -> Vec<Finding>

use sva_ast::{Binds, Expr, Graph, Literal, children};
use sva_core::{LintCode, Severity};

use crate::lint::Finding;

/// A render chooses its own, so a node written against one of these drifts at every other.
const WRITTEN: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];

pub fn rate_findings(graph: &Graph) -> Vec<Finding> {
    graph
        .paths()
        .filter_map(|path| {
            let found = literal(graph.expr(path)?)?;
            Some(Finding {
                code: LintCode::LiteralSampleRate,
                severity: Severity::Advice,
                subject: path.to_string(),
                message: format!(
                    "`{path}` writes `{found}`, which is a rate a render chooses, not a \
                     number this composition holds — write the sample period as `sp`: \
                     `x/{found}` is `x*1sp`, `t - 1/{found}` is `t - 1sp`, and \
                     `{found}*(a - b)` is `(a - b)/1sp`"
                ),
                line: None,
            })
        })
        .collect()
}

fn literal(e: &Expr) -> Option<f64> {
    if let Expr::Lit(Literal::Num(n)) = e
        && WRITTEN.contains(n)
    {
        return Some(*n);
    }
    children(e, Binds::Substitute).into_iter().find_map(literal)
}
