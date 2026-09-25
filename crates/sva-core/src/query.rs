// Concern: the observation both front ends ask for, and the window it is asked over | Non-concern: argv (sva-cli's args.rs), taking the reading (sva-engine) | IO: (names, window) -> Query or CliError

use std::path::{Path, PathBuf};

use sva_engine::{DEFAULT_FRAME_SECS, Horizon, Representation};

use crate::cli_error::CliError;

pub const REPRESENTATIONS: [&str; 16] = [
    "lines",
    "atoms",
    "spectrum",
    "envelope",
    "derivative",
    "samples",
    "ledger",
    "pitch",
    "formants",
    "stereo",
    "bands",
    "crest",
    "loudness",
    "alias",
    "bindings",
    "arguments",
];

pub const RETIRED: [(&str, &str); 2] = [
    ("exact-envelope", "envelope, symbolic on a closed form"),
    ("exact-derivative", "derivative, symbolic on a closed form"),
];

/// No destination is stdout.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub name: String,
    pub representation: Representation,
    pub dest: Option<PathBuf>,
}

pub fn is_wav(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
}

pub const DEFAULT_OVERSAMPLE: u32 = 4;
pub const DEFAULT_MAX_PEAKS: usize = 16;
pub const DEFAULT_LEDGER_DEPTH: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shaping {
    pub frame_secs: Option<f64>,
    pub depth: usize,
    pub peaks: usize,
    pub oversample: u32,
}

impl Default for Shaping {
    fn default() -> Shaping {
        Shaping {
            frame_secs: None,
            depth: DEFAULT_LEDGER_DEPTH,
            peaks: DEFAULT_MAX_PEAKS,
            oversample: DEFAULT_OVERSAMPLE,
        }
    }
}

pub fn representation_for(name: &str, shape: Shaping) -> Option<Representation> {
    let frame = shape.frame_secs.unwrap_or(DEFAULT_FRAME_SECS);
    Some(match name {
        "spectrum" => Representation::Spectrum {
            max_peaks: shape.peaks,
            frame_secs: shape.frame_secs,
        },
        "envelope" => Representation::Envelope {
            frame_secs: shape.frame_secs,
        },
        "pitch" => Representation::Pitch {
            max_notes: shape.peaks,
            frame_secs: frame,
        },
        "formants" => Representation::Formants {
            max_formants: shape.peaks,
            frame_secs: frame,
        },
        "stereo" => Representation::Stereo { frame_secs: frame },
        "ledger" => Representation::Ledger { depth: shape.depth },
        "alias" => Representation::Alias {
            oversample: shape.oversample,
        },
        other => Representation::from_name(other)?,
    })
}

pub fn retired(name: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(gone, _)| *gone == name)
        .map(|(_, write)| *write)
}

/// What a window flag names, in the literals FORMAT 5.1 already spells: a bare number and
/// `Ns` are seconds, `Nb` is bars, and `end` is the node's own extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowEdge {
    Secs(f64),
    Bars(f64),
    End,
}

/// The bits `--to silent` measures silence at where none are written: the profile's own.
pub const DEFAULT_SILENT_BITS: u32 = sva_engine::PSYCHOACOUSTIC_V1.precision_bits as u32;

/// The latest instant `--to silent` looks for silence by, where `--max` names none.
pub const DEFAULT_SILENT_MAX_SECS: f64 = 60.0;

/// `silent` or `silent:<bits>`, the end a render proves rather than names; `None` for any
/// other end. A double holds 53 bits, so no finer silence is one it could show.
pub fn silent_edge(raw: &str) -> Option<Result<u32, CliError>> {
    if raw == "silent" {
        return Some(Ok(DEFAULT_SILENT_BITS));
    }
    let bits = raw.strip_prefix("silent:")?;
    Some(match bits.parse::<u32>() {
        Ok(bits) if (1..=53).contains(&bits) => Ok(bits),
        _ => Err(CliError::Usage(format!(
            "--to silent takes whole bits from 1 to 53, as `silent:16`, got `{raw}`"
        ))),
    })
}

pub fn window_edge(raw: &str, flag: &str) -> Result<WindowEdge, CliError> {
    let refuse = || {
        CliError::Usage(format!(
            "{flag} needs a time — a number of seconds, `<n>s`, `<n>b`, or `end`, got `{raw}`"
        ))
    };
    if raw == "end" {
        return Ok(WindowEdge::End);
    }
    let (number, wrap): (&str, fn(f64) -> WindowEdge) = match raw.strip_suffix('b') {
        Some(bars) => (bars, WindowEdge::Bars),
        None => (raw.strip_suffix('s').unwrap_or(raw), WindowEdge::Secs),
    };
    match number.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(wrap(v)),
        _ => Err(refuse()),
    }
}

fn edge(
    given: Option<WindowEdge>,
    flag: &str,
    unset: f64,
    seconds_per_bar: Option<f64>,
) -> Result<f64, CliError> {
    let secs = match given {
        None | Some(WindowEdge::End) => return Ok(unset),
        Some(WindowEdge::Secs(v)) => v,
        Some(WindowEdge::Bars(v)) => match seconds_per_bar {
            Some(secs) => v * secs,
            None => {
                return Err(CliError::BadTempo(format!(
                    "{flag} is written in bars and nothing here declares a tempo; \
                     state `variables/bpm` and `variables/meter`, or write seconds"
                )));
            }
        },
    };
    match secs.is_finite() && secs >= 0.0 {
        true => Ok(secs),
        false => Err(CliError::Usage(format!(
            "{flag} needs a time at or after zero, got `{secs}`"
        ))),
    }
}

/// An unstated end, and `end` itself, are infinite here: `config_for` settles both against
/// the node's own extent.
pub fn window_for(
    from: Option<WindowEdge>,
    to: Option<WindowEdge>,
    seconds_per_bar: Option<f64>,
) -> Result<Horizon, CliError> {
    Ok(Horizon::secs(
        edge(from, "--from", 0.0, seconds_per_bar)?,
        edge(to, "--to", f64::INFINITY, seconds_per_bar)?,
    ))
}
