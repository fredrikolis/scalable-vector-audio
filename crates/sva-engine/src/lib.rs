// Concern: resolves a graph into instances a caller can ask about, and keeps their buffers | Non-concern: parsing (sva-ast), CLI framing (sva-cli) | IO: (&Graph, root) -> Instances, Traced

mod arguments;
mod bindings;
mod cache;
mod cast;
mod error;
mod index;
pub mod instantiate;
mod loops;
mod lower;
mod meaning;
pub mod overload;
pub mod query;
mod recent;
mod refs;
pub mod render;
mod schedule;
mod steps;
mod threads;
mod time;
mod trace;
mod typing;
mod vocabulary;

pub use arguments::{Argument, Arguments, Called, Chosen};
pub use bindings::Binding;
pub use cache::log::cache_log;
pub use cache::{
    Backend, CacheStats, Counters, DEFAULT_CACHE_BYTES, DEFAULT_MARK_EVERY, DEFAULT_STORE_BYTES,
    FETCH_READS, Hash, INDEX_NAME, Lookup, Nothing, Outcome, PayloadKind, Persisted, STORE_FORMAT,
    Store, Stored, Tier,
};
pub use cast::Cast;
pub use error::{BindingFault, Diagnostic, EngineError, Located, REGISTRY};
pub use meaning::{Meaning, meaning};
pub use query::{Answer, Ask, DEFAULT_FRAME_SECS, Output, Representation};
pub use refs::{identity, nodes_in, spectral_sum_of};
pub use render::until::{Cmp, Term};
pub use render::{
    Abandon, Built, Change, Changed, Counts, Handle, LATEST, NOTES, Never, Out, Placed, Range,
    Render, RenderConfig, STREAMED, Session, Stream, StreamConfig, Until, Work, answer,
    answer_buffer, change, ends, fetch, plan, render, render_in, render_over, sketch_atom,
};
pub use schedule::{Order, Schedule, schedule_from};
pub use sva_formula::{C64, Codomain, Held, Line, NodeId, SpectralSum, Ty, Var};
pub use sva_samples::{
    Alias, AliasBand, BAND_COUNT, BandCrest, BandTrack, Bands, Buffer, Crest,
    CuttingBelowSilenceThreshold, Detail, EnvelopeFrame, Extent, FormantFrame, Frames, Label,
    LedgerEntry, Loudness, LoudnessFrame, MAX_PINNED_FRAME, Onsets, PSYCHOACOUSTIC_V1, PitchFrame,
    Profile, Rule, SignalKind, Source, Spectrum, StereoFrame, StereoImage, measure_alias,
    pinned_frame,
};
pub use threads::default_threads;
pub use trace::{Traced, Up, trace};
pub use typing::{Typing, Value, When};
pub use vocabulary::{MAX_WIDTH, named_may_move, recognized_named, shape, shape_name};

use sva_ast::Graph;

pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// One inference, so a lint and a render refuse identically.
pub fn check_structure(graph: &Graph, root: &str) -> Result<(), EngineError> {
    types(graph, root).map(|_| ())
}

/// One `Ty` per node, across refs, over the instances `root` reaches.
pub fn types(graph: &Graph, root: &str) -> Result<Typing, EngineError> {
    types_at(graph, root, DEFAULT_SAMPLE_RATE)
}

/// At `rate`, whose step each `sp` is.
pub fn types_at(graph: &Graph, root: &str, rate: u32) -> Result<Typing, EngineError> {
    let instances = instantiate::instantiate(graph, root, rate)?;
    // A root names the instance its own defaults resolved to, as `render` asks for.
    let held = instances.instance_of(root)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&held))?;
    typing::infer_all(&instances, &order)
}
