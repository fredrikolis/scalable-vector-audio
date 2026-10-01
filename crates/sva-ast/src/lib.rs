// Concern: the document model for a composition | Non-concern: rendering or DSP (sva-engine), CLI wiring (sva-cli) | IO: (a Source) -> Graph or Vec<Refusal>

mod diag;
mod dir;
mod doc_comment;
mod expr;
mod filename;
mod graph;
mod ingest;
mod lexer;
pub mod outline;
mod parameters;
mod parser;
mod print;
mod refusal;
mod skipped;
mod source;
mod tsv;
mod vocabulary;

pub use diag::{ByteSpan, Diag, DiagCode};
pub use dir::Dir;
pub use doc_comment::{DocComment, parse as parse_doc_comment};
pub use expr::{
    Address, Arg, BinOp, Binds, CEIL, Expr, FLOOR, INDEX, JOIN, Literal, SERIES, children,
    map_children,
};
pub use filename::{FileSpan, SpanUnit};
pub use graph::{
    Defined, Graph, Held, PerBar, VARIABLES, load, load_beside, load_reaching, names_a_node,
    reads_of, resolve_ref_path,
};
pub use ingest::{Parsed, occurs_free, parse_file};
pub use lexer::{
    LogUnit, Token, TokenKind, ref_spans, strip_line_comment, tokenize, whole_ref_path,
};
pub use outline::{Outline, outline};
pub use parameters::free_parameters;
pub use parser::parse as parse_expr;
pub use print::render as render_expr;
pub use refusal::{Location, Refusal};
pub use skipped::{Skip, Skipped};
pub use source::{Composition, Listing, Source};
pub use tsv::Grid;
pub use vocabulary::{
    BUILTINS, CASTS, CHANNEL, FILTERS, FINITE_DIFFERENCE, MODAL, RESERVED, SELF, is_builtin,
    is_language_value, is_reserved, note_midi,
};

use std::path::Path;

/// No numeric evaluation happens here; that is `sva-engine`'s job.
pub fn parse_composition(dir: &Path) -> Result<Graph, Vec<Refusal>> {
    load(&Dir::at(dir))
}
