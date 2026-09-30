// Concern: resolves a graph into instances a caller can ask about, and keeps their buffers | Non-concern: parsing (sva-ast), CLI framing (sva-cli) | IO: (&Graph, root) -> Instances, Traced

mod arguments;
mod bindings;
mod cache;
mod cast;
mod error;
pub mod flops;
mod index;
pub mod instantiate;
mod loops;
mod lower;
mod meaning;
pub mod overload;
pub mod query;
mod refs;
pub mod render;
mod schedule;
mod source;
mod time;
mod trace;
mod typing;
mod vocabulary;

pub use arguments::{Argument, Arguments, Called, Chosen};
pub use bindings::Binding;
pub use cache::log::cache_log;
pub use cache::{
    Backend, Cache, CachePolicy, CacheStats, DEFAULT_CACHE_BYTES, DEFAULT_MARK_EVERY,
    DEFAULT_STORE_BYTES, Hash, INDEX_NAME, Lookup, NoStore, Outcome, PayloadKind, Persisted,
    PrunePolicy, STORE_FORMAT, Store, Stored, Through,
};
pub use cast::Cast;
pub use error::{BindingFault, Diagnostic, EngineError, Located, REGISTRY};
pub use flops::{Row as FlopRow, Tree as FlopTree, Work};
pub use meaning::{Meaning, meaning};
pub use query::{Answer, Ask, DEFAULT_FRAME_SECS, Output, Representation};
pub use refs::{identity, nodes_in, spectral_sum_of, symbolic_hash};
pub use render::until::{Cmp, Term};
pub use render::{
    Block, Change, Changed, Handle, NOTES, QUIET_AFTER_SECS, QUIET_LEVEL, QuietTail, Range, Render,
    RenderConfig, STREAMED, Stream, StreamConfig, Until, answer, answer_buffer, change, plan,
    quiet_tails, render, render_through, sketch_atom, warm,
};
pub use schedule::{Order, Schedule, schedule_from};
pub use sva_formula::{C64, Codomain, Held, Line, NodeId, SpectralSum, Ty, Var};
pub use sva_samples::{
    Alias, AliasBand, BAND_COUNT, BandCrest, BandTrack, Bands, Buffer, Cost, Crest, Detail,
    EnvelopeFrame, Extent, FormantFrame, Frames, Label, LedgerEntry, Loudness, LoudnessFrame,
    MAX_PINNED_FRAME, PSYCHOACOUSTIC_V1, PitchFrame, Profile, Rule, SignalKind, Source, Spectrum,
    StereoFrame, StereoImage, measure_alias, pinned_frame,
};
pub use trace::{Traced, Up, trace};
pub use typing::{Typing, Value, When};
pub use vocabulary::{BUILTINS, MAX_WIDTH, RESERVED, is_builtin, named_may_move, recognized_named};

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
