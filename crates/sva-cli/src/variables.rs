// Concern: what the reserved `variables/key` node must hold | Non-concern: the tempo variables (sva-core), the rest of the lint pass (lint.rs) | IO: (&Graph) -> Vec<Finding>

use sva_ast::{Binds, Expr, Graph, Literal, children};
use sva_core::{LintCode, Severity};

use crate::lint::Finding;

const KEY: &str = "variables/key";

/// One pitch, which every `st` offset written against it reads it as.
pub fn key_findings(graph: &Graph) -> Vec<Finding> {
    let Some(expr) = graph.expr(KEY) else {
        return Vec::new();
    };
    if pitch(expr) {
        return Vec::new();
    }
    vec![Finding {
        code: LintCode::KeyIsNotAPitch,
        severity: Severity::Warning,
        subject: KEY.to_string(),
        message: format!(
            "`{KEY}` holds `{}`; a key is one note name or a number of hertz, and every \
             `st` offset written against it reads it as one",
            sva_ast::render_expr(expr)
        ),
        line: None,
    }]
}

fn pitch(e: &Expr) -> bool {
    match e {
        Expr::Lit(Literal::Num(_)) => true,
        Expr::Var(name) => sva_formula::note::frequency(name).is_some(),
        Expr::Bin(..) => children(e, Binds::Substitute).into_iter().all(pitch),
        _ => false,
    }
}
