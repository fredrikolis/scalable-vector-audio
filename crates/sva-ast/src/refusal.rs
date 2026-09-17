// Concern: declares the directory-level located Refusal, wrapping a Diag with the file it landed in | Non-concern: raising one (graph.rs), the code registry (diag.rs) | IO: none

use crate::diag::{ByteSpan, DiagCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub span: ByteSpan,
}

/// `{ at, code, reason }`: never a panic, never a poison node — a hole in the graph
/// is data beside it, not inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub at: Location,
    pub code: DiagCode,
    pub reason: String,
}

impl Refusal {
    pub fn new(
        path: impl Into<String>,
        span: ByteSpan,
        code: DiagCode,
        reason: impl Into<String>,
    ) -> Refusal {
        Refusal {
            at: Location {
                path: path.into(),
                span,
            },
            code,
            reason: reason.into(),
        }
    }
}
