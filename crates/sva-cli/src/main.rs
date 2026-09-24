// Concern: dispatches one parsed subcommand and prints its envelope | Non-concern: parsing argv (args/), the pipeline and its errors (sva-core) | IO: (argv) -> stdout JSON + an exit code

use std::process::ExitCode;

use sva_core::Diagnostic;

use sva_cli::{
    CliError, Command, Finding, Format, NAME, VERSION, analyze, builtins, builtins_data, caught,
    colored, composition, cwd, diagnostics_text, error_envelope, help_data, help_text, lint,
    lint_data, new_data, parse_args, render, scaffold, stopped, success_envelope, trace,
    trace_data, version_data,
};

/// Parsing, desugaring and dropping a node's expression all recurse to its depth, which `sum`
/// lets an author choose. The process's own stack is fixed before it starts; a thread's is not.
const STACK_BYTES: usize = 128 << 20;

fn main() -> ExitCode {
    match caught(STACK_BYTES, run) {
        Ok(code) => code,
        Err(why) => {
            println!("{}", stopped(&why));
            ExitCode::from(1)
        }
    }
}

fn run() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let parsed = parse_args(&argv);
    // The one evaluator subcommand renders its findings either way; nothing else takes `--format`.
    let format = match &parsed {
        Ok(Command::Lint { format, .. }) => *format,
        _ => Format::Json,
    };
    let outcome = match parsed {
        Ok(Command::Version) => Ok(success_envelope(&version_data(NAME, VERSION), &[])),
        Ok(Command::Help) => Ok(success_envelope(&help_data(&help_text()), &[])),
        Ok(Command::Render(args)) => render(&args),
        Ok(Command::Analyze(args)) => analyze(&args),
        Ok(Command::Lint {
            target,
            dir,
            format,
        }) => lint_composition(target.as_deref(), dir.as_deref(), format),
        Ok(Command::Trace { target, dir }) => trace_node(&target, dir.as_deref()),
        Ok(Command::Builtins) => Ok(success_envelope(&builtins_data(&builtins()), &[])),
        Ok(Command::Outline { text }) => {
            sva_core::outline_data(&text).map(|data| success_envelope(&data, &[]))
        }
        Ok(Command::New {
            name,
            idempotency_key,
        }) => scaffold_new(&name, idempotency_key.as_deref()),
        Err(e) => Err(e),
    };

    match outcome {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            match format {
                Format::Text => {
                    let found = match e.diagnostics() {
                        held if held.is_empty() => vec![Diagnostic::new(e.code(), e.message())],
                        held => held,
                    };
                    println!("{}", diagnostics_text(&found, colored()));
                }
                Format::Json => println!(
                    "{}",
                    error_envelope(e.code(), &e.message(), &e.diagnostics())
                ),
            }
            ExitCode::from(e.exit_code())
        }
    }
}

fn lint_composition(
    target: Option<&str>,
    named: Option<&str>,
    format: Format,
) -> Result<String, CliError> {
    let dir = composition(named)?;
    let report = lint(&dir, target)?;
    let found: Vec<Diagnostic> = report.findings.iter().map(Finding::diagnostic).collect();
    if format == Format::Text {
        return Ok(diagnostics_text(&found, colored()));
    }
    Ok(success_envelope(
        &lint_data(&dir.display().to_string(), target, report.nodes),
        &found,
    ))
}

fn trace_node(target: &str, named: Option<&str>) -> Result<String, CliError> {
    let traced = trace(&composition(named)?, target)?;
    Ok(success_envelope(&trace_data(&traced), &[]))
}

fn scaffold_new(name: &str, idempotency_key: Option<&str>) -> Result<String, CliError> {
    Ok(success_envelope(
        &new_data(&scaffold(&cwd()?, name, idempotency_key)?),
        &[],
    ))
}
