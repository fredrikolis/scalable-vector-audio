// Concern: parses argv into the subcommands this CLI answers and their options | Non-concern: what a target names (sva-core) | IO: (argv) -> Command or CliError

use std::path::PathBuf;

use sva_core::{Asked, CliError, WindowEdge};

mod reading;

pub(crate) use reading::check_frame;
use reading::{analyze_args, render_args};

pub const USAGE: &str = "usage: sva-cli render [<node|expression>] [query options]\n       \
     sva-cli analyze <file.wav> [--as <representation>[=<destination>]]... [--from <time>] \
     [--to <time>] [--frame <secs>] [--peaks <n>]\n       \
     sva-cli lint [<node|expression>] [--in <dir>] [--format <json|text>]\n       \
     sva-cli trace <node|expression> [--in <dir>]\n       \
     sva-cli builtins\n       \
     sva-cli outline <expression>\n       \
     sva-cli new <name> [--idempotency-key <key>]\n\
     query options: [--in <dir>] [--node <path>] [--from <time>] [--to <time>] \
     [--as <representation>[=<destination>]]... [--frame <secs>] [--depth <n>] [--peaks <n>] \
     [--sample-rate <hz>] [--oversample <n>] [--flop-budget <n>] \
     [--brief] [--skim] [--pcm16] [--confirm]\n\
     representations: lines atoms spectrum envelope derivative samples ledger pitch formants \
     stereo bands crest loudness alias bindings arguments flops\n\
     analyses (`analyze` only): onsets trajectory masking gain-reduction\n\
     --against <file.wav> is the second signal `--as masking` is read against\n\
     --sample-rate <hz> is the observation rate, legal with any --as\n\
     --node <path> names the instance a reading is taken of; required with `--as bindings`\n\
     --flop-budget <n> is the operation count the caller means to pay; the profile's own \
     budget refuses past it, and `--as flops` prints the tree that count came from\n\
     --brief condenses `ledger` to the nodes that clipped\n\
     --skim condenses `ledger`'s fields to node/channel/rms/peak/clipped\n\
     --pcm16 quantizes a `.wav` destination to 16-bit PCM instead of 32-bit float\n\
     destinations: a `.wav` path takes `samples` as audio; any other path takes JSON; none \
     prints JSON to stdout\n\
     --in <dir> names the composition `render`, `lint` and `trace` read; without it they read \
     the current directory. `new` and `analyze` take none. Each <node|expression> names a node \
     path or an expression in the same expression grammar a node file's body holds. A file \
     holds structure besides that body -- `name = <expr>` defaults, a TSV grid, a bare meter \
     such as `4/4` -- and an argument is a body alone, so `4/4` there is a division.\n\
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
    /// A node path or an expression; which one it is, only the graph can say.
    pub target: Option<String>,
    /// The instance a reading is taken of, where it is not the target itself.
    pub node: Option<String>,
    /// The composition directory `--in` named, where the caller named one.
    pub dir: Option<String>,
    pub sample_rate: Option<u32>,
    pub from: Option<WindowEdge>,
    pub to: Option<WindowEdge>,
    /// `--to silent`: the end is where silence is proven, not a time.
    pub silent: Option<sva_core::Silent>,
    pub asked: Vec<Asked>,
    pub brief: bool,
    pub skim: bool,
    pub pcm16: bool,
    /// The caller said a destination that already holds a file may be replaced.
    pub confirm: bool,
    pub flop_budget: Option<u128>,
}

#[derive(Debug, PartialEq)]
pub struct AnalyzeArgs {
    pub path: PathBuf,
    pub from: Option<WindowEdge>,
    pub to: Option<WindowEdge>,
    pub asked: Vec<Asked>,
    /// The readings `sva-analysis` answers, which no `Representation` names.
    pub analyses: Vec<(String, Option<PathBuf>)>,
    pub against: Option<PathBuf>,
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
        dir: Option<String>,
        format: Format,
    },
    Trace {
        target: String,
        dir: Option<String>,
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
    let (dir, rest) = composition_flag(rest)?;
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
                    "lint takes only [<node|expression>], `--in <dir>` and `--format \
                     <json|text>`, not `{extra}`\n{USAGE}"
                )));
            }
        }
    }
    Ok(Command::Lint {
        target,
        dir,
        format,
    })
}

