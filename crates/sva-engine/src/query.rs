// Concern: the one table of observations: names, arguments, defaults, what each reads, help; the answer envelope | Non-concern: taking one reading (answer.rs) | IO: (name, Given) -> Representation

use sva_formula::{C64, Line, SpectralSum, Var};
use sva_samples::measure::harmony::dissonance::Dissonance;
use sva_samples::measure::harmony::inharmonicity::Inharmonicity;
use sva_samples::measure::harmony::tonnetz::TonalDistance;
use sva_samples::measure::harmony::voice_leading::{Norm, VoiceLeading};
use sva_samples::measure::modulation::Modulation;
use sva_samples::measure::periodicity::Periodicity;
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
/// How much audio a live device holds ahead of what it plays, where `samples` names one.
pub const DEFAULT_BUFFER_SECS: f64 = 0.05;
/// The amplitude, in dB against full scale, under which a harmonic reading drops a partial.
pub const DEFAULT_FLOOR_DB: f64 = -40.0;
/// The most partial pairs one `dissonance` frame sums before it refuses.
pub const DEFAULT_MAX_PAIRS: usize = 1 << 17;
/// The span of onset-strength envelope one `periodicity` window autocorrelates.
pub const DEFAULT_PERIODICITY_WINDOW_SECS: f64 = 6.0;
/// The span of band envelope one `modulation` window transforms.
pub const DEFAULT_MODULATION_WINDOW_SECS: f64 = 4.0;

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
    Buffer,
    Floor,
    Norm,
    Pairs,
    Window,
    Curve,
    Bpm,
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
            Arg::Buffer => "buffer",
            Arg::Floor => "floor",
            Arg::Norm => "norm",
            Arg::Pairs => "pairs",
            Arg::Window => "window",
            Arg::Curve => "curve",
            Arg::Bpm => "bpm",
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
    pub floor_db: f64,
    pub norm: Norm,
    pub max_pairs: usize,
    /// `None` until written: each windowed reading then takes its own default.
    pub window: Option<f64>,
    pub curve: bool,
    pub bpm: Option<f64>,
}

