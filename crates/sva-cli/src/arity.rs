// Concern: refuses a builtin call given arguments its builtin does not read | Non-concern: other lint rules, what a call lowers to (sva-engine) | IO: (&Graph) -> Vec<LintViolation>

use sva_ast::{Arg, Binds, Expr, Graph, children};
use sva_core::{LintCode, LintViolation, Severity};

/// The check a render lowers each call under.
pub fn arity_violations(graph: &Graph) -> Vec<LintViolation> {
    let mut out = Vec::new();
    for path in graph.paths() {
        let Some(expr) = graph.expr(path) else {
            continue;
        };
        calls(expr, &mut |name, args| {
            let positional = args.iter().filter(|a| matches!(a, Arg::Pos(_))).count();
            let keys: Vec<String> = args
                .iter()
                .filter_map(|a| match a {
                    Arg::Named(k, _) => Some(k.clone()),
                    Arg::Pos(_) => None,
                })
                .collect();
            if let Err(m) = sva_engine::overload::check_arity(name, positional, &keys) {
                out.push(LintViolation {
                    code: LintCode::Arity,
                    severity: Severity::Error,
                    subject: path.to_string(),
                    message: format!("`{path}` calls `{name}`: {}", m.repair),
                    line: None,
                });
            }
        });
    }
    out
}

fn calls(e: &Expr, found: &mut dyn FnMut(&str, &[Arg])) {
    if let Expr::Call { name, args, .. } = e {
        found(name, args);
    }
    for child in children(e, Binds::Substitute) {
        calls(child, found);
    }
}
