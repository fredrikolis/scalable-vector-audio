// Concern: writes one reading to the path it named | Non-concern: picking that path (args/), the WAV byte layout (wav.rs) | IO: (Answer, path) -> a written file or CliError

use std::path::Path;

use sva_core::{Answer, CliError, Horizon, Output, Report, is_wav, query_data};

use crate::wav::{SampleEncoding, write_channels};
use sva_core::success_envelope;

pub fn refuse_inside(dir: &Path, dest: &Path) -> Result<(), CliError> {
    if !under(dir, dest) {
        return Ok(());
    }
    Err(CliError::Usage(format!(
        "{} is inside the composition at {}, where it would be read as a node on the next \
         parse; write it outside the composition",
        dest.display(),
        dir.display()
    )))
}

/// A link at the destination is where the write lands; a path not there yet takes the
/// nearest ancestor that is, and a root that will not resolve proves nothing either way.
fn under(dir: &Path, dest: &Path) -> bool {
    let Ok(root) = dir.canonicalize() else {
        return true;
    };
    if let Ok(real) = dest.canonicalize() {
        return real.starts_with(&root);
    }
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => Path::new(".").to_path_buf(),
    };
    let Ok(mut at) = std::path::absolute(parent) else {
        return true;
    };
    loop {
        if let Ok(real) = at.canonicalize() {
            return real.starts_with(&root);
        }
        if !at.pop() {
            return true;
        }
    }
}

pub struct Framing {
    pub target: String,
    pub rate: u32,
    pub horizon: Horizon,
    pub profile: &'static str,
    pub encoding: SampleEncoding,
    pub skim: bool,
    pub replace: bool,
}

/// A written file does not come back, so `--confirm` is what replaces one.
pub(crate) fn refuse_replacing(dest: &Path, replace: bool) -> Result<(), CliError> {
    match !replace && dest.exists() {
        true => Err(CliError::Conflict {
            by: "destination",
            message: format!(
                "{} already holds a file; pass `--confirm` to replace it",
                dest.display()
            ),
        }),
        false => Ok(()),
    }
}

/// A reading no `Representation` names: its value is already JSON, so only the envelope.
pub fn write_analysis(
    name: &str,
    value: &str,
    dest: &Path,
    framing: &Framing,
) -> Result<(), CliError> {
    if is_wav(dest) {
        return Err(not_audio(name));
    }
    refuse_replacing(dest, framing.replace)?;
    let analyses = [(name.to_string(), value.to_string())];
    let json = success_envelope(
        &query_data(&Report {
            target: &framing.target,
            rate: framing.rate,
            horizon: framing.horizon,
            profile: framing.profile,
            label: None,
            written: &[],
            cache: None,
            answers: &[],
            analyses: &analyses,
            limit: None,
            skim: framing.skim,
        }),
        &[],
    );
    std::fs::write(dest, json + "\n")
        .map_err(|e| CliError::Io(format!("could not write {}: {e}", dest.display())))
}

/// Argv refuses this pair at parse time; a library caller passed none, so it is refused here.
fn not_audio(name: &str) -> CliError {
    CliError::Usage(format!(
        "`{name}` is not audio, so it has no samples to write to a `.wav` path"
    ))
}

/// A `.wav` path takes the samples; any other takes the JSON stdout caps.
pub fn write(name: &str, answer: &Answer, dest: &Path, framing: &Framing) -> Result<(), CliError> {
    refuse_replacing(dest, framing.replace)?;
    if is_wav(dest) {
        let Output::Samples(buffer) = &answer.value else {
            return Err(not_audio(name));
        };
        let held: Vec<Vec<f32>> = (0..buffer.width).map(|c| buffer.as_f32(c)).collect();
        let planes: Vec<&[f32]> = held.iter().map(Vec::as_slice).collect();
        return write_channels(&planes, buffer.rate, dest, framing.encoding);
    }
    let answers = [(name.to_string(), answer.clone())];
    let json = success_envelope(
        &query_data(&Report {
            target: &framing.target,
            rate: framing.rate,
            horizon: framing.horizon,
            profile: framing.profile,
            label: None,
            written: &[],
            cache: None,
            answers: &answers,
            analyses: &[],
            limit: None,
            skim: framing.skim,
        }),
        &[],
    );
    std::fs::write(dest, json + "\n")
        .map_err(|e| CliError::Io(format!("could not write {}: {e}", dest.display())))
}
