// Concern: enumerates the pipeline error variants, exit codes, JSON codes and diagnostics | Non-concern: JSON envelope framing (output.rs), argv (sva-cli) | IO: none

use std::fmt;

use sva_ast::Refusal;
use sva_engine::EngineError;

use crate::lint_code::LintCode;
use crate::output::{Diagnostic, Severity};

/// One `lint` finding: its code, how hard it lands, and the node it is against.
#[derive(Debug)]
pub struct LintViolation {
    pub code: LintCode,
    pub severity: Severity,
    pub subject: String,
    pub message: String,
    pub line: Option<usize>,
}

/// Every expected failure mode this crate can hit; none of them a panic.
#[derive(Debug)]
pub enum CliError {
    /// An argument mistake: `code` names which, `message` quotes the text it refuses.
    Usage {
        code: &'static str,
        message: String,
    },
    Refusals(Vec<Refusal>),
    Engine(EngineError),
    BadTempo(String),
    BadProbe(String),
    Io(String),
    /// A path the caller named that nothing answers for; an `Io` is one that is there.
    NotFound(String),
    /// A name already taken, as opposed to an `Io` the filesystem raised.
    Conflict {
        by: &'static str,
        message: String,
    },
    /// Every `LintViolation` the scan found, not just the first.
    LintRefused(Vec<LintViolation>),
}

impl CliError {
    pub fn usage(code: &'static str, message: impl Into<String>) -> CliError {
        CliError::Usage {
            code,
            message: message.into(),
        }
    }
}

impl From<EngineError> for CliError {
    fn from(e: EngineError) -> CliError {
        CliError::Engine(e)
    }
}

fn engine_error_path(e: &EngineError) -> Option<String> {
    match e {
        EngineError::Binding { node, .. } => Some(node.clone()),
        EngineError::UnknownNode(p)
        | EngineError::AmbiguousNode(p, _)
        | EngineError::RefAboveRoot(p, _) => Some(p.clone()),
        other => other.at().map(|at| at.node.clone()),
    }
}

fn engine_error_span(e: &EngineError) -> Option<(usize, usize)> {
    let span = match e {
        EngineError::Binding { span, .. } => *span,
        other => other.at().and_then(|at| at.span),
    }?;
    Some((span.start, span.end - span.start))
}

fn names_no_node(e: &EngineError) -> bool {
    matches!(e, EngineError::UnknownNode(_))
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::Engine(e) if names_no_node(e) => 24,
            CliError::Usage { .. }
            | CliError::Refusals(_)
            | CliError::Engine(_)
            | CliError::BadTempo(_)
            | CliError::BadProbe(_)
            | CliError::LintRefused(_) => 3,
            CliError::Conflict { .. } => 4,
            CliError::NotFound(_) => 24,
            CliError::Io(_) => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            CliError::Engine(e) if names_no_node(e) => "not_found",
            CliError::Usage { .. }
            | CliError::Refusals(_)
            | CliError::Engine(_)
            | CliError::BadTempo(_)
            | CliError::BadProbe(_)
            | CliError::LintRefused(_) => "validation_error",
            CliError::Conflict { .. } => "conflict",
            CliError::NotFound(_) => "not_found",
            CliError::Io(_) => "internal_error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            CliError::Usage { message, .. } => message.clone(),
            CliError::Refusals(rs) => format!("{} refusal(s) parsing the composition", rs.len()),
            CliError::Engine(e) => e.to_string(),
            CliError::BadTempo(m) | CliError::BadProbe(m) | CliError::Io(m) => m.clone(),
            CliError::NotFound(m) => m.clone(),
            CliError::Conflict { message, .. } => message.clone(),
            CliError::LintRefused(vs) => format!(
                "{} lint violation(s)",
                vs.iter().filter(|v| v.severity == Severity::Error).count()
            ),
        }
    }

    /// Per-finding detail; empty only where the failure has no subject to locate it against.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            CliError::Refusals(rs) => rs
                .iter()
                .map(|r| {
                    Diagnostic::new(r.code.code_str(), r.reason.clone()).at(
                        Some(r.at.path.clone()),
                        Some((r.at.span.start, r.at.span.end - r.at.span.start)),
                    )
                })
                .collect(),
            CliError::Engine(e) => vec![engine_diagnostic(e)],
            CliError::BadTempo(m) => vec![
                Diagnostic::new("tempo.invalid_bpm_meter", m.clone()).helped(
                    "state `variables/bpm` as a positive number and `variables/meter` as \
                     `<beats>/<note>`, e.g. `4/4`",
                ),
            ],
            CliError::BadProbe(m) => vec![Diagnostic::new("probe.invalid_expression", m.clone())
                .helped("name a node path that exists, or an expression in the same grammar a node file holds")],
            CliError::Usage { code, message } => vec![Diagnostic::new(*code, message.clone())
                .helped("`sva-cli --help` states every subcommand, its arguments and their defaults")],
            CliError::Conflict { by, message } => vec![
                Diagnostic::new(format!("{by}.already_exists"), message.clone())
                    .helped("pick a name nothing occupies, or remove what is there first"),
            ],
            CliError::NotFound(m) => vec![
                Diagnostic::new("io.no_such_path", m.clone())
                    .helped("name a path that exists; `sva-cli lint` says what a composition holds"),
            ],
            CliError::LintRefused(vs) => vs
                .iter()
                .map(|v| lint_diagnostic(v.code, &v.subject, &v.message, v.severity, v.line))
                .collect(),
            CliError::Io(_) => Vec::new(),
        }
    }
}

