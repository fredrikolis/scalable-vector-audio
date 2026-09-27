// Concern: parses the readings and settings a render or an analysis is asked under | Non-concern: the subcommand dispatch (mod.rs), what a target names | IO: (argv tail) -> Command or CliError

use std::path::{Component, Path, PathBuf};

use sva_core::{Asked, CliError, Settings, is_wav, representation_for, retired, wav_path};
use sva_engine::{DEFAULT_SAMPLE_RATE, MAX_PINNED_FRAME, Representation, pinned_frame};

use super::{ANALYZE_REPRESENTATIONS, AnalyzeArgs, Command, RenderArgs, USAGE, value};

/// What `render` and `analyze` both read: readings and settings.
#[derive(Default)]
struct Flags {
    asked: Vec<(String, Option<PathBuf>)>,
    settings: Settings,
    confirm: bool,
}

impl Flags {
    /// `Ok(false)` means neither subcommand reads this flag here.
    fn read<'a>(
        &mut self,
        flag: &str,
        it: &mut impl Iterator<Item = &'a String>,
        keys: &[&str],
    ) -> Result<bool, CliError> {
        match flag {
            "--representation" => {
                for entry in value(it, "--representation")?.split(',') {
                    let (name, dest) = split_as(entry.trim())?;
                    refuse_audio_dest(&name, dest.as_deref())?;
                    self.asked.push((name, dest));
                }
            }
            "-c" => self.set(&value(it, "-c")?, keys)?,
            "--confirm" => self.confirm = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn set(&mut self, raw: &str, keys: &[&str]) -> Result<(), CliError> {
        let Some((key, raw)) = raw.split_once('=') else {
            return Err(CliError::Usage(format!(
                "`-c {raw}` names no value; write `-c <key>=<value>`\n{USAGE}"
            )));
        };
        self.settings
            .set(key, raw, keys)
            .map_err(|e| CliError::Usage(format!("-c {}\n{USAGE}", e.message())))
    }

    /// The asks `sva-analysis` answers, lifted out before a `Representation` is looked for:
    /// those four name no representation and never reach `sva-core`'s table.
    fn analyses(&self) -> Vec<(String, Option<PathBuf>)> {
        self.asked
            .iter()
            .filter(|(name, _)| sva_analysis::ANALYSES.contains(&name.as_str()))
            .cloned()
            .collect()
    }

    fn resolved(&self, allowed: Option<&[&str]>) -> Result<Vec<Asked>, CliError> {
        let mut out = Vec::with_capacity(self.asked.len());
        for (name, dest) in &self.asked {
            if sva_analysis::ANALYSES.contains(&name.as_str()) {
                if allowed.is_none() {
                    return Err(CliError::Usage(format!(
                        "`{name}` reads a rendered buffer back; write the render to a `.wav` \
                         and `analyze` it\n{USAGE}"
                    )));
                }
                continue;
            }
            if let Some(write) = retired(name) {
                return Err(CliError::Usage(format!(
                    "`{name}` left the language; write `{write}`\n{USAGE}"
                )));
            }
            let representation =
                representation_for(name, self.settings.shape).ok_or_else(|| {
                    CliError::Usage(format!("unknown representation `{name}`\n{USAGE}"))
                })?;
            if allowed.is_some_and(|set| !set.contains(&name.as_str())) {
                return Err(CliError::Usage(format!(
                    "`analyze` does not answer `{name}` — only {} need no rendered \
                     graph\n{USAGE}",
                    ANALYZE_REPRESENTATIONS.join(", ")
                )));
            }
            out.push(Asked {
                name: name.clone(),
                representation,
                dest: dest.clone(),
            });
        }
        Ok(out)
    }
}

const RENDER_KEYS: [&str; 10] = [
    "flop_budget",
    "proof_limit",
    "node",
    "depth",
    "peaks",
    "oversample",
    "frame",
    "brief",
    "skim",
    "pcm16",
];

const ANALYZE_KEYS: [&str; 3] = ["frame", "peaks", "against"];

/// A positional, which may open with a minus as an expression does; only a flag is none.
fn positional(arg: Option<&String>) -> Option<String> {
    arg.filter(|a| !a.starts_with("--") && *a != "-c").cloned()
}

/// The target is the one positional; everything else is a flag.
pub(super) fn render_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter();
    let target = positional(it.next()).ok_or_else(|| {
        CliError::Usage(format!(
            "render needs a target expression, as `sva-cli render '@master([0, inf))'`\n{USAGE}"
        ))
    })?;
    let mut flags = Flags::default();
    let (mut until, mut rate) = (None, None);
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it, &RENDER_KEYS)? {
            continue;
        }
        match flag.as_str() {
            "--until" => until = Some(value(&mut it, "--until")?),
            "--rate" => rate = Some(hertz(&value(&mut it, "--rate")?)?),
            other => {
                return Err(CliError::Usage(format!(
                    "unknown argument `{other}`\n{USAGE}"
                )));
            }
        }
    }
    let asked = flags.resolved(None)?;
    refuse_shared_destination(asked.iter().filter_map(|a| a.dest.as_deref()))?;
    if asked.is_empty() {
        return Err(CliError::Usage(format!(
            "render needs at least one `--representation <r>`; `--representation \
             samples=<path>.wav` writes audio\n{USAGE}"
        )));
    }
    if asked.iter().any(|a| a.name == "bindings") && flags.settings.node.is_none() {
        return Err(CliError::Usage(format!(
            "`bindings` needs `-c node=<path>`\n{USAGE}"
        )));
    }
    check_frame(&asked, rate.unwrap_or(DEFAULT_SAMPLE_RATE))?;
    Ok(Command::Render(Box::new(RenderArgs {
        target,
        until,
        rate,
        asked,
        settings: flags.settings,
        confirm: flags.confirm,
    })))
}

