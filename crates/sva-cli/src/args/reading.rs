// Concern: parses the readings, flags and destinations of a render or an analysis | Non-concern: the subcommand dispatch (mod.rs), what a target names | IO: (argv tail) -> Command or CliError

use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};

use sva_core::{Asked, Call, CliError, asked_all, calls, is_wav, wav_path};
use sva_engine::{DEFAULT_SAMPLE_RATE, MAX_PINNED_FRAME, READINGS, Representation, pinned_frame};

use super::{AnalyzeArgs, CacheAt, Command, RenderArgs, value};

/// What `render` and `analyze` both read: the calls asked for, and `--confirm`.
#[derive(Default)]
struct Flags {
    calls: Vec<Call>,
    confirm: bool,
}

impl Flags {
    /// `Ok(false)` means neither subcommand reads this flag here.
    fn read<'a>(
        &mut self,
        flag: &str,
        it: &mut impl Iterator<Item = &'a String>,
    ) -> Result<bool, CliError> {
        match flag {
            "--representation" => {
                for call in calls(&value(it, "--representation")?)? {
                    refuse_audio_dest(&call.name, call.dest.as_deref())?;
                    self.calls.push(call);
                }
            }
            "--confirm" => self.confirm = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `off_file` keeps to the readings a file's samples answer alone.
    fn resolved(&self, off_file: bool) -> Result<Vec<Asked>, CliError> {
        let out = asked_all(&self.calls)?;
        if let Some(one) = out
            .iter()
            .find(|one| off_file && !one.representation.off_samples(true))
        {
            let answered: Vec<&str> = READINGS
                .iter()
                .filter(|row| row.reads.off_samples(true))
                .map(|row| row.name)
                .collect();
            return Err(CliError::usage(
                "query.not_analyzable",
                format!(
                    "`analyze` does not answer `{}` — only {} need no rendered graph",
                    one.name,
                    answered.join(", ")
                ),
            ));
        }
        Ok(out)
    }
}

/// A positional, which may open with a minus as an expression does; only a flag is none.
fn positional(arg: Option<&String>) -> Option<String> {
    arg.filter(|a| !a.starts_with("--")).cloned()
}

/// The target is the one positional; everything else is a flag.
pub(super) fn render_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter();
    let target = positional(it.next()).ok_or_else(|| {
        CliError::usage(
            "cli.missing_argument",
            "render needs a target expression, as `sva-cli render '@song([0, inf))'`",
        )
    })?;
    let mut flags = Flags::default();
    let (mut until, mut rate, mut bits, mut threads) = (None, None, None, None);
    let mut cache = CacheAt::Platform;
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it)? {
            continue;
        }
        match flag.as_str() {
            "--until" => until = Some(value(&mut it, "--until")?),
            "--rate" => rate = Some(hertz(&value(&mut it, "--rate")?)?),
            "--bits" => bits = Some(whole(&value(&mut it, "--bits")?, "--bits")?),
            "--threads" => threads = Some(count(&value(&mut it, "--threads")?)?),
            "--cache" => {
                cache = match value(&mut it, "--cache")?.as_str() {
                    "none" => CacheAt::Off,
                    path => CacheAt::Path(PathBuf::from(path)),
                };
            }
            other => {
                return Err(CliError::usage(
                    "cli.unknown_argument",
                    format!("unknown argument `{other}`"),
                ));
            }
        }
    }
    let asked = flags.resolved(false)?;
    refuse_shared_destination(asked.iter().filter_map(|a| a.dest.as_deref()))?;
    if asked.is_empty() {
        return Err(CliError::usage(
            "cli.missing_argument",
            "render needs at least one `--representation <r>`; `--representation \
             samples=<path>.wav` writes audio",
        ));
    }
    check_frame(&asked, rate.unwrap_or(DEFAULT_SAMPLE_RATE))?;
    Ok(Command::Render(Box::new(RenderArgs {
        target,
        until,
        rate,
        bits,
        threads,
        asked,
        confirm: flags.confirm,
        cache,
    })))
}

