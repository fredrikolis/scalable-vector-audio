// Concern: the one table of observations: names, arguments, defaults, what each reads, help; the answer envelope | Non-concern: taking one reading (answer.rs) | IO: (name, Given) -> Representation

use std::mem::discriminant;

use sva_formula::{C64, Line, SpectralSum, Var};
use sva_samples::{
    Alias, Bands, Buffer, Consumes, Crest, EnvelopeFrame, FormantFrame, LedgerEntry, Loudness,
    Onsets, PitchFrame, Source, Spectrum, StereoImage,
};

use crate::arguments::Arguments;
use crate::bindings::Binding;

/// The default window a framed reading uses when the observation names none.
pub const DEFAULT_FRAME_SECS: f64 = 0.05;
pub const DEFAULT_MAX_PEAKS: usize = 16;
pub const DEFAULT_LEDGER_DEPTH: usize = 3;
pub const DEFAULT_OVERSAMPLE: u32 = 4;

/// Every named argument a reading may take; the engine reads only those `Given` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    Peaks,
    Frame,
    Depth,
    Oversample,
    Brief,
    Skim,
    Node,
}

impl Arg {
    pub fn key(self) -> &'static str {
        match self {
            Arg::Peaks => "peaks",
            Arg::Frame => "frame",
            Arg::Depth => "depth",
            Arg::Oversample => "oversample",
            Arg::Brief => "brief",
            Arg::Skim => "skim",
            Arg::Node => "node",
        }
    }
}

/// The arguments a `Representation` is built from, each at its default until written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Given {
    pub peaks: usize,
    /// `None` until written: `spectrum` then sizes its transform to the range, and every
    /// other framed reading steps by `DEFAULT_FRAME_SECS`.
    pub frame: Option<f64>,
    pub depth: usize,
    pub oversample: u32,
}

impl Default for Given {
    fn default() -> Given {
        Given {
            peaks: DEFAULT_MAX_PEAKS,
            frame: None,
            depth: DEFAULT_LEDGER_DEPTH,
            oversample: DEFAULT_OVERSAMPLE,
        }
    }
}

/// What a reading reads, which decides whether its answer is exact or measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    /// The written closed form alone: always exact.
    ClosedForm,
    /// Whichever the node holds: exact off a closed form, measured off samples.
    Either,
    /// Samples alone: always measured.
    Samples,
    /// Samples and the graph that rendered them, so no file answers it.
    Graph,
    /// The resolved instances, before anything renders; nothing composes for it.
    Structure,
}

impl Reads {
    pub fn consumes(self, closed: bool) -> Consumes {
        match self {
            Reads::ClosedForm | Reads::Structure => Consumes::ClosedForm,
            Reads::Either if closed => Consumes::ClosedForm,
            Reads::Either | Reads::Samples | Reads::Graph => Consumes::Buffer,
        }
    }

    pub fn off_samples(self, sampled: bool) -> bool {
        match self {
            Reads::Samples => true,
            Reads::Either => sampled,
            Reads::ClosedForm | Reads::Graph | Reads::Structure => false,
        }
    }
}

pub struct Reading {
    pub name: &'static str,
    pub takes: &'static [Arg],
    pub reads: Reads,
    pub help: &'static str,
    build: fn(&Given) -> Representation,
}

impl Reading {
    pub fn representation(&self, given: &Given) -> Representation {
        (self.build)(given)
    }
}

