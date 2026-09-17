// Concern: declares the parser-level refusal code registry, ByteSpan and Diag | Non-concern: directory-level Refusal wrapping (refusal.rs), raising one | IO: (DiagCode, ByteSpan, message) -> Diag

use std::fmt;

/// A half-open byte range into one file's content; always lands on char boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

impl ByteSpan {
    pub const fn new(start: usize, end: usize) -> ByteSpan {
        ByteSpan { start, end }
    }

    pub const fn at(offset: usize) -> ByteSpan {
        ByteSpan {
            start: offset,
            end: offset,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagCode {
    UnexpectedChar,
    InvalidNumber,
    UnexpectedToken,
    UnexpectedEof,
    UnclosedParen,
    UnbalancedParen,
    EmptyExpression,
    RecursionLimit,
    BadArity,
    TsvMissingSpan,
    DanglingRef,
    RefAboveRoot,
    RefCycle,
    Io,
    BadArrangementArg,
    BadDefault,
    BareTime,
    NotNodeContent,
}

impl DiagCode {
    #[cfg(test)]
    const ALL: &'static [DiagCode] = &[
        DiagCode::UnexpectedChar,
        DiagCode::InvalidNumber,
        DiagCode::UnexpectedToken,
        DiagCode::UnexpectedEof,
        DiagCode::UnclosedParen,
        DiagCode::UnbalancedParen,
        DiagCode::EmptyExpression,
        DiagCode::RecursionLimit,
        DiagCode::BadArity,
        DiagCode::TsvMissingSpan,
        DiagCode::DanglingRef,
        DiagCode::RefAboveRoot,
        DiagCode::RefCycle,
        DiagCode::Io,
        DiagCode::BadArrangementArg,
        DiagCode::BadDefault,
        DiagCode::BareTime,
        DiagCode::NotNodeContent,
    ];

    pub fn code_str(self) -> &'static str {
        match self {
            DiagCode::UnexpectedChar => "unexpected-char",
            DiagCode::InvalidNumber => "invalid-number",
            DiagCode::UnexpectedToken => "unexpected-token",
            DiagCode::UnexpectedEof => "unexpected-eof",
            DiagCode::UnclosedParen => "unclosed-paren",
            DiagCode::UnbalancedParen => "unbalanced-paren",
            DiagCode::EmptyExpression => "empty-expression",
            DiagCode::RecursionLimit => "recursion-limit",
            DiagCode::BadArity => "bad-arity",
            DiagCode::TsvMissingSpan => "tsv-missing-span",
            DiagCode::DanglingRef => "dangling-ref",
            DiagCode::RefAboveRoot => "ref-above-root",
            DiagCode::RefCycle => "ref-cycle",
            DiagCode::Io => "io",
            DiagCode::BadArrangementArg => "bad-arrangement-arg",
            DiagCode::BadDefault => "bad-default",
            DiagCode::BareTime => "bare-time",
            DiagCode::NotNodeContent => "not-node-content",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub code: DiagCode,
    pub span: ByteSpan,
    pub message: String,
}

impl Diag {
    pub fn new(code: DiagCode, span: ByteSpan, message: impl Into<String>) -> Diag {
        Diag {
            code,
            span,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "error[{}]: {} (at bytes {}..{})",
            self.code.code_str(),
            self.message,
            self.span.start,
            self.span.end,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_codes_are_unique_kebab_case() {
        let mut codes: Vec<&str> = DiagCode::ALL.iter().map(|c| c.code_str()).collect();
        let before = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(before, codes.len(), "code strings must be unique");
        for c in DiagCode::ALL {
            let s = c.code_str();
            assert!(
                s.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-'),
                "kebab-case only: {s:?}"
            );
        }
    }

    #[test]
    fn display_is_located_and_ascii() {
        let d = Diag::new(DiagCode::UnexpectedEof, ByteSpan::new(1, 4), "oops");
        let s = d.to_string();
        assert!(s.is_ascii());
        assert!(s.contains("unexpected-eof"));
        assert!(s.contains("1..4"));
    }
}
