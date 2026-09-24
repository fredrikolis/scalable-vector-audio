// Concern: resolves a graph into instances a caller can ask about, and keeps their buffers | Non-concern: parsing (sva-ast), CLI framing (sva-cli) | IO: (&Graph, root) -> Instances, Traced

mod arguments;
mod bindings;
mod cache;
mod cast;
mod error;
pub mod flops;
pub mod instantiate;
mod loops;
mod lower;
mod offset;
pub mod overload;
pub mod query;
mod refs;
pub mod render;
mod schedule;
mod trace;
mod typing;
mod vocabulary;

pub use arguments::{Argument, Arguments, Called, Chosen};
pub use bindings::Binding;
pub use cache::{
    Cache, CacheStats, DEFAULT_SLOT_BYTES, DiskCache, ENGINE_DIR_PREFIX, Entry, Expected, Hash,
    IO_NANOS_PER_BYTE, Lookup, Medium, MemoryCache, Outcome, PACK_FORMAT, Pack, Payload,
    PayloadKind, Put, RawF64, SampleCodec, Slots, Tier, Tiered, VecMedium, buffer_key,
    symbolic_key,
};
pub use cast::Cast;
pub use error::{BindingFault, Diagnostic, EngineError, Located, REGISTRY};
pub use flops::{Row as FlopRow, Tree as FlopTree};
pub use loops::{Delay, Shift};
pub use offset::Offset;
pub use query::{Answer, Ask, DEFAULT_FRAME_SECS, Output, Representation};
pub use refs::{Read, identity, nodes_in, resolve, spectral_sum_of, symbolic_hash};
pub use render::{
    Render, RenderConfig, answer, answer_buffer, ledger_over, render, render_with_slots,
    sketch_atom,
};
pub use schedule::{Order, Schedule, schedule_from};
pub use sva_formula::{C64, Codomain, Held, Line, NodeId, SpectralSum, Ty, Var};
pub use sva_samples::{
    Alias, AliasBand, BAND_COUNT, BandCrest, BandTrack, Bands, Buffer, Cost, Crest, Detail,
    EnvelopeFrame, FormantFrame, Frames, Horizon, Label, LedgerEntry, Loudness, LoudnessFrame,
    MAX_PINNED_FRAME, PSYCHOACOUSTIC_V1, PitchFrame, Profile, Rule, SignalKind, Source, Spectrum,
    StereoFrame, StereoImage, measure_alias, pinned_frame,
};
pub use trace::{Traced, Up, trace};
pub use typing::{Typing, Value};
pub use vocabulary::{BUILTINS, MAX_WIDTH, is_builtin, recognized_named};

use sva_ast::Graph;

pub const DEFAULT_SAMPLE_RATE: u32 = 44100;

pub const SOURCE_HASH: &str = env!("SVA_ENGINE_SRC_HASH");

/// Every crate whose source shapes a rendered sample, folded: equal only where a render is.
pub const RENDER_FINGERPRINT: u64 = sva_fingerprint::fold(&[
    sva_ast::SOURCE_HASH,
    sva_formula::SOURCE_HASH,
    sva_samples::SOURCE_HASH,
    SOURCE_HASH,
]);

/// One inference, so a lint and a render refuse identically.
pub fn check_structure(graph: &Graph, root: &str) -> Result<(), EngineError> {
    types(graph, root).map(|_| ())
}

/// One `Ty` per node, across refs, over the instances `root` reaches.
pub fn types(graph: &Graph, root: &str) -> Result<Typing, EngineError> {
    let instances = instantiate::instantiate(graph, root)?;
    // A root names the instance its own defaults resolved to, as `render` asks for.
    let held = instances.instance_of(root)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&held))?;
    typing::infer_all(&instances, &order)
}

/// Every instance whose samples are a function of the RATE, not just of `t`.
pub fn rate_dependent(graph: &Graph, root: &str) -> Result<Vec<String>, EngineError> {
    let instances = instantiate::instantiate(graph, root)?;
    Ok(instances
        .paths()
        .filter(|p| instances.at(p).is_some_and(|(e, _)| reads_rate(e)))
        .map(str::to_string)
        .collect())
}

fn reads_rate(e: &sva_ast::Expr) -> bool {
    match e {
        sva_ast::Expr::Lit(sva_ast::Literal::Samples(_)) => true,
        sva_ast::Expr::Lit(_) => false,
        sva_ast::Expr::Var(_) => false,
        sva_ast::Expr::Bin(_, a, b) => reads_rate(a) || reads_rate(b),
        sva_ast::Expr::SelfRef { .. } => true,
        sva_ast::Expr::Ref { arg, binds, .. } => {
            reads_rate(arg) || binds.iter().any(|(_, v)| reads_rate(v))
        }
        sva_ast::Expr::Call { name, args, .. } => {
            let drawn = name == "rand"
                && !matches!(
                    args.first(),
                    Some(sva_ast::Arg::Pos(sva_ast::Expr::Lit(_))) | None
                );
            drawn
                || args.iter().any(|a| match a {
                    sva_ast::Arg::Pos(v) | sva_ast::Arg::Named(_, v) => reads_rate(v),
                })
        }
    }
}
