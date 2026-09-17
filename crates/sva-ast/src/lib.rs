// Concern: the document model for a composition | Non-concern: rendering or DSP (sva-engine), CLI wiring (sva-cli) | IO: (a Source) -> Graph or Vec<Refusal>

mod diag;
mod dir;
mod expr;
mod filename;
mod graph;
mod ingest;
mod lexer;
mod parser;
mod print;
mod refusal;
mod skipped;
mod source;
mod tsv;

pub use diag::{ByteSpan, Diag, DiagCode};
pub use dir::Dir;
pub use expr::{Arg, BinOp, Binds, Expr, JOIN, Literal, children, map_children};
pub use filename::{FileSpan, SpanUnit};
pub use graph::{Graph, VARIABLES, load, load_reaching, names_a_node, reads_of, resolve_ref_path};
pub use ingest::mentions;
pub use lexer::ref_spans;
pub use parser::parse as parse_expr;
pub use print::render as render_expr;
pub use refusal::{Location, Refusal};
pub use skipped::{Skip, Skipped};
pub use source::{Composition, Listing, Source};
pub use tsv::Grid;

use std::path::Path;

/// One composition directory resolved into a [`Graph`] — the filesystem PRODUCER of what
/// [`load`] takes. No numeric evaluation happens here; that is `sva-engine`'s job.
pub fn parse_composition(dir: &Path) -> Result<Graph, Vec<Refusal>> {
    load(&Dir::at(dir))
}
