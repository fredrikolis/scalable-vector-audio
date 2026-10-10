// Concern: parses argv into the subcommands this CLI answers and their options | Non-concern: what a target names (sva-core) | IO: (argv) -> Command or CliError

use std::path::PathBuf;

use sva_core::{Asked, CliError};

mod reading;

pub(crate) use reading::check_frame;
use reading::{analyze_args, render_args};

#[derive(Debug, PartialEq)]
pub struct RenderArgs {
    /// One expression; its refs name nodes from the current directory or an absolute path.
    pub target: String,
    pub until: Option<String>,
    pub rate: Option<u32>,
    /// The precision every sample is written to; the profile's own where `None`.
    pub bits: Option<i32>,
    /// The most threads it computes on; every core where `None`.
    pub threads: Option<std::num::NonZeroUsize>,
    pub asked: Vec<Asked>,
    /// The caller said a destination that already holds a file may be replaced.
    pub confirm: bool,
    pub cache: CacheAt,
}

/// Where a render's persistent store lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CacheAt {
    #[default]
    Platform,
    Path(PathBuf),
    Off,
}

#[derive(Debug, PartialEq)]
pub struct AnalyzeArgs {
    pub path: PathBuf,
    pub asked: Vec<Asked>,
    /// The caller said a destination that already holds a file may be replaced.
    pub confirm: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Version,
    Help,
    Render(Box<RenderArgs>),
    Analyze(Box<AnalyzeArgs>),
    /// One expression, as render's target; `None` lints every file under its own rules.
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
        .ok_or_else(|| CliError::usage("cli.missing_value", format!("{name} needs a value")))
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
            .ok_or_else(|| CliError::usage("cli.missing_subcommand", "no subcommand given"))?,
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
        other => Err(CliError::usage(
            "cli.unknown_subcommand",
            format!("unknown subcommand `{other}`"),
        )),
    }
}

fn builtins_args(rest: &[String]) -> Result<Command, CliError> {
    match rest.first() {
        None => Ok(Command::Builtins),
        Some(extra) => Err(CliError::usage(
            "cli.unknown_argument",
            format!("builtins takes no arguments, not `{extra}`"),
        )),
    }
}

