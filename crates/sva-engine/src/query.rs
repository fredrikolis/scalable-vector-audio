// Concern: names every observation, what each consumes, and the envelope an answer carries | Non-concern: taking one reading (answer.rs, sva-samples) | IO: (name) -> Representation, Consumes

use sva_formula::{Line, SpectralSum};
use sva_samples::{
    Alias, Bands, Buffer, Consumes, Crest, EnvelopeFrame, FormantFrame, LedgerEntry, Loudness,
    PitchFrame, Source, Spectrum, StereoImage,
};

use crate::arguments::Arguments;
use crate::bindings::Binding;

/// The default window a framed reading uses when the observation names none.
pub const DEFAULT_FRAME_SECS: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Representation {
    Lines,
    Atoms,
    Spectrum {
        max_peaks: usize,
        frame_secs: Option<f64>,
    },
    Envelope {
        frame_secs: Option<f64>,
    },
    Derivative,
    Samples,
    Ledger {
        depth: usize,
    },
    Pitch {
        max_notes: usize,
        frame_secs: f64,
    },
    Formants {
        max_formants: usize,
        frame_secs: f64,
    },
    Stereo {
        frame_secs: f64,
    },
    Bands,
    Crest,
    Loudness,
    Alias {
        oversample: u32,
    },
    Bindings,
    Arguments,
    Flops,
}

/// What a caller asked of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub node: String,
    pub representation: Representation,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Lines(Vec<Line>),
    Atoms(Vec<String>),
    Symbolic(Box<SpectralSum>),
    Samples(Box<Buffer>),
    Spectrum(Box<Spectrum>),
    Envelope(Vec<EnvelopeFrame>),
    Pitch(Vec<PitchFrame>),
    Formants(Vec<FormantFrame>),
    Stereo(Box<StereoImage>),
    Bands(Box<Bands>),
    Crest(Box<Crest>),
    Loudness(Box<Loudness>),
    Alias(Box<Alias>),
    Ledger(Vec<LedgerEntry>),
    Bindings(Vec<Binding>),
    Arguments(Vec<Arguments>),
    Flops(Box<crate::flops::Tree>),
}

/// Every answer says which reading ran and under which profile, so a caller never has to
/// infer either from the shape of the value.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub value: Output,
    pub source: Source,
    pub profile: &'static str,
    pub rate: Option<u32>,
    /// FORMAT 14.1: a series answers the terms it keeps, the tail beside them.
    pub dropped: Vec<sva_formula::Line>,
    pub tail_db: Option<f64>,
}

impl Answer {
    pub fn whole(
        value: Output,
        source: Source,
        profile: &'static str,
        rate: Option<u32>,
    ) -> Answer {
        Answer {
            value,
            source,
            profile,
            rate,
            dropped: Vec::new(),
            tail_db: None,
        }
    }
}

impl Representation {
    pub fn name(self) -> &'static str {
        match self {
            Representation::Lines => "lines",
            Representation::Atoms => "atoms",
            Representation::Spectrum { .. } => "spectrum",
            Representation::Envelope { .. } => "envelope",
            Representation::Derivative => "derivative",
            Representation::Samples => "samples",
            Representation::Ledger { .. } => "ledger",
            Representation::Pitch { .. } => "pitch",
            Representation::Formants { .. } => "formants",
            Representation::Stereo { .. } => "stereo",
            Representation::Bands => "bands",
            Representation::Crest => "crest",
            Representation::Loudness => "loudness",
            Representation::Alias { .. } => "alias",
            Representation::Bindings => "bindings",
            Representation::Arguments => "arguments",
            Representation::Flops => "flops",
        }
    }

    /// `lines` and `atoms` read a closed form and nothing else. `spectrum`, `envelope`,
    /// `derivative` and `pitch` read whichever the node holds, so its own `Ty` decides.
    pub fn consumes(self, closed: bool) -> Consumes {
        match self {
            Representation::Lines | Representation::Atoms => Consumes::ClosedForm,
            Representation::Spectrum { .. }
            | Representation::Envelope { .. }
            | Representation::Derivative
            | Representation::Pitch { .. }
                if closed =>
            {
                Consumes::ClosedForm
            }
            // None reads a buffer: two are resolved first, the other counts the schedule.
            Representation::Bindings | Representation::Arguments | Representation::Flops => {
                Consumes::ClosedForm
            }
            _ => Consumes::Buffer,
        }
    }

    pub fn from_name(name: &str) -> Option<Representation> {
        Some(match name {
            "lines" => Representation::Lines,
            "atoms" => Representation::Atoms,
            "spectrum" => Representation::Spectrum {
                max_peaks: 16,
                frame_secs: None,
            },
            "envelope" => Representation::Envelope { frame_secs: None },
            "derivative" => Representation::Derivative,
            "samples" => Representation::Samples,
            "ledger" => Representation::Ledger { depth: 3 },
            "pitch" => Representation::Pitch {
                max_notes: 6,
                frame_secs: DEFAULT_FRAME_SECS,
            },
            "formants" => Representation::Formants {
                max_formants: 5,
                frame_secs: DEFAULT_FRAME_SECS,
            },
            "stereo" => Representation::Stereo {
                frame_secs: DEFAULT_FRAME_SECS,
            },
            "bands" => Representation::Bands,
            "crest" => Representation::Crest,
            "loudness" => Representation::Loudness,
            "alias" => Representation::Alias { oversample: 4 },
            "bindings" => Representation::Bindings,
            "arguments" => Representation::Arguments,
            "flops" => Representation::Flops,
            _ => return None,
        })
    }
}