/// `--in <dir>` names the composition, wherever in the tail it is written; without it, the
/// process's own directory is the one a subcommand reads.
pub(super) fn composition_flag(rest: &[String]) -> Result<(Option<String>, Vec<String>), CliError> {
    let mut dir = None;
    let mut kept = Vec::with_capacity(rest.len());
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--in" => {
                dir = Some(
                    it.next()
                        .cloned()
                        .ok_or_else(|| CliError::Usage(format!("`--in` needs a path\n{USAGE}")))?,
                );
            }
            _ => kept.push(arg.clone()),
        }
    }
    Ok((dir, kept))
}

/// One positional and nothing else: a trace answers structure, which no option narrows —
/// the rate is an observation parameter and a trace takes no observation.
fn trace_args(rest: &[String]) -> Result<Command, CliError> {
    let (dir, rest) = composition_flag(rest)?;
    match rest.as_slice() {
        [target] if !target.starts_with("--") => Ok(Command::Trace {
            target: target.clone(),
            dir,
        }),
        [] => Err(CliError::Usage(format!(
            "missing <node|expression>\n{USAGE}"
        ))),
        _ => Err(CliError::Usage(format!(
            "trace takes one <node|expression>, `--in <dir>` and nothing else\n{USAGE}"
        ))),
    }
}

pub(crate) fn number(raw: &str, flag: &str) -> Result<f64, CliError> {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(CliError::Usage(format!(
            "{flag} needs a finite number, got `{raw}`\n{USAGE}"
        ))),
    }
}

pub(crate) fn positive(raw: &str, flag: &str) -> Result<f64, CliError> {
    match number(raw, flag)? {
        v if v > 0.0 => Ok(v),
        _ => Err(CliError::Usage(format!(
            "{flag} needs a positive number, got `{raw}`\n{USAGE}"
        ))),
    }
}

pub(crate) fn operations(raw: &str, flag: &str) -> Result<u128, CliError> {
    raw.parse::<u128>().map_err(|_| not_a_count(raw, flag))
}

pub(crate) fn count(raw: &str, flag: &str) -> Result<usize, CliError> {
    usize::try_from(operations(raw, flag)?).map_err(|_| not_a_count(raw, flag))
}

