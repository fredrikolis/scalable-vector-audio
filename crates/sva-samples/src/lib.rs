// Concern: the sampled representation and what produces or measures one | Non-concern: closed forms (sva-formula), nodes and refs (sva-engine) | IO: (ClosedForm, rate, extent) -> Buffer + Label

pub mod biquad;
pub mod buffer;
pub mod collapse;
pub mod error;
pub mod fft;
pub mod filters;
pub mod frames;
pub mod gain;
pub mod label;
pub mod machine;
pub mod measure;
pub mod physics;
pub mod profile;
pub mod reconstruct;
pub mod stft;

pub use biquad::Coeffs;
pub use buffer::Buffer;
pub use collapse::{
    ALIAS_OVERSAMPLE, AliasScore, Audible, Extent, Planned, Refs, Rows, crop_gain,
    eval_spectral_sum_at, eval_written_at, lane_of, of_spectral_sum, of_spectral_sum_or_point,
    planned, render, render_written, truncate_spectral_sum, truncate_written, unary,
};
pub use error::{CollapseError, SampleError};
pub use filters::{Automation, AutomationFrame, FilterSite, FilterTrace};
pub use frames::Frames;
pub use label::{Cost, Detail, Dropped, Label, Rule, Source};
pub use machine::renderer::{
    At, Binary, BufId, Map, NodeRenderer, Site, SiteId, Slot, Unary, Wrap,
};
pub use machine::tape::{Tape, Window};
pub use machine::{Ctx, Machine, MachineState, Standing};
pub use measure::Consumes;
pub use measure::alias::{
    AUDIBLE_NMR_DB, Alias, AliasBand, PLAYBACK_DB_SPL, measure_alias, worst as worst_alias,
};
pub use measure::bands::{BAND_COUNT, BandFloor, BandTrack, Bands, DECIMATED_HZ, cam, erb_hz};
pub use measure::crest::{BandCrest, Crest};
pub use measure::envelope::EnvelopeFrame;
pub use measure::formants::{Formant, FormantFrame, MAX_ORDER, default_order};
pub use measure::ledger::{LedgerEntry, SignalKind, Unit};
pub use measure::loudness::{Loudness, LoudnessFrame};
pub use measure::pitch::{Note, PitchFrame};
pub use measure::spectrum::{
    Band, MAX_PINNED_FRAME, Peak, Spectrum, db, magnitudes, pinned_frame, third_octave_edges,
};
pub use measure::stereo::{StereoFrame, StereoImage};
pub use physics::{Params, Solver, site};
pub use profile::{LATTICE_8K, PSYCHOACOUSTIC_V1, Profile};
pub use reconstruct::{Bound, Kernel, KernelFamily, kernel, plain, shortest};
pub use sva_formula::Shape;
