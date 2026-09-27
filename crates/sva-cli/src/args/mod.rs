// Concern: parses argv into the subcommands this CLI answers and their options | Non-concern: what a target names (sva-core) | IO: (argv) -> Command or CliError

use std::path::PathBuf;

use sva_core::{Asked, CliError, Settings};

mod reading;

pub(crate) use reading::check_frame;
use reading::{analyze_args, render_args};

pub const USAGE: &str = "usage: sva-cli render '<expression>' [--until '<condition>'] \
     [--representation <r>[=<path>][,...]]... [--rate <hz>] [-c <key>=<value>]... [--confirm]\n       \
     sva-cli analyze <file.wav> [--representation <r>[=<path>][,...]]... [-c <key>=<value>]... \
     [--confirm]\n       \
     sva-cli lint [<node|expression>] [--format <json|text>]\n       \
     sva-cli trace <node|expression>\n       \
     sva-cli builtins\n       \
     sva-cli outline <expression>\n       \
     sva-cli new <name> [--idempotency-key <key>]\n\
     a render's target is one expression; `@path` reads a node in the current directory, \
     `@/abs/path` one anywhere, and its own ref may read an interval: `@piano([0, 2b], f0=C4)`\n\
     representations: lines atoms spectrum envelope derivative samples ledger pitch formants \
     stereo bands crest loudness alias bindings arguments flops\n\
     analyses (`analyze` only): onsets trajectory masking gain-reduction\n\
     -c keys: flop_budget proof_limit node depth peaks oversample frame brief skim pcm16, \
     and against for `analyze`\n\
     destinations: a `.wav` path takes `samples` as audio; any other path takes JSON; none \
     puts the reading under `data.readings`\n\
     verb aliases: `validate`=lint, `list`=builtins, `create`=new, `show`=trace. `render` \
     and `analyze` take a reading, which the standard's verb list has no word for, so they \
     keep their own names.";

/// Only the readings that are a pure function of a buffer; the rest need the graph behind it.
pub const ANALYZE_REPRESENTATIONS: [&str; 10] = [
    "samples",
    "spectrum",
    "envelope",
    "derivative",
    "pitch",
    "formants",
    "bands",
    "loudness",
    "crest",
    "stereo",
];

#[derive(Debug, PartialEq)]
pub struct RenderArgs {
    /// One expression; its refs name nodes from the current directory or an absolute path.
    pub target: String,
    pub until: Option<String>,
    pub rate: Option<u32>,
    pub asked: Vec<Asked>,
    pub settings: Settings,
    /// The caller said a destination that already holds a file may be replaced.
    pub confirm: bool,
}

#[derive(Debug, PartialEq)]
pub struct AnalyzeArgs {
    pub path: PathBuf,
    pub asked: Vec<Asked>,
    /// The readings `sva-analysis` answers, which no `Representation` names.
    pub analyses: Vec<(String, Option<PathBuf>)>,
    pub settings: Settings,
    /// The caller said a destination that already holds a file may be replaced.
    pub confirm: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Version,
    Help,
    Render(Box<RenderArgs>),
    Analyze(Box<AnalyzeArgs>),
    /// A node path or an expression, same grammar as render's target; `None` lints the whole
    /// directory.
    Lint {
        target: Option<String>,
        format: Format,
    },
    Trace {
        target: String,
    },
    Builtins,
    /// An expression's own parse tree; it reads no composition.
    Outline {
        text: String,
    },
    New {
        name: String,
        /// Present where the caller says this is a retry, per the `cli` standard's own
        /// requirement that a create verb be made idempotent.
        idempotency_key: Option<String>,
    },
}

/// How the diagnostics an evaluator answers are rendered. One set of objects, two renderings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Json,
    Text,
}

const STANDARD_VERBS: [(&str, &str); 4] = [
    ("validate", "lint"),
    ("list", "builtins"),
    ("create", "new"),
    ("show", "trace"),
];

fn canonical(subcommand: &str) -> &str {
    STANDARD_VERBS
        .iter()
        .find(|(alias, _)| *alias == subcommand)
        .map_or(subcommand, |(_, name)| *name)
}

/// A free function rather than a closure per parser: a closure would hold its loop's
/// iterator borrowed.
pub(crate) fn value<'a>(
    it: &mut impl Iterator<Item = &'a String>,
    name: &str,
) -> Result<String, CliError> {
    it.next()
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("{name} needs a value\n{USAGE}")))
}

