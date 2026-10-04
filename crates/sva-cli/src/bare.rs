// Concern: whether each node plays as a bare `@path`: every parameter defaulted, its support ending | Non-concern: deciding either (sva-ast, sva-engine) | IO: (&Graph) -> Vec<Finding>

use sva_ast::Graph;
use sva_core::{LintCode, Severity};
use sva_engine::{DEFAULT_SAMPLE_RATE, RenderConfig};

use crate::lint::Finding;

/// Every node but a reserved variable is a sound a registry may play on its own.
pub fn bare_findings(graph: &Graph) -> Vec<Finding> {
    let sounds: Vec<&str> = graph
        .paths()
        .filter(|p| !crate::trace::reserved_variable(p))
        .collect();
    let mut findings = Vec::new();
    let mut whole = Vec::new();
    for path in sounds {
        let free = graph.free_parameters(path);
        if free.is_empty() {
            whole.push(path.to_string());
        }
        findings.extend(free.into_iter().map(|name| no_default(path, &name)));
    }
    findings.extend(endless(graph, &whole).into_iter().map(never_ends));
    findings
}

fn no_default(path: &str, name: &str) -> Finding {
    Finding {
        code: LintCode::ParameterHasNoDefault,
        severity: Severity::Warning,
        subject: path.to_string(),
        message: format!(
            "`{path}` reads the parameter `{name}`, which no default line binds, so a bare \
             `@{path}` cannot play — write `{name} = <value>` above the expression"
        ),
        line: None,
    }
}

fn never_ends(path: String) -> Finding {
    Finding {
        code: LintCode::SupportNeverEnds,
        severity: Severity::Warning,
        message: format!(
            "`{path}`'s support never ends, even cut where its bound falls under the silence threshold, so a bare \
             `@{path}` has no end to render to"
        ),
        subject: path,
        line: None,
    }
}

/// Typed together, else each alone; one that refuses alone is the render's to report.
fn endless(graph: &Graph, paths: &[String]) -> Vec<String> {
    let config = RenderConfig::at(DEFAULT_SAMPLE_RATE);
    let ends: Vec<Option<Option<i64>>> = match sva_engine::ends(graph, paths, &config) {
        Ok(ends) => ends.into_iter().map(Some).collect(),
        Err(_) => paths
            .iter()
            .map(|p| {
                let one = sva_engine::ends(graph, std::slice::from_ref(p), &config);
                one.ok().and_then(|e| e.first().copied())
            })
            .collect(),
    };
    paths
        .iter()
        .zip(ends)
        .filter(|(_, end)| *end == Some(None))
        .map(|(path, _)| path.clone())
        .collect()
}
