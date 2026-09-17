// Concern: tokenizes one expression string, locates its `@ref` spans | Non-concern: how tokens combine (parser.rs), directory/filename parsing | IO: (&str) -> Vec<Token> or a Diag

use crate::diag::{ByteSpan, Diag, DiagCode};
use crate::filename::SpanUnit;

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: ByteSpan,
}

/// A logarithmic unit folds a leading `-` into itself: `-3db` is 0.708, never `0 - 1.41`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogUnit {
    Decibels,
    Cents,
    Semitones,
}

impl LogUnit {
    pub fn resolve(self, written: f64) -> f64 {
        match self {
            LogUnit::Decibels => 10f64.powf(written / 20.0),
            LogUnit::Cents => 2f64.powf(written / 1200.0),
            LogUnit::Semitones => 2f64.powf(written / 12.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Num(f64),
    Time(f64, SpanUnit),
    Samples(f64),
    Log(f64, LogUnit),
    Ident(String),
    Ref(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Dot,
    Comma,
    LParen,
    RParen,
}

/// Not `#`, which is the sharp sign, and not `//`, which keeps `/` for time signatures.
pub const COMMENT: u8 = b';';

/// The part of one line the lexer will read. A caller splitting content into lines strips
/// each one with this, so a comment can never be mistaken for a cell or an extra row.
pub fn strip_line_comment(line: &str) -> &str {
    match line.find(COMMENT as char) {
        Some(at) => &line[..at],
        None => line,
    }
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, Diag> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == COMMENT {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;

        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let (n, after_digits) = lex_number(src, b, i)?;
            let (kind, next) = match unit_suffix(b, after_digits) {
                Some((unit, after_unit)) => (unit.apply(n), after_unit),
                None => (TokenKind::Num(n), after_digits),
            };
            out.push(Token {
                kind,
                span: ByteSpan::new(start, next),
            });
            i = next;
            continue;
        }

        if c == b'@' {
            let (path, next) = lex_ref_path(b, i + 1);
            if next == i + 1 {
                return Err(Diag::new(
                    DiagCode::UnexpectedChar,
                    ByteSpan::new(start, start + 1),
                    "`@` must be followed by a ref path",
                ));
            }
            out.push(Token {
                kind: TokenKind::Ref(path),
                span: ByteSpan::new(start, next),
            });
            i = next;
            continue;
        }

        if c.is_ascii_alphabetic() || c == b'_' {
            let next = lex_ident_end(b, i);
            out.push(Token {
                kind: TokenKind::Ident(src[start..next].to_string()),
                span: ByteSpan::new(start, next),
            });
            i = next;
            continue;
        }

        let (kind, len) = match c {
            b'+' => (TokenKind::Plus, 1),
            b'-' => (TokenKind::Minus, 1),
            b'*' => (TokenKind::Star, 1),
            b'/' => (TokenKind::Slash, 1),
            b'%' => (TokenKind::Percent, 1),
            b'=' => (TokenKind::Eq, 1),
            b'.' => (TokenKind::Dot, 1),
            b',' => (TokenKind::Comma, 1),
            b'(' => (TokenKind::LParen, 1),
            b')' => (TokenKind::RParen, 1),
            _ => {
                let ch_len = utf8_char_len(c);
                return Err(Diag::new(
                    DiagCode::UnexpectedChar,
                    ByteSpan::new(start, (start + ch_len).min(b.len())),
                    format!("unexpected character {:?}", first_char_at(src, start)),
                ));
            }
        };
        out.push(Token {
            kind,
            span: ByteSpan::new(start, start + len),
        });
        i += len;
    }

    Ok(out)
}

/// Byte spans of every `@ref` occurrence in `src`, in source order — just the `@path` head,
/// never a trailing `(...)` invocation. Empty (not an error) on unlexable input.
pub fn ref_spans(src: &str) -> Vec<ByteSpan> {
    tokenize(src)
        .map(|tokens| {
            tokens
                .into_iter()
                .filter_map(|t| matches!(t.kind, TokenKind::Ref(_)).then_some(t.span))
                .collect()
        })
        .unwrap_or_default()
}

/// Longest match first, which is the whole reason `2ms` and `2m` are different numbers.
const SUFFIXES: [(&str, Suffix); 11] = [
    ("khz", Suffix::Scale(1000.0)),
    ("ms", Suffix::Secs(0.001)),
    ("sp", Suffix::Samples),
    ("hz", Suffix::Scale(1.0)),
    ("db", Suffix::Log(LogUnit::Decibels)),
    ("ct", Suffix::Log(LogUnit::Cents)),
    ("st", Suffix::Log(LogUnit::Semitones)),
    ("b", Suffix::Bars),
    ("s", Suffix::Secs(1.0)),
    ("m", Suffix::Secs(60.0)),
    ("h", Suffix::Secs(3600.0)),
];

#[derive(Clone, Copy)]
enum Suffix {
    Bars,
    Secs(f64),
    Samples,
    Scale(f64),
    Log(LogUnit),
}

impl Suffix {
    fn apply(self, n: f64) -> TokenKind {
        match self {
            Suffix::Bars => TokenKind::Time(n, SpanUnit::Bars),
            Suffix::Secs(per) => TokenKind::Time(n * per, SpanUnit::Seconds),
            Suffix::Samples => TokenKind::Samples(n),
            Suffix::Scale(per) => TokenKind::Num(n * per),
            Suffix::Log(unit) => TokenKind::Log(n, unit),
        }
    }
}

/// Only when nothing continues the word, so `2sin(t)` and `2sr` still lex as a number beside
/// an identifier.
fn unit_suffix(b: &[u8], i: usize) -> Option<(Suffix, usize)> {
    for (name, suffix) in SUFFIXES {
        let end = i + name.len();
        if b.get(i..end) != Some(name.as_bytes()) {
            continue;
        }
        return match b.get(end) {
            Some(c) if c.is_ascii_alphanumeric() || *c == b'_' => None,
            _ => Some((suffix, end)),
        };
    }
    None
}

fn lex_number(src: &str, b: &[u8], mut i: usize) -> Result<(f64, usize), Diag> {
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let lexeme = &src[start..i];
    match lexeme.parse::<f64>() {
        Ok(n) if n.is_finite() => Ok((n, i)),
        _ => Err(Diag::new(
            DiagCode::InvalidNumber,
            ByteSpan::new(start, i),
            format!("`{lexeme}` is not a finite number"),
        )),
    }
}

/// Whether a whole string is a ref path and nothing else, which a node's name has to be.
pub fn whole_ref_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    lex_ref_path(bytes, 0).1 == bytes.len()
}

/// A `@`-prefixed path: alnum, `_`, `-`, `/` — its own lexical class, so `@lead-dry` never
/// splits at `-` the way a subtraction would. A `.` joins the path when a digit follows it, so
/// `@pluck-1.5s` is one token, or when it opens a `./` or `../` segment; `@kick.lp(...)` still
/// ends at the dot and chains.
fn lex_ref_path(b: &[u8], mut i: usize) -> (String, usize) {
    let start = i;
    loop {
        if i < b.len()
            && (b[i].is_ascii_alphanumeric()
                || matches!(b[i], b'_' | b'-')
                || (b[i] == b'/' && i > start && opens_segment(b, i + 1)))
        {
            i += 1;
        } else if let Some(next) = relative_segment(b, i, start) {
            i = next;
        } else if i < b.len()
            && b[i] == b'.'
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
            && i > start
        {
            i += 2;
        } else {
            break;
        }
    }
    (String::from_utf8_lossy(&b[start..i]).into_owned(), i)
}

/// A `/` continues the path only into a NAME, so `@env/0.75` divides rather than reaching for a
/// directory. The cost is that a path segment after the first cannot begin with a digit.
fn opens_segment(b: &[u8], at: usize) -> bool {
    match b.get(at) {
        Some(c) if c.is_ascii_alphabetic() || *c == b'_' => true,
        Some(b'.') => matches!(
            (b.get(at + 1), b.get(at + 2)),
            (Some(b'/'), _) | (Some(b'.'), Some(b'/'))
        ),
        _ => false,
    }
}

/// `./` or `../`, and only where a segment begins, so a dot after a name still chains.
fn relative_segment(b: &[u8], i: usize, start: usize) -> Option<usize> {
    if i > start && b[i - 1] != b'/' {
        return None;
    }
    match (b.get(i), b.get(i + 1), b.get(i + 2)) {
        (Some(b'.'), Some(b'/'), _) => Some(i + 2),
        (Some(b'.'), Some(b'.'), Some(b'/')) => Some(i + 3),
        _ => None,
    }
}

fn lex_ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
        i += 1;
    }
    i
}

