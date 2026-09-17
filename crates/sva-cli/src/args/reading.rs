// Concern: parses the flags a reading is asked under, for render and analyze alike | Non-concern: the subcommand dispatch (mod.rs) | IO: (argv tail) -> Command or CliError

use std::path::{Component, Path, PathBuf};

use sva_core::{
    Asked, CliError, Shaping, WindowEdge, is_wav, representation_for, retired, window_edge,
};
use sva_engine::{DEFAULT_SAMPLE_RATE, MAX_PINNED_FRAME, Representation, pinned_frame};

use super::{
    ANALYZE_REPRESENTATIONS, AnalyzeArgs, Command, RenderArgs, USAGE, count, operations, positive,
    value,
};

struct Flags {
    from: Option<WindowEdge>,
    to: Option<WindowEdge>,
    shape: Shaping,
    asked: Vec<(String, Option<PathBuf>)>,
}

impl Flags {
    fn new() -> Flags {
        Flags {
            from: None,
            to: None,
            shape: Shaping::default(),
            asked: Vec::new(),
        }
    }

    /// The flags `render` and `analyze` both read. `Ok(false)` means neither did.
    fn read<'a>(
        &mut self,
        flag: &str,
        it: &mut impl Iterator<Item = &'a String>,
    ) -> Result<bool, CliError> {
        match flag {
            "--from" => self.from = Some(window_edge(&value(it, "--from")?, "--from")?),
            "--to" => self.to = Some(window_edge(&value(it, "--to")?, "--to")?),
            "--frame" => {
                self.shape.frame_secs = Some(positive(&value(it, "--frame")?, "--frame")?);
            }
            "--peaks" => self.shape.peaks = count(&value(it, "--peaks")?, "--peaks")?,
            "--depth" => self.shape.depth = count(&value(it, "--depth")?, "--depth")?,
            "--oversample" => self.shape.oversample = factor(&value(it, "--oversample")?)?,
            "--as" => {
                let (name, dest) = split_as(&value(it, "--as")?)?;
                refuse_audio_dest(&name, dest.as_deref())?;
                self.asked.push((name, dest));
            }
            _ => return Ok(false),
        }
        Ok(true)
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
            let representation = representation_for(name, self.shape).ok_or_else(|| {
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

/// A bare word is the target; anything starting `--` is a flag.
pub(super) fn render_args(rest: &[String]) -> Result<Command, CliError> {
    let (dir, rest) = super::composition_flag(rest)?;
    let mut peeked = rest.iter().peekable();
    let target = match peeked.peek() {
        Some(a) if !a.starts_with("--") => peeked.next().cloned(),
        _ => None,
    };
    let mut it = peeked;

    let mut flags = Flags::new();
    let (mut cache, mut brief, mut skim, mut pcm16) = (true, false, false, false);
    let mut confirm = false;
    let mut node: Option<String> = None;
    let mut sample_rate: Option<u32> = None;
    let mut flop_budget: Option<u128> = None;
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it)? {
            continue;
        }
        match flag.as_str() {
            "--no-cache" => cache = false,
            "--brief" => brief = true,
            "--skim" => skim = true,
            "--pcm16" => pcm16 = true,
            "--confirm" => confirm = true,
            "--node" => node = Some(value(&mut it, "--node")?),
            "--sample-rate" => sample_rate = Some(hertz(&value(&mut it, "--sample-rate")?)?),
            "--flop-budget" => {
                flop_budget = Some(operations(
                    &value(&mut it, "--flop-budget")?,
                    "--flop-budget",
                )?);
            }
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
            "render needs at least one `--as <representation>`; `--as samples=<path>.wav` \
             writes audio\n{USAGE}"
        )));
    }
    if asked.iter().any(|a| a.name == "bindings") && node.is_none() {
        return Err(CliError::Usage(format!(
            "`--as bindings` needs `--node <path>`\n{USAGE}"
        )));
    }
    check_frame(&asked, sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE))?;
    Ok(Command::Render(Box::new(RenderArgs {
        target,
        node,
        dir,
        cache,
        sample_rate,
        from: flags.from,
        to: flags.to,
        asked,
        brief,
        skim,
        pcm16,
        confirm,
        flop_budget,
    })))
}

/// A file's rate is read off it, never chosen, so `analyze` has no `--sample-rate` and no
/// cache: there is no graph to key one against.
pub(super) fn analyze_args(rest: &[String]) -> Result<Command, CliError> {
    let mut it = rest.iter();
    let path = it
        .next()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("missing <file.wav>\n{USAGE}")))?;
    let path = wav_path(&path)?;
    let mut flags = Flags::new();
    let mut against: Option<PathBuf> = None;
    let mut confirm = false;
    while let Some(flag) = it.next() {
        if flags.read(flag, &mut it)? {
            continue;
        }
        match flag.as_str() {
            "--confirm" => confirm = true,
            "--against" => against = Some(wav_path(&value(&mut it, "--against")?)?),
            other => {
                return Err(CliError::Usage(format!(
                    "unknown argument `{other}`\n{USAGE}"
                )));
            }
        }
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
            "`analyze` needs at least one `--as <representation>`\n{USAGE}"
        )));
    }
    Ok(Command::Analyze(Box::new(AnalyzeArgs {
        path,
        from: flags.from,
        to: flags.to,
        asked,
        analyses,
        against,
        confirm,
    })))
}

/// Two readings written to one path leave one of them on disk while the envelope lists
/// both as written. Refusing says which `--as` to move, before anything is rendered.
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

fn wav_path(raw: &str) -> Result<PathBuf, CliError> {
    match is_wav(Path::new(raw)) {
        true => Ok(PathBuf::from(raw)),
        false => Err(CliError::Usage(format!(
            "analyze takes a `.wav` file, not `{raw}` — mp3 and every other format are out \
             of scope\n{USAGE}"
        ))),
    }
}

/// The analysis frame is a power of two at the base rate and stays one at k times it.
fn factor(raw: &str) -> Result<u32, CliError> {
    match raw.parse::<u32>() {
        Ok(k) if (2..=16).contains(&k) && k.is_power_of_two() => Ok(k),
        _ => Err(CliError::Usage(format!(
            "--oversample needs a power of two from 2 to 16, got `{raw}`\n{USAGE}"
        ))),
    }
}

fn hertz(raw: &str) -> Result<u32, CliError> {
    match raw.parse::<u32>() {
        Ok(hz) if hz > 0 => Ok(hz),
        _ => Err(CliError::Usage(format!(
            "--sample-rate needs a whole number of hertz above zero, got `{raw}`\n{USAGE}"
        ))),
    }
}

/// The destination may hold `=` itself, so only the first one separates.
fn split_as(raw: &str) -> Result<(String, Option<PathBuf>), CliError> {
    match raw.split_once('=') {
        Some((name, "")) => Err(CliError::Usage(format!(
            "`--as {name}=` names no destination; drop the `=` to print to stdout\n{USAGE}"
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
                "--frame {secs} sizes spectrum's transform to {frame} samples at {sr} Hz, past \
                 the {MAX_PINNED_FRAME}-sample bound\n{USAGE}"
            )));
        }
    }
    Ok(())
}