/// `--version`/`-V` and `--help` are global flags, recognized anywhere in argv (cli standard).
pub fn parse_args(argv: &[String]) -> Result<Command, CliError> {
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        return Ok(Command::Version);
    }
    if argv.iter().any(|a| a == "--help") {
        return Ok(Command::Help);
    }

    let mut it = argv.iter();
    let subcommand = canonical(
        it.next()
            .ok_or_else(|| CliError::Usage(format!("no subcommand given\n{USAGE}")))?,
    );
    let rest = it.as_slice();
    match subcommand {
        "render" => render_args(rest),
        "analyze" => analyze_args(rest),
        "lint" => lint_args(rest),
        "trace" => trace_args(rest),
        "builtins" => builtins_args(rest),
        "outline" => outline_args(rest),
        "new" => new_args(rest),
        other => Err(CliError::Usage(format!(
            "unknown subcommand `{other}`\n{USAGE}"
        ))),
    }
}

fn builtins_args(rest: &[String]) -> Result<Command, CliError> {
    match rest.first() {
        None => Ok(Command::Builtins),
        Some(extra) => Err(CliError::Usage(format!(
            "builtins takes no arguments, not `{extra}`\n{USAGE}"
        ))),
    }
}

fn outline_args(rest: &[String]) -> Result<Command, CliError> {
    match rest {
        [text] => Ok(Command::Outline { text: text.clone() }),
        _ => Err(CliError::Usage(format!(
            "outline takes one <expression>, quoted as one argument\n{USAGE}"
        ))),
    }
}

fn new_args(rest: &[String]) -> Result<Command, CliError> {
    match rest {
        [name] if !name.starts_with("--") => Ok(Command::New {
            name: name.clone(),
            idempotency_key: None,
        }),
        [name, flag, key] if !name.starts_with("--") && flag == "--idempotency-key" => {
            Ok(Command::New {
                name: name.clone(),
                idempotency_key: Some(key.clone()),
            })
        }
        [] => Err(CliError::Usage(format!("missing <name>\n{USAGE}"))),
        _ => Err(CliError::Usage(format!(
            "new takes one <name> and an optional `--idempotency-key <key>`\n{USAGE}"
        ))),
    }
}

fn lint_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter().peekable();
    let target = match it.peek() {
        Some(a) if !a.starts_with("--") => it.next().cloned(),
        _ => None,
    };
    let mut format = Format::default();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--format" => {
                format = match it.next().map(String::as_str) {
                    Some("json") => Format::Json,
                    Some("text") => Format::Text,
                    other => {
                        return Err(CliError::Usage(format!(
                            "`--format` takes json or text, not `{}`\n{USAGE}",
                            other.unwrap_or("nothing")
                        )));
                    }
                };
            }
            extra => {
                return Err(CliError::Usage(format!(
                    "lint takes only [<node|expression>] and `--format <json|text>`, not \
                     `{extra}`\n{USAGE}"
                )));
            }
        }
    }
    Ok(Command::Lint { target, format })
}