fn utf8_char_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

fn first_char_at(src: &str, at: usize) -> char {
    src[at..].chars().next().unwrap_or('\u{FFFD}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn numbers_lex() {
        assert_eq!(kinds("120"), vec![TokenKind::Num(120.0)]);
        assert_eq!(kinds(".5"), vec![TokenKind::Num(0.5)]);
        assert_eq!(kinds("1.2e3"), vec![TokenKind::Num(1200.0)]);
        assert_eq!(tokenize("1e999").unwrap_err().code, DiagCode::InvalidNumber);
    }

    #[test]
    fn a_unit_suffix_makes_a_time_literal_but_only_when_the_word_ends_there() {
        assert_eq!(kinds("0.25b"), vec![TokenKind::Time(0.25, SpanUnit::Bars)]);
        assert_eq!(kinds("1.5s"), vec![TokenKind::Time(1.5, SpanUnit::Seconds)]);
        assert_eq!(
            kinds("t % 1b"),
            vec![
                TokenKind::Ident("t".to_string()),
                TokenKind::Percent,
                TokenKind::Time(1.0, SpanUnit::Bars),
            ]
        );
        assert_eq!(
            kinds("2sq"),
            vec![TokenKind::Num(2.0), TokenKind::Ident("sq".to_string())],
            "a suffix only ends a word, it never starts an identifier"
        );
    }

    /// Longest match is the whole reason `2ms` and `2m` are different numbers.
    #[test]
    fn every_unit_suffix_resolves_at_parse_time_and_the_longest_one_wins() {
        assert_eq!(
            kinds("2ms"),
            vec![TokenKind::Time(0.002, SpanUnit::Seconds)]
        );
        assert_eq!(kinds("2m"), vec![TokenKind::Time(120.0, SpanUnit::Seconds)]);
        assert_eq!(
            kinds("2h"),
            vec![TokenKind::Time(7200.0, SpanUnit::Seconds)]
        );
        assert_eq!(kinds("2s"), vec![TokenKind::Time(2.0, SpanUnit::Seconds)]);
        assert_eq!(kinds("2b"), vec![TokenKind::Time(2.0, SpanUnit::Bars)]);
        assert_eq!(kinds("2sp"), vec![TokenKind::Samples(2.0)]);
        assert_eq!(kinds("2khz"), vec![TokenKind::Num(2000.0)]);
        assert_eq!(kinds("440hz"), vec![TokenKind::Num(440.0)]);
        assert_eq!(kinds("3db"), vec![TokenKind::Log(3.0, LogUnit::Decibels)]);
        assert_eq!(kinds("7ct"), vec![TokenKind::Log(7.0, LogUnit::Cents)]);
        assert_eq!(kinds("3st"), vec![TokenKind::Log(3.0, LogUnit::Semitones)]);
        assert_eq!(
            kinds("2msx"),
            vec![TokenKind::Num(2.0), TokenKind::Ident("msx".to_string())],
            "a suffix still only ends a word"
        );
    }

    #[test]
    fn ref_paths_lex_as_one_token_across_hyphens_and_slashes() {
        assert_eq!(
            kinds("@lead-dry"),
            vec![TokenKind::Ref("lead-dry".to_string())]
        );
        assert_eq!(
            kinds("@drums/kick"),
            vec![TokenKind::Ref("drums/kick".to_string())]
        );
        assert_eq!(
            tokenize("@").unwrap_err().code,
            DiagCode::UnexpectedChar,
            "a bare @ with no path refuses"
        );
    }

    /// The reciprocal that used to be swallowed: `@env/0.75` was the path `env/0.75`.
    #[test]
    fn a_slash_continues_a_ref_path_only_into_a_name_so_a_number_divides() {
        assert_eq!(
            kinds("@x/2"),
            vec![
                TokenKind::Ref("x".to_string()),
                TokenKind::Slash,
                TokenKind::Num(2.0),
            ]
        );
        assert_eq!(
            kinds("@env/0.75"),
            vec![
                TokenKind::Ref("env".to_string()),
                TokenKind::Slash,
                TokenKind::Num(0.75),
            ]
        );
        assert_eq!(
            kinds("@drums/kick/2"),
            vec![
                TokenKind::Ref("drums/kick".to_string()),
                TokenKind::Slash,
                TokenKind::Num(2.0),
            ],
            "the named segments still join"
        );
        assert_eq!(
            kinds("@a/_b"),
            vec![TokenKind::Ref("a/_b".to_string())],
            "an underscore opens a name"
        );
    }

    #[test]
    fn a_dot_joins_a_ref_path_only_when_a_digit_follows_it() {
        assert_eq!(
            kinds("@pluck-1.5s"),
            vec![TokenKind::Ref("pluck-1.5s".to_string())],
            "a fractional-second span must be referenceable"
        );
        assert_eq!(
            kinds("@kick.lp"),
            vec![
                TokenKind::Ref("kick".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("lp".to_string()),
            ],
            "chain sugar still ends the ref at the dot"
        );
        assert_eq!(
            tokenize("@.5").unwrap_err().code,
            DiagCode::UnexpectedChar,
            "a path may not open with a dot"
        );
    }

    #[test]
    fn a_dot_segment_joins_a_ref_path_wherever_a_segment_begins() {
        assert_eq!(
            kinds("@./partials"),
            vec![TokenKind::Ref("./partials".to_string())]
        );
        assert_eq!(
            kinds("@../variables/bpm"),
            vec![TokenKind::Ref("../variables/bpm".to_string())]
        );
        assert_eq!(
            kinds("@../../a/../b"),
            vec![TokenKind::Ref("../../a/../b".to_string())]
        );
        assert_eq!(
            kinds("@../pluck-1.5s"),
            vec![TokenKind::Ref("../pluck-1.5s".to_string())],
            "a fractional-second span stays referenceable from a sibling directory"
        );
        assert_eq!(
            kinds("@../kick.lp"),
            vec![
                TokenKind::Ref("../kick".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("lp".to_string()),
            ],
            "a dot after a name still ends the ref and chains"
        );
    }

    #[test]
    fn idents_and_operators_lex() {
        assert_eq!(
            kinds("sin(2*pi*t)"),
            vec![
                TokenKind::Ident("sin".to_string()),
                TokenKind::LParen,
                TokenKind::Num(2.0),
                TokenKind::Star,
                TokenKind::Ident("pi".to_string()),
                TokenKind::Star,
                TokenKind::Ident("t".to_string()),
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn a_semicolon_comments_out_the_rest_of_its_line() {
        assert_eq!(kinds("sin(t) ; a #-free, /-free marker"), kinds("sin(t)"));
        assert_eq!(
            kinds("; leading\nsin(t) ; trailing\n"),
            kinds("sin(t)"),
            "a comment ends at the newline, not at the end of input"
        );
        assert!(tokenize("; nothing but a comment").unwrap().is_empty());
        assert_eq!(strip_line_comment("@kick*0.9 ; loud"), "@kick*0.9 ");
        assert_eq!(strip_line_comment("@kick"), "@kick");
    }

    #[test]
    fn hostile_bytes_are_located_never_panic() {
        assert_eq!(tokenize("\\").unwrap_err().code, DiagCode::UnexpectedChar);
        let d = tokenize("λ").unwrap_err();
        assert_eq!(d.code, DiagCode::UnexpectedChar);
        assert_eq!(
            d.span,
            ByteSpan::new(0, 2),
            "the span must not split a char"
        );
    }

    #[test]
    fn ref_spans_cover_only_the_at_path_head_never_a_call_or_non_ref_content() {
        let src = "@drums/kick(t) + @a/../b";
        let spans = ref_spans(src);
        assert_eq!(spans.len(), 2);
        assert_eq!(&src[spans[0].start..spans[0].end], "@drums/kick");
        assert_eq!(&src[spans[1].start..spans[1].end], "@a/../b");

        assert_eq!(ref_spans("@kick"), vec![ByteSpan::new(0, 5)]);

        assert!(ref_spans("sin(2*pi*t) + 0.5").is_empty());
    }

    #[test]
    fn spans_are_sliceable_on_every_token() {
        let src = "@kick(t - 0.5) + sin(2*pi*t)";
        for t in tokenize(src).unwrap() {
            let _ = &src[t.span.start..t.span.end];
        }
    }
}