/// A file's rate is read off it, never chosen, and a file is read whole.
pub(super) fn analyze_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter();
    let path = positional(it.next())
        .ok_or_else(|| CliError::usage("cli.missing_argument", "missing <file.wav>"))?;
    let path = wav_path(&path)?;
    let mut flags = Flags::default();
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it)? {
            continue;
        }
        return Err(CliError::usage(
            "cli.unknown_argument",
            format!("unknown argument `{flag}`"),
        ));
    }
    let asked = flags.resolved(true)?;
    refuse_shared_destination(asked.iter().filter_map(|a| a.dest.as_deref()))?;
    if asked.is_empty() {
        return Err(CliError::usage(
            "cli.missing_argument",
            "`analyze` needs at least one `--representation <r>`",
        ));
    }
    Ok(Command::Analyze(Box::new(AnalyzeArgs {
        path,
        asked,
        confirm: flags.confirm,
    })))
}

/// Two readings written to one path leave one of them on disk while the envelope lists
/// both as written. Refusing says which reading to move, before anything is rendered.
fn refuse_shared_destination<'a>(dests: impl Iterator<Item = &'a Path>) -> Result<(), CliError> {
    let mut taken: Vec<PathBuf> = Vec::new();
    for dest in dests {
        let held = lexical(dest);
        if taken.contains(&held) {
            return Err(CliError::usage(
                "query.shared_destination",
                format!(
                    "more than one reading names `{}`; each would overwrite the last, so give \
                 every one its own path",
                    dest.display()
                ),
            ));
        }
        taken.push(held);
    }
    Ok(())
}

/// `./out.json` and `out.json` are one file, so the comparison folds `.` and `..` away
/// first. Nothing here touches the filesystem: a destination need not exist yet, so a
/// symlink or two spellings of one directory still reach the write.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn hertz(raw: &str) -> Result<u32, CliError> {
    match raw.parse::<u32>() {
        Ok(hz) if hz > 0 => Ok(hz),
        _ => Err(CliError::usage(
            "cli.bad_value",
            format!("--rate needs a whole number of hertz above zero, got `{raw}`"),
        )),
    }
}

fn count(raw: &str) -> Result<NonZeroUsize, CliError> {
    raw.parse::<NonZeroUsize>().map_err(|_| {
        CliError::usage(
            "cli.bad_value",
            format!("--threads needs a whole number of threads above zero, got `{raw}`"),
        )
    })
}

pub(super) fn whole(raw: &str, flag: &str) -> Result<i32, CliError> {
    raw.parse::<i32>().map_err(|_| {
        CliError::usage(
            "cli.bad_value",
            format!("{flag} needs a whole number, got `{raw}`"),
        )
    })
}

fn refuse_audio_dest(name: &str, dest: Option<&Path>) -> Result<(), CliError> {
    if dest.is_some_and(is_wav) && name != "samples" {
        return Err(CliError::usage(
            "destination.not_audio",
            format!(
                "a `.wav` destination carries audio, and `{name}` is not audio; write it to a path \
             with any other extension for JSON"
            ),
        ));
    }
    Ok(())
}

/// A transform sized to fit is not the grid the caller asked two windows to share.
pub(crate) fn check_frame(asked: &[Asked], rate: u32) -> Result<(), CliError> {
    let sr = f64::from(rate);
    for one in asked {
        let Representation::Spectrum {
            frame_secs: Some(secs),
            ..
        } = one.representation
        else {
            continue;
        };
        let frame = pinned_frame(secs, sr);
        if frame > MAX_PINNED_FRAME {
            return Err(CliError::usage(
                "query.bad_value",
                format!(
                    "`spectrum(frame={secs})` sizes its transform to {frame} samples at {sr} Hz, \
                 past the {MAX_PINNED_FRAME}-sample bound"
                ),
            ));
        }
    }
    Ok(())
}