fn not_a_count(raw: &str, flag: &str) -> CliError {
    CliError::Usage(format!("{flag} needs a whole count, got `{raw}`\n{USAGE}"))
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
                dir: None,
                format: Format::Json,
            }
        );
        assert_eq!(parse_args(&argv(&["list"])).unwrap(), Command::Builtins);
        assert_eq!(
            parse_args(&argv(&["show", "kick"])).unwrap(),
            Command::Trace {
                target: "kick".to_string(),
                dir: None,
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

    /// The rate is an observation parameter, so it is legal beside any `--as` and nowhere
    /// near a structural subcommand.
    #[test]
    fn the_sample_rate_is_an_observation_flag_render_takes_with_any_reading() {
        for name in ["lines", "atoms", "samples", "ledger"] {
            let args = rendered(&["render", "--as", name, "--sample-rate", "48000"]);
            assert_eq!(args.sample_rate, Some(48_000));
            assert_eq!(args.asked[0].name, name);
        }
        assert!(matches!(
            parse_args(&argv(&["trace", "kick", "--sample-rate", "48000"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn a_representation_that_left_the_language_names_what_replaced_it() {
        for (gone, write) in sva_core::RETIRED {
            let Err(CliError::Usage(message)) = parse_args(&argv(&["render", "--as", gone])) else {
                panic!("`{gone}` must refuse")
            };
            assert!(message.contains(write), "{message}");
        }
    }

    #[test]
    fn a_wav_destination_takes_samples_and_nothing_else() {
        let args = rendered(&["render", "--as", "samples=/tmp/out.wav", "--pcm16"]);
        assert!(args.pcm16);
        assert_eq!(
            args.asked[0].dest.as_deref(),
            Some(Path::new("/tmp/out.wav"))
        );
        assert!(matches!(
            parse_args(&argv(&["render", "--as", "ledger=/tmp/out.wav"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn render_needs_a_reading_and_bindings_needs_a_node() {
        assert!(matches!(
            parse_args(&argv(&["render"])),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            parse_args(&argv(&["render", "--as", "bindings"])),
            Err(CliError::Usage(_))
        ));
        let args = rendered(&["render", "--as", "bindings", "--node", "kick"]);
        assert_eq!(args.node.as_deref(), Some("kick"));
    }

    #[test]
    fn render_and_analyze_parse_the_same_window_flags() {
        let args = rendered(&["render", "--as", "samples", "--from", "0.5", "--to", "2"]);
        let window = (Some(WindowEdge::Secs(0.5)), Some(WindowEdge::Secs(2.0)));
        assert_eq!((args.from, args.to), window);
        let Ok(Command::Analyze(args)) = parse_args(&argv(&[
            "analyze",
            "/tmp/a.wav",
            "--as",
            "spectrum",
            "--from",
            "0.5",
            "--to",
            "2",
        ])) else {
            panic!("an analyze")
        };
        assert_eq!((args.from, args.to), window);
    }

    #[test]
    fn to_silent_takes_bits_and_a_latest_time_and_nothing_else_does() {
        let deep = rendered(&["render", "--as", "samples", "--to", "silent"]);
        let bits = sva_core::DEFAULT_SILENT_BITS;
        let max_secs = sva_core::DEFAULT_SILENT_MAX_SECS;
        assert_eq!(deep.silent, Some(sva_core::Silent { bits, max_secs }));
        assert_eq!(deep.to, None);
        let timed = rendered(&["render", "--as", "samples", "--to", "silent", "--to", "3"]);
        assert_eq!(
            (timed.to, timed.silent),
            (Some(WindowEdge::Secs(3.0)), None)
        );
        let later = rendered(&["render", "--as", "samples", "--to", "3", "--to", "silent"]);
        assert_eq!((later.to, later.silent.map(|s| s.bits)), (None, Some(bits)));
        let shallow = rendered(&[
            "render",
            "--as",
            "samples",
            "--to",
            "silent:16",
            "--max",
            "8",
        ]);
        let max_secs = 8.0;
        assert_eq!(
            shallow.silent,
            Some(sva_core::Silent { bits: 16, max_secs })
        );
        for refused in [
            &["render", "--as", "samples", "--max", "8"][..],
            &["render", "--as", "samples", "--to", "silent:0"],
            &["render", "--as", "samples", "--to", "silent:54"],
            &["render", "--as", "samples", "--to", "silently"],
            &[
                "analyze",
                "/tmp/a.wav",
                "--as",
                "spectrum",
                "--to",
                "silent",
            ],
        ] {
            assert!(
                matches!(parse_args(&argv(refused)), Err(CliError::Usage(_))),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn lint_takes_an_optional_target_and_nothing_else() {
        assert_eq!(
            parse_args(&argv(&["lint"])).unwrap(),
            Command::Lint {
                target: None,
                dir: None,
                format: Format::Json
            }
        );
        assert_eq!(
            parse_args(&argv(&["lint", "drums/kick"])).unwrap(),
            Command::Lint {
                target: Some("drums/kick".to_string()),
                dir: None,
                format: Format::Json
            }
        );
        assert!(matches!(
            parse_args(&argv(&["lint", "a", "b"])),
            Err(CliError::Usage(_))
        ));
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
        assert!(matches!(
            parse_args(&argv(&["new"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn builtins_takes_no_arguments() {
        assert_eq!(parse_args(&argv(&["builtins"])).unwrap(), Command::Builtins);
        assert!(matches!(
            parse_args(&argv(&["builtins", "x"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn a_subcommand_this_cli_does_not_answer_refuses_by_name() {
        for gone in ["bench", "nonlinear-solve"] {
            let Err(CliError::Usage(message)) = parse_args(&argv(&[gone])) else {
                panic!("`{gone}` must refuse")
            };
            assert!(message.contains(gone), "{message}");
        }
    }
}