/// Every reading a caller may ask for, in the order a listing prints them.
pub const READINGS: [Reading; 17] = [
    Reading {
        name: "lines",
        takes: &[],
        reads: Reads::ClosedForm,
        help: "the exact line list of a closed form",
        build: |_| Representation::Lines,
    },
    Reading {
        name: "atoms",
        takes: &[],
        reads: Reads::ClosedForm,
        help: "each term of a closed form: its sketch, line, decay, polynomial and window",
        build: |_| Representation::Atoms,
    },
    Reading {
        name: "spectrum",
        takes: &[Arg::Peaks, Arg::Frame],
        reads: Reads::Either,
        help: "its lines, or its spectrum averaged over every frame",
        build: |g| Representation::Spectrum {
            max_peaks: g.peaks,
            frame_secs: g.frame,
        },
    },
    Reading {
        name: "envelope",
        takes: &[Arg::Frame],
        reads: Reads::Either,
        help: "rms and peak, per frame off samples",
        build: |g| Representation::Envelope {
            frame_secs: g.frame,
        },
    },
    Reading {
        name: "derivative",
        takes: &[],
        reads: Reads::Either,
        help: "the derivative, or the first difference of samples",
        build: |_| Representation::Derivative,
    },
    Reading {
        name: "samples",
        takes: &[],
        reads: Reads::Samples,
        help: "the samples themselves; a `.wav` destination writes them as audio",
        build: |_| Representation::Samples,
    },
    Reading {
        name: "ledger",
        takes: &[Arg::Depth, Arg::Brief, Arg::Skim],
        reads: Reads::Graph,
        help: "rms, peak, share and clipping per node under the target",
        build: |g| Representation::Ledger { depth: g.depth },
    },
    Reading {
        name: "pitch",
        takes: &[Arg::Peaks, Arg::Frame],
        reads: Reads::Either,
        help: "the notes it holds, per frame off samples",
        build: |g| Representation::Pitch {
            max_notes: g.peaks,
            frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        },
    },
    Reading {
        name: "formants",
        takes: &[Arg::Peaks, Arg::Frame],
        reads: Reads::Samples,
        help: "formant frequencies, bandwidths and levels per frame",
        build: |g| Representation::Formants {
            max_formants: g.peaks,
            frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        },
    },
    Reading {
        name: "stereo",
        takes: &[Arg::Frame],
        reads: Reads::Samples,
        help: "correlation, width and balance between two channels, per frame",
        build: |g| Representation::Stereo {
            frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        },
    },
    Reading {
        name: "bands",
        takes: &[],
        reads: Reads::Samples,
        help: "each auditory band's level over time",
        build: |_| Representation::Bands,
    },
    Reading {
        name: "crest",
        takes: &[],
        reads: Reads::Samples,
        help: "peak against rms, broadband and per band",
        build: |_| Representation::Crest,
    },
    Reading {
        name: "loudness",
        takes: &[],
        reads: Reads::Samples,
        help: "integrated, momentary and short-term loudness in LUFS",
        build: |_| Representation::Loudness,
    },
    Reading {
        name: "onsets",
        takes: &[],
        reads: Reads::Samples,
        help: "onset times, their strength and the spacing between them",
        build: |_| Representation::Onsets,
    },
    Reading {
        name: "alias",
        takes: &[Arg::Oversample],
        reads: Reads::Graph,
        help: "what folded, against a re-render at a multiple of the rate",
        build: |g| Representation::Alias {
            oversample: g.oversample,
        },
    },
    Reading {
        name: "bindings",
        takes: &[Arg::Node],
        reads: Reads::Structure,
        help: "the bindings one instance resolved, each with its source",
        build: |_| Representation::Bindings,
    },
    Reading {
        name: "arguments",
        takes: &[],
        reads: Reads::Structure,
        help: "each builtin call's named arguments as the numbers it was lowered with",
        build: |_| Representation::Arguments,
    },
];

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
    Onsets,
    Alias {
        oversample: u32,
    },
    Bindings,
    Arguments,
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
    Atoms(Vec<Atom>),
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
    Onsets(Box<Onsets>),
    Alias(Box<Alias>),
    Ledger(Vec<LedgerEntry>),
    Bindings(Vec<Binding>),
    Arguments(Vec<Arguments>),
}

/// `amp (x-polynomial_at)^degree e^(-decay (x-decay_from)) e^(2pi i turns x)` on `[start, end)`
#[derive(Clone, Debug, PartialEq)]
pub struct Atom {
    pub var: Var,
    pub sketch: String,
    pub amp: C64,
    pub turns: f64,
    pub decay: f64,
    pub decay_from: f64,
    pub degree: u16,
    pub polynomial_at: f64,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

/// Every answer says which reading ran and under which profile, so a caller never has to
/// infer either from the shape of the value.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub value: Output,
    pub source: Source,
    pub profile: &'static str,
    pub rate: Option<u32>,
    /// A series answers the terms it keeps, the tail beside them.
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
    pub fn reading(self) -> &'static Reading {
        let this = discriminant(&self);
        READINGS
            .iter()
            .find(|row| discriminant(&row.representation(&Given::default())) == this)
            .expect("every reading has its row in READINGS")
    }

    pub fn name(self) -> &'static str {
        self.reading().name
    }

    pub fn consumes(self, closed: bool) -> Consumes {
        self.reading().reads.consumes(closed)
    }

    /// Whether a node's samples alone answer this reading, `sampled` where it holds samples.
    pub fn off_samples(self, sampled: bool) -> bool {
        self.reading().reads.off_samples(sampled)
    }

    pub fn from_name(name: &str) -> Option<Representation> {
        READINGS
            .iter()
            .find(|row| row.name == name)
            .map(|row| row.representation(&Given::default()))
    }
}