impl Default for Given {
    fn default() -> Given {
        Given {
            peaks: DEFAULT_MAX_PEAKS,
            frame: None,
            depth: DEFAULT_LEDGER_DEPTH,
            oversample: DEFAULT_OVERSAMPLE,
            floor_db: DEFAULT_FLOOR_DB,
            norm: Norm::L1,
            max_pairs: DEFAULT_MAX_PAIRS,
            window: None,
            curve: false,
            bpm: None,
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

static LINES: Reading = Reading {
    name: "lines",
    takes: &[],
    reads: Reads::ClosedForm,
    help: "the exact line list of a closed form",
    build: |_| Representation::Lines,
};

static ATOMS: Reading = Reading {
    name: "atoms",
    takes: &[],
    reads: Reads::ClosedForm,
    help: "each term of a closed form: its sketch, line, decay, polynomial and window",
    build: |_| Representation::Atoms,
};

static SPECTRUM: Reading = Reading {
    name: "spectrum",
    takes: &[Arg::Peaks, Arg::Frame],
    reads: Reads::Either,
    help: "its lines, or its spectrum averaged over every frame",
    build: |g| Representation::Spectrum {
        max_peaks: g.peaks,
        frame_secs: g.frame,
    },
};

static ENVELOPE: Reading = Reading {
    name: "envelope",
    takes: &[Arg::Frame],
    reads: Reads::Either,
    help: "rms and peak, per frame off samples",
    build: |g| Representation::Envelope {
        frame_secs: g.frame,
    },
};

static DERIVATIVE: Reading = Reading {
    name: "derivative",
    takes: &[],
    reads: Reads::Either,
    help: "the derivative, or the first difference of samples",
    build: |_| Representation::Derivative,
};

static SAMPLES: Reading = Reading {
    name: "samples",
    takes: &[Arg::Buffer],
    reads: Reads::Samples,
    help: "the samples themselves; a `.wav` destination writes them as audio, an \
           `alsa:<device>` one plays them as they render",
    build: |_| Representation::Samples,
};

static LEDGER: Reading = Reading {
    name: "ledger",
    takes: &[Arg::Depth, Arg::Brief, Arg::Skim],
    reads: Reads::Graph,
    help: "rms, peak, share and clipping per node under the target",
    build: |g| Representation::Ledger { depth: g.depth },
};

static PITCH: Reading = Reading {
    name: "pitch",
    takes: &[Arg::Peaks, Arg::Frame],
    reads: Reads::Either,
    help: "the notes it holds, per frame off samples",
    build: |g| Representation::Pitch {
        max_notes: g.peaks,
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
    },
};

static FORMANTS: Reading = Reading {
    name: "formants",
    takes: &[Arg::Peaks, Arg::Frame],
    reads: Reads::Samples,
    help: "formant frequencies, bandwidths and levels per frame",
    build: |g| Representation::Formants {
        max_formants: g.peaks,
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
    },
};

static STEREO: Reading = Reading {
    name: "stereo",
    takes: &[Arg::Frame],
    reads: Reads::Samples,
    help: "correlation, width and balance between two channels, per frame",
    build: |g| Representation::Stereo {
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
    },
};

static BANDS: Reading = Reading {
    name: "bands",
    takes: &[],
    reads: Reads::Samples,
    help: "each auditory band's level over time",
    build: |_| Representation::Bands,
};

static CREST: Reading = Reading {
    name: "crest",
    takes: &[],
    reads: Reads::Samples,
    help: "peak against rms, broadband and per band",
    build: |_| Representation::Crest,
};

static LOUDNESS: Reading = Reading {
    name: "loudness",
    takes: &[],
    reads: Reads::Samples,
    help: "integrated, momentary and short-term loudness in LUFS",
    build: |_| Representation::Loudness,
};

static ONSETS: Reading = Reading {
    name: "onsets",
    takes: &[],
    reads: Reads::Samples,
    help: "onset times, their strength and the spacing between them",
    build: |_| Representation::Onsets,
};

static ALIAS: Reading = Reading {
    name: "alias",
    takes: &[Arg::Oversample],
    reads: Reads::Graph,
    help: "what folded, against a re-render at a multiple of the rate",
    build: |g| Representation::Alias {
        oversample: g.oversample,
    },
};

static DISSONANCE: Reading = Reading {
    name: "dissonance",
    takes: &[Arg::Frame, Arg::Floor, Arg::Pairs],
    reads: Reads::Either,
    help: "Sethares' sensory roughness per frame of its partials over a floor, with its mean, \
           max and min",
    build: |g| Representation::Harmony {
        reading: Harmonic::Dissonance {
            max_pairs: g.max_pairs,
        },
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        floor_db: g.floor_db,
    },
};

static VOICE_LEADING: Reading = Reading {
    name: "voice_leading",
    takes: &[Arg::Frame, Arg::Floor, Arg::Norm],
    reads: Reads::Either,
    help: "its chords over time, and the smallest voice leading between each two, l1 or l2 \
           in semitones",
    build: |g| Representation::Harmony {
        reading: Harmonic::VoiceLeading { norm: g.norm },
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        floor_db: g.floor_db,
    },
};

static TONAL_DISTANCE: Reading = Reading {
    name: "tonal_distance",
    takes: &[Arg::Frame, Arg::Floor],
    reads: Reads::Either,
    help: "its chords over time, the Tonnetz steps between each two triads and each one's \
           from the first",
    build: |g| Representation::Harmony {
        reading: Harmonic::TonalDistance,
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        floor_db: g.floor_db,
    },
};

static INHARMONICITY: Reading = Reading {
    name: "inharmonicity",
    takes: &[Arg::Frame, Arg::Floor],
    reads: Reads::Either,
    help: "its harmonic series per frame: each member's cents off n f1, the stiff-string B each \
           fits, and the energy in no series",
    build: |g| Representation::Harmony {
        reading: Harmonic::Inharmonicity,
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        floor_db: g.floor_db,
    },
};

static PERIODICITY: Reading = Reading {
    name: "periodicity",
    takes: &[Arg::Frame, Arg::Window, Arg::Curve],
    reads: Reads::Samples,
    help: "how strongly its onset strength repeats per sliding window, broadband, under 150 Hz \
           and over: the leading periods in seconds and bpm, their autocorrelation heights, \
           the drift between windows",
    build: |g| Representation::Periodicity {
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        window_secs: g.window.unwrap_or(DEFAULT_PERIODICITY_WINDOW_SECS),
        curve: g.curve,
    },
};

static MODULATION: Reading = Reading {
    name: "modulation",
    takes: &[Arg::Frame, Arg::Window, Arg::Bpm],
    reads: Reads::Samples,
    help: "how its band envelopes move per sliding window, under 150 Hz, to 2 kHz and over: \
           the leading modulation rates from 0.5 to 20 Hz, in beats where `bpm` is given, \
           their depth as a modulation index, the level, and the spectral centroid's mean \
           and its own swing in Hz",
    build: |g| Representation::Modulation {
        frame_secs: g.frame.unwrap_or(DEFAULT_FRAME_SECS),
        window_secs: g.window.unwrap_or(DEFAULT_MODULATION_WINDOW_SECS),
        bpm: g.bpm,
    },
};

static BINDINGS: Reading = Reading {
    name: "bindings",
    takes: &[Arg::Node],
    reads: Reads::Structure,
    help: "the bindings one instance resolved, each with its source",
    build: |_| Representation::Bindings,
};

static ARGUMENTS: Reading = Reading {
    name: "arguments",
    takes: &[],
    reads: Reads::Structure,
    help: "each builtin call's named arguments as the numbers it was lowered with",
    build: |_| Representation::Arguments,
};

/// Every reading a caller may ask for, in the order a listing prints them.
pub static READINGS: [&Reading; 23] = [
    &LINES,
    &ATOMS,
    &SPECTRUM,
    &ENVELOPE,
    &DERIVATIVE,
    &SAMPLES,
    &LEDGER,
    &PITCH,
    &FORMANTS,
    &STEREO,
    &BANDS,
    &CREST,
    &LOUDNESS,
    &ONSETS,
    &ALIAS,
    &DISSONANCE,
    &VOICE_LEADING,
    &TONAL_DISTANCE,
    &INHARMONICITY,
    &PERIODICITY,
    &MODULATION,
    &BINDINGS,
    &ARGUMENTS,
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
    /// The partials it is read from step by `frame_secs` and stop at `floor_db`.
    Harmony {
        reading: Harmonic,
        frame_secs: f64,
        floor_db: f64,
    },
    /// Windows of `window_secs` centred every `frame_secs`; `curve` adds each one's heights.
    Periodicity {
        frame_secs: f64,
        window_secs: f64,
        curve: bool,
    },
    /// Windows of `window_secs` centred every `frame_secs`; `bpm` adds each rate in beats.
    Modulation {
        frame_secs: f64,
        window_secs: f64,
        bpm: Option<f64>,
    },
    Bindings,
    Arguments,
}

/// The readings taken off one curve of partials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harmonic {
    Dissonance { max_pairs: usize },
    VoiceLeading { norm: Norm },
    TonalDistance,
    Inharmonicity,
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
    Dissonance(Box<Dissonance>),
    VoiceLeading(Box<VoiceLeading>),
    TonalDistance(Box<TonalDistance>),
    Inharmonicity(Box<Inharmonicity>),
    Periodicity(Box<Periodicity>),
    Modulation(Box<Modulation>),
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
    /// Exhaustive, so a variant with no row is a compile error.
    pub fn reading(self) -> &'static Reading {
        match self {
            Representation::Lines => &LINES,
            Representation::Atoms => &ATOMS,
            Representation::Spectrum { .. } => &SPECTRUM,
            Representation::Envelope { .. } => &ENVELOPE,
            Representation::Derivative => &DERIVATIVE,
            Representation::Samples => &SAMPLES,
            Representation::Ledger { .. } => &LEDGER,
            Representation::Pitch { .. } => &PITCH,
            Representation::Formants { .. } => &FORMANTS,
            Representation::Stereo { .. } => &STEREO,
            Representation::Bands => &BANDS,
            Representation::Crest => &CREST,
            Representation::Loudness => &LOUDNESS,
            Representation::Onsets => &ONSETS,
            Representation::Alias { .. } => &ALIAS,
            Representation::Harmony { reading, .. } => match reading {
                Harmonic::Dissonance { .. } => &DISSONANCE,
                Harmonic::VoiceLeading { .. } => &VOICE_LEADING,
                Harmonic::TonalDistance => &TONAL_DISTANCE,
                Harmonic::Inharmonicity => &INHARMONICITY,
            },
            Representation::Periodicity { .. } => &PERIODICITY,
            Representation::Modulation { .. } => &MODULATION,
            Representation::Bindings => &BINDINGS,
            Representation::Arguments => &ARGUMENTS,
        }
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
