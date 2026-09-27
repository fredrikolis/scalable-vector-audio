// Concern: the observations both front ends ask for, by name and shape | Non-concern: argv (sva-cli's args.rs), taking the reading (sva-engine) | IO: (names, shaping) -> Representation

use std::path::{Path, PathBuf};

use sva_engine::{DEFAULT_FRAME_SECS, Representation};

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