/// `Display` runs a refusal's three parts together; a diagnostic keeps each apart.
fn engine_diagnostic(e: &EngineError) -> Diagnostic {
    let held = match e {
        EngineError::Refused(d) => {
            Diagnostic::new(d.code.clone(), d.message.clone()).helped(d.help.clone())
        }
        other => Diagnostic::new(other.code(), other.to_string()),
    };
    held.at(engine_error_path(e), engine_error_span(e))
}

/// The one place a `lint` code becomes a diagnostic, so both of `lint`'s response paths
/// namespace it, locate it and help against it identically.
pub fn lint_diagnostic(
    code: LintCode,
    subject: &str,
    message: &str,
    severity: Severity,
    line: Option<usize>,
) -> Diagnostic {
    Diagnostic::new(
        format!("lint.{}", code.code_str()).replace('-', "_"),
        message,
    )
    .with_severity(severity)
    .at(Some(subject.to_string()), None)
    .on_line(line)
    .helped(code.help())
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `cli` standard's table, and its rule that code and exit code always agree.
    #[test]
    fn every_variant_answers_the_standards_own_code_and_exit_code() {
        let cases: [(CliError, &str, u8); 7] = [
            (CliError::NotFound("gone".to_string()), "not_found", 24),
            (
                CliError::Engine(EngineError::UnknownNode("gone".to_string())),
                "not_found",
                24,
            ),
            (
                CliError::usage("cli.unknown_flag", "bad flag"),
                "validation_error",
                3,
            ),
            (
                CliError::BadTempo("bad bpm".to_string()),
                "validation_error",
                3,
            ),
            (
                CliError::Conflict {
                    by: "new",
                    message: "taken".to_string(),
                },
                "conflict",
                4,
            ),
            (CliError::Io("disk".to_string()), "internal_error", 1),
            (CliError::LintRefused(Vec::new()), "validation_error", 3),
        ];
        for (err, code, exit) in cases {
            assert_eq!(err.code(), code, "{err:?}");
            assert_eq!(err.exit_code(), exit, "{err:?}");
        }
    }

    /// Each argument mistake is its own code, its message the one line that names the text.
    #[test]
    fn an_argument_mistake_is_one_diagnostic_under_its_own_code() {
        let refused = CliError::usage("query.unknown_argument", "`lines` takes no arguments");
        assert_eq!(refused.message(), "`lines` takes no arguments");
        let [d] = &refused.diagnostics()[..] else {
            panic!("an argument mistake carries exactly one diagnostic");
        };
        assert_eq!(d.code, "query.unknown_argument");
        assert_eq!(d.message, "`lines` takes no arguments");
        assert!(d.help.is_some(), "and says where to look");
    }
}