/// A file's rate is read off it, never chosen, and a file is read whole.
pub(super) fn analyze_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter();
    let path = positional(it.next())
        .ok_or_else(|| CliError::Usage(format!("missing <file.wav>\n{USAGE}")))?;
    let path = wav_path(&path)?;
    let mut flags = Flags::default();
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it, &ANALYZE_KEYS)? {
            continue;
        }
        return Err(CliError::Usage(format!(
            "unknown argument `{flag}`\n{USAGE}"
        )));
    }
    let asked = flags.resolved(Some(&ANALYZE_REPRESENTATIONS))?;
    let analyses = flags.analyses();
    refuse_shared_destination(
        asked
            .iter()
            .filter_map(|a| a.dest.as_deref())
            .chain(analyses.iter().filter_map(|(_, dest)| dest.as_deref())),
    )?;
    if asked.is_empty() && analyses.is_empty() {
        return Err(CliError::Usage(format!(
            "`analyze` needs at least one `--representation <r>`\n{USAGE}"
        )));
    }
    Ok(Command::Analyze(Box::new(AnalyzeArgs {
        path,
        asked,
        analyses,
        settings: flags.settings,
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
            return Err(CliError::Usage(format!(
                "more than one reading names `{}`; each would overwrite the last, so give \
                 every one its own path\n{USAGE}",
                dest.display()
            )));
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
        _ => Err(CliError::Usage(format!(
            "--rate needs a whole number of hertz above zero, got `{raw}`\n{USAGE}"
        ))),
    }
}

/// The destination may hold `=` itself, so only the first one separates.
fn split_as(raw: &str) -> Result<(String, Option<PathBuf>), CliError> {
    match raw.split_once('=') {
        Some((name, "")) => Err(CliError::Usage(format!(
            "`{name}=` names no destination; drop the `=` to read it under `data.readings`\n{USAGE}"
        ))),
        Some((name, path)) => Ok((name.to_string(), Some(PathBuf::from(path)))),
        None => Ok((raw.to_string(), None)),
    }
}

fn refuse_audio_dest(name: &str, dest: Option<&Path>) -> Result<(), CliError> {
    if dest.is_some_and(is_wav) && name != "samples" {
        return Err(CliError::Usage(format!(
            "a `.wav` destination carries audio, and `{name}` is not audio; write it to a path \
             with any other extension for JSON\n{USAGE}"
        )));
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
            return Err(CliError::Usage(format!(
                "-c frame={secs} sizes spectrum's transform to {frame} samples at {sr} Hz, past \
                 the {MAX_PINNED_FRAME}-sample bound\n{USAGE}"
            )));
        }
    }
    Ok(())
}