fn outline_args(rest: &[String]) -> Result<Command, CliError> {
    match rest {
        [text] => Ok(Command::Outline { text: text.clone() }),
        _ => Err(CliError::usage(
            "cli.missing_argument",
            "outline takes one <expression>, quoted as one argument",
        )),
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
        [] => Err(CliError::usage("cli.missing_argument", "missing <name>")),
        _ => Err(CliError::usage(
            "cli.unknown_argument",
            "new takes one <name> and an optional `--idempotency-key <key>`",
        )),
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
                        return Err(CliError::usage(
                            "cli.bad_value",
                            format!(
                                "`--format` takes json or text, not `{}`",
                                other.unwrap_or("nothing")
                            ),
                        ));
                    }
                };
            }
            extra => {
                return Err(CliError::usage(
                    "cli.unknown_argument",
                    format!(
                        "lint takes only ['<expression>'] and `--format <json|text>`, not \
                     `{extra}`"
                    ),
                ));
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
        [] => Err(CliError::usage(
            "cli.missing_argument",
            "missing <node|expression>",
        )),
        _ => Err(CliError::usage(
            "cli.unknown_argument",
            "trace takes one <node|expression> and nothing else",
        )),
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
            Err(CliError::Usage { message, .. }) => message,
            other => panic!("expected a usage refusal, got {other:?}"),
        }
    }

    /// Each mistake answers a code a caller branches on, its message quoting the text it
    /// refuses, and no banner: `--help` is where the whole surface is stated.
    #[test]
    fn each_argument_mistake_answers_its_own_code_naming_the_text() {
        let render = |tail: &[&'static str]| [&["render", "@a"][..], tail].concat();
        let cases: [(Vec<&str>, &str, &str); 15] = [
            (
                render(&["--representation", "spectre"]),
                "query.unknown_representation",
                "`spectre`",
            ),
            (
                render(&["--representation", "lines(depth=2)"]),
                "query.unknown_argument",
                "`depth`",
            ),
            (
                render(&["--representation", "ledger(depth=many)"]),
                "query.bad_value",
                "`many`",
            ),
            (
                render(&["--representation", "lines,lines"]),
                "query.duplicate",
                "`lines`",
            ),
            (
                render(&["--representation", "bindings"]),
                "query.missing_argument",
                "`bindings`",
            ),
            (
                render(&["--representation", "spectrum(peaks=8"]),
                "query.malformed",
                "`spectrum(peaks=8`",
            ),
            (
                render(&["--representation", "exact-envelope"]),
                "query.retired",
                "`exact-envelope`",
            ),
            (
                render(&["--representation", "a=x,b=x"]),
                "query.unknown_representation",
                "`a`",
            ),
            (
                render(&["--representation", "lines=x.json,atoms=./x.json"]),
                "query.shared_destination",
                "x.json",
            ),
            (
                render(&["--representation", "lines", "--bogus"]),
                "cli.unknown_argument",
                "`--bogus`",
            ),
            (
                render(&["--representation"]),
                "cli.missing_value",
                "--representation",
            ),
            (
                render(&["--representation", "lines", "--rate", "0"]),
                "cli.bad_value",
                "`0`",
            ),
            (vec!["bench"], "cli.unknown_subcommand", "`bench`"),
            (
                vec!["analyze", "/tmp/a.wav", "--representation", "ledger"],
                "query.not_analyzable",
                "`ledger`",
            ),
            (
                vec!["analyze", "/tmp/a.flac", "--representation", "bands"],
                "analyze.not_wav",
                "`/tmp/a.flac`",
            ),
        ];
        for (parts, code, text) in cases {
            let Err(CliError::Usage {
                code: held,
                message,
            }) = parse_args(&argv(&parts))
            else {
                panic!("{parts:?} refuses as an argument mistake");
            };
            assert_eq!(held, code, "{parts:?}: {message}");
            assert!(message.contains(text), "{parts:?}: {message}");
            assert!(!message.contains("usage:"), "{parts:?}: {message}");
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

    /// A render names its target, its readings as calls in one comma list or several, each
    /// reading's options as its own arguments, and its flags; nothing else.
    #[test]
    fn a_render_takes_a_target_readings_with_their_arguments_and_flags() {
        let args = rendered(&[
            "render",
            "@piano([0, 2b], f0=C4)",
            "--representation",
            "samples=/tmp/out.wav, spectrum(peaks=8, frame=50ms)",
            "--representation",
            "ledger(depth=2, brief=1)=/tmp/ledger.json,bindings(node=@voice)",
            "--rate",
            "48000",
            "--bits",
            "16",
            "--until",
            "t > 1s",
            "--threads",
            "3",
        ]);
        assert_eq!(args.target, "@piano([0, 2b], f0=C4)");
        assert_eq!(args.until.as_deref(), Some("t > 1s"));
        assert_eq!(args.rate, Some(48_000));
        assert_eq!(args.bits, Some(16));
        assert_eq!(args.threads.map(std::num::NonZeroUsize::get), Some(3));
        let names: Vec<&str> = args.asked.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["samples", "spectrum", "ledger", "bindings"]);
        assert_eq!(args.asked[0].path(), Some(Path::new("/tmp/out.wav")));
        assert_eq!(
            args.asked[1].representation,
            sva_engine::Representation::Spectrum {
                max_peaks: 8,
                frame_secs: Some(0.05)
            }
        );
        assert_eq!(
            args.asked[2].representation,
            sva_engine::Representation::Ledger { depth: 2 }
        );
        assert!(args.asked[2].brief);
        assert_eq!(args.asked[2].path(), Some(Path::new("/tmp/ledger.json")));
        assert_eq!(args.asked[3].node.as_deref(), Some("voice"));
    }

    #[test]
    fn a_render_on_no_threads_is_refused() {
        let message = refused(&[
            "render",
            "1",
            "--representation",
            "envelope",
            "--threads",
            "0",
        ]);
        assert!(message.contains("--threads"), "{message}");
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
            "-c",
            "--in",
            "--from",
            "--to",
            "--max",
            "--sample-rate",
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
    fn an_argument_its_reading_does_not_take_or_a_malformed_one_refuses() {
        for reading in [
            "lines(depth=2)",
            "ledger(colour=red)",
            "ledger(depth)",
            "ledger(depth=many)",
            "ledger(brief=yes)",
            "samples(bits=16)",
            "spectrum(peaks=8",
            "bindings(node=voice)",
        ] {
            refused(&["render", "@a", "--representation", reading]);
        }
    }

    /// The answer keys a reading by its name, so asking one twice would print that key twice.
    #[test]
    fn a_reading_asked_twice_refuses_by_name() {
        for written in [
            &["--representation", "spectrum(peaks=4),spectrum(frame=20ms)"][..],
            &["--representation", "lines", "--representation", "lines"][..],
            &["--representation", "samples=/tmp/a.wav,samples"][..],
        ] {
            let message = refused(&[&["render", "@a"][..], written].concat());
            assert!(message.contains("asked more than once"), "{message}");
        }
        let message = refused(&[
            "analyze",
            "/tmp/a.wav",
            "--representation",
            "pitch,pitch(peaks=2)",
        ]);
        assert!(message.contains("`pitch`"), "{message}");
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

    /// `alsa:` after `=` names a live device; its buffer is the reading's own argument, at
    /// the table's default until written, and the other flags and readings stand beside it.
    #[test]
    fn a_device_destination_takes_samples_and_a_buffer_beside_every_other_flag() {
        use sva_core::Destination;
        let row = sva_engine::Representation::Samples.reading();
        assert_eq!(row.takes, [sva_engine::Arg::Buffer]);
        assert_eq!(sva_engine::Arg::Buffer.key(), "buffer");
        let args = rendered(&["render", "@a", "--representation", "samples=alsa:default"]);
        assert_eq!(
            args.asked[0].dest,
            Some(Destination::Alsa {
                device: "default".to_string(),
                buffer_secs: sva_engine::DEFAULT_BUFFER_SECS,
            })
        );
        assert_eq!(args.asked[0].path(), None);
        let args = rendered(&[
            "render",
            "@a",
            "--representation",
            "samples(buffer=20ms)=alsa:hw:1,0,loudness",
            "--until",
            "t > 1s",
            "--rate",
            "44100",
        ]);
        let names: Vec<&str> = args.asked.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["samples", "loudness"]);
        assert_eq!(
            args.asked[0].dest,
            Some(Destination::Alsa {
                device: "hw:1,0".to_string(),
                buffer_secs: 0.02,
            }),
            "a comma inside a device name stays in it where no reading follows"
        );
        assert_eq!(args.asked[1].dest, None);
        assert_eq!(args.until.as_deref(), Some("t > 1s"));
        assert_eq!(args.rate, Some(44_100));
        let args = rendered(&[
            "render",
            "@a",
            "--representation",
            "lines,samples=alsa:plughw:CARD=PCH,DEV=0",
        ]);
        assert_eq!(
            args.asked[1].dest,
            Some(Destination::Alsa {
                device: "plughw:CARD=PCH,DEV=0".to_string(),
                buffer_secs: sva_engine::DEFAULT_BUFFER_SECS,
            })
        );
    }

    #[test]
    fn a_device_refuses_what_it_cannot_play_naming_the_text() {
        let render = |r: &'static str| vec!["render", "@a", "--representation", r];
        let cases: [(Vec<&str>, &str, &str); 9] = [
            (
                render("samples=alsa:hw:0,samples=alsa:hw:1"),
                "query.duplicate",
                "`samples`",
            ),
            (
                render("samples=alsa:"),
                "destination.no_device",
                "`samples=alsa:`",
            ),
            (
                render("loudness=alsa:default"),
                "destination.not_audio",
                "`loudness`",
            ),
            (
                render("samples(buffer=50ms)"),
                "query.buffer_without_device",
                "`samples`",
            ),
            (
                render("samples(buffer=50ms)=/tmp/a.wav"),
                "query.buffer_without_device",
                "`buffer`",
            ),
            (
                render("samples(buffer=0)=alsa:default"),
                "query.bad_value",
                "`0`",
            ),
            (
                render("samples(buffer=5000s)=alsa:default"),
                "query.bad_value",
                "`5000s`",
            ),
            (
                render("lines(buffer=1)"),
                "query.unknown_argument",
                "`buffer`",
            ),
            (
                vec![
                    "analyze",
                    "/tmp/a.wav",
                    "--representation",
                    "samples=alsa:hw:1",
                ],
                "destination.needs_render",
                "`alsa:hw:1`",
            ),
        ];
        for (parts, code, text) in cases {
            let Err(CliError::Usage {
                code: held,
                message,
            }) = parse_args(&argv(&parts))
            else {
                panic!("{parts:?} refuses as an argument mistake");
            };
            assert_eq!(held, code, "{parts:?}: {message}");
            assert!(message.contains(text), "{parts:?}: {message}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn off_linux_a_device_refuses_at_parse_as_an_unsupported_platform() {
        let parts = argv(&["render", "@a", "--representation", "samples=alsa:default"]);
        let Err(CliError::Usage { code, .. }) = parse_args(&parts) else {
            panic!("refused");
        };
        assert_eq!(code, "destination.unsupported_platform");
    }

    #[test]
    fn a_harmonic_reading_takes_its_floor_in_decibels_and_its_norm_by_name() {
        let args = rendered(&[
            "render",
            "@a",
            "--representation",
            "voice_leading(frame=10ms, floor=-60db, norm=l2),dissonance",
        ]);
        assert_eq!(
            args.asked[0].representation,
            sva_engine::Representation::Harmony {
                reading: sva_engine::Harmonic::VoiceLeading {
                    norm: sva_engine::harmony::voice_leading::Norm::L2
                },
                frame_secs: 0.01,
                floor_db: -60.0,
            }
        );
        assert_eq!(
            args.asked[1].representation,
            sva_engine::Representation::Harmony {
                reading: sva_engine::Harmonic::Dissonance {
                    max_pairs: sva_engine::DEFAULT_MAX_PAIRS
                },
                frame_secs: sva_engine::DEFAULT_FRAME_SECS,
                floor_db: sva_engine::DEFAULT_FLOOR_DB,
            }
        );
        let render = |r: &'static str| vec!["render", "@a", "--representation", r];
        for (parts, text) in [
            (render("dissonance(floor=-60)"), "`-60`"),
            (render("voice_leading(norm=l3)"), "`l3`"),
            (render("dissonance(norm=l1)"), "`norm`"),
        ] {
            let Err(CliError::Usage { message, .. }) = parse_args(&argv(&parts)) else {
                panic!("{parts:?} refuses as an argument mistake");
            };
            assert!(message.contains(text), "{parts:?}: {message}");
        }
    }

    #[test]
    fn bindings_needs_a_node() {
        refused(&["render", "@a", "--representation", "bindings"]);
    }

    #[test]
    fn lint_takes_an_optional_target_and_nothing_else() {
        assert_eq!(
            parse_args(&argv(&["lint", "@drums/kick"])).unwrap(),
            Command::Lint {
                target: Some("@drums/kick".to_string()),
                format: Format::Json,
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