/// One positional and nothing else: a trace answers structure, which no option narrows.
fn trace_args(rest: &[String]) -> Result<Command, CliError> {
    match rest {
        [target] if !target.starts_with("--") => Ok(Command::Trace {
            target: target.clone(),
        }),
        [] => Err(CliError::Usage(format!(
            "missing <node|expression>\n{USAGE}"
        ))),
        _ => Err(CliError::Usage(format!(
            "trace takes one <node|expression> and nothing else\n{USAGE}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    fn rendered(parts: &[&str]) -> RenderArgs {
        match parse_args(&argv(parts)) {
            Ok(Command::Render(args)) => *args,
            other => panic!("expected a render, got {other:?}"),
        }
    }

    fn refused(parts: &[&str]) -> String {
        match parse_args(&argv(parts)) {
            Err(CliError::Usage(message)) => message,
            other => panic!("expected a usage refusal, got {other:?}"),
        }
    }

    #[test]
    fn version_and_help_are_recognized_anywhere_in_argv() {
        assert_eq!(parse_args(&argv(&["--version"])).unwrap(), Command::Version);
        assert_eq!(parse_args(&argv(&["-V"])).unwrap(), Command::Version);
        assert_eq!(
            parse_args(&argv(&["lint", "--help"])).unwrap(),
            Command::Help
        );
    }

    #[test]
    fn each_standard_verb_alias_reaches_the_subcommand_it_names() {
        assert_eq!(
            parse_args(&argv(&["validate"])).unwrap(),
            Command::Lint {
                target: None,
                format: Format::Json,
            }
        );
        assert_eq!(parse_args(&argv(&["list"])).unwrap(), Command::Builtins);
        assert_eq!(
            parse_args(&argv(&["show", "kick"])).unwrap(),
            Command::Trace {
                target: "kick".to_string(),
            }
        );
        assert_eq!(
            parse_args(&argv(&["create", "song1"])).unwrap(),
            Command::New {
                name: "song1".to_string(),
                idempotency_key: None,
            }
        );
    }

    /// A render names its target, its readings as one comma list or several, its rate and
    /// its settings; nothing else.
    #[test]
    fn a_render_takes_a_target_readings_a_rate_and_settings() {
        let args = rendered(&[
            "render",
            "@piano([0, 2b], f0=C4)",
            "--representation",
            "samples=/tmp/out.wav,lines",
            "--representation",
            "ledger",
            "--rate",
            "48000",
            "-c",
            "depth=2",
            "-c",
            "pcm16=true",
            "--until",
            "t > 1s",
        ]);
        assert_eq!(args.target, "@piano([0, 2b], f0=C4)");
        assert_eq!(args.until.as_deref(), Some("t > 1s"));
        assert_eq!(args.rate, Some(48_000));
        let names: Vec<&str> = args.asked.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["samples", "lines", "ledger"]);
        assert_eq!(
            args.asked[0].dest.as_deref(),
            Some(Path::new("/tmp/out.wav"))
        );
        assert_eq!(args.settings.shape.depth, 2);
        assert!(args.settings.pcm16);
    }

    #[test]
    fn a_target_may_open_with_a_minus() {
        let args = rendered(&["render", "-1*sin(2*pi*220*t)", "--representation", "lines"]);
        assert_eq!(args.target, "-1*sin(2*pi*220*t)");
    }

    #[test]
    fn a_render_with_no_target_or_no_reading_refuses() {
        refused(&["render"]);
        refused(&["render", "--representation", "lines"]);
        refused(&["render", "@a"]);
    }

    /// Every flag the release before this one read is gone, with no alias left behind.
    #[test]
    fn a_retired_flag_refuses_by_name() {
        for gone in [
            "--in",
            "--from",
            "--to",
            "--max",
            "--sample-rate",
            "--flop-budget",
            "--as",
            "--node",
            "--frame",
            "--depth",
            "--peaks",
            "--oversample",
            "--brief",
            "--skim",
            "--pcm16",
        ] {
            let message = refused(&["render", "@a", "--representation", "lines", gone, "x"]);
            assert!(message.contains(gone), "{gone}: {message}");
        }
    }

    #[test]
    fn a_setting_no_key_names_or_a_malformed_one_refuses() {
        for setting in [
            "colour=red",
            "depth",
            "depth=many",
            "pcm16=yes",
            "against=/tmp/a.wav",
        ] {
            refused(&["render", "@a", "--representation", "lines", "-c", setting]);
        }
    }

    #[test]
    fn a_representation_that_left_the_language_names_what_replaced_it() {
        for (gone, write) in sva_core::RETIRED {
            let message = refused(&["render", "@a", "--representation", gone]);
            assert!(message.contains(write), "{message}");
        }
    }

    #[test]
    fn a_wav_destination_takes_samples_and_nothing_else() {
        refused(&["render", "@a", "--representation", "ledger=/tmp/out.wav"]);
    }

    #[test]
    fn bindings_needs_a_node() {
        refused(&["render", "@a", "--representation", "bindings"]);
        let args = rendered(&[
            "render",
            "@a",
            "--representation",
            "bindings",
            "-c",
            "node=kick",
        ]);
        assert_eq!(args.settings.node.as_deref(), Some("kick"));
    }

    #[test]
    fn lint_takes_an_optional_target_and_nothing_else() {
        assert_eq!(
            parse_args(&argv(&["lint", "drums/kick"])).unwrap(),
            Command::Lint {
                target: Some("drums/kick".to_string()),
                format: Format::Json
            }
        );
        refused(&["lint", "a", "b"]);
        refused(&["lint", "--in", "x"]);
    }

    #[test]
    fn new_takes_a_name_and_an_optional_idempotency_key() {
        assert_eq!(
            parse_args(&argv(&["new", "song1", "--idempotency-key", "k"])).unwrap(),
            Command::New {
                name: "song1".to_string(),
                idempotency_key: Some("k".to_string()),
            }
        );
        refused(&["new"]);
    }

    #[test]
    fn builtins_takes_no_arguments() {
        assert_eq!(parse_args(&argv(&["builtins"])).unwrap(), Command::Builtins);
        refused(&["builtins", "x"]);
    }

    #[test]
    fn a_subcommand_this_cli_does_not_answer_refuses_by_name() {
        for gone in ["bench", "nonlinear-solve"] {
            assert!(refused(&[gone]).contains(gone));
        }
    }
}
