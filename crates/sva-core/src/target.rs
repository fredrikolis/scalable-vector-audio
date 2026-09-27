// Concern: splits a target into the expression it renders and the interval its ref reads | Non-concern: the expression grammar (sva-ast), where an open interval ends | IO: (text) -> Target

use sva_ast::{Token, TokenKind, tokenize};

use crate::cli_error::CliError;

/// One end of an interval, in the units it was written in; a bare zero is zero in all of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    Secs(f64),
    Bars(f64),
    Samples(f64),
    Inf,
}

impl Edge {
    /// The grid sample this edge falls on; `None` where it is no edge at all.
    pub fn sample(self, rate: u32, seconds_per_bar: Option<f64>) -> Result<Option<i64>, CliError> {
        let secs = match self {
            Edge::Inf => return Ok(None),
            Edge::Samples(n) => return Ok(Some(n.round() as i64)),
            Edge::Secs(secs) => secs,
            Edge::Bars(bars) => match seconds_per_bar {
                Some(per) => bars * per,
                None => {
                    return Err(CliError::BadTempo(format!(
                        "the interval is written in bars and nothing here declares a tempo; \
                         state `variables/bpm` and `variables/meter`, or write seconds, as \
                         `{bars}s`"
                    )));
                }
            },
        };
        Ok(Some((secs * f64::from(rate)).round() as i64))
    }
}

/// The expression with its interval read as `t`, and that interval's two ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub expr: String,
    pub interval: Option<(Edge, Edge)>,
}

/// An interval is the argument of the target's own ref, `@a([0, 2b], vel=0.5)`: nowhere else.
pub fn target(text: &str) -> Result<Target, CliError> {
    let tokens = tokenize(text).map_err(|d| unparsed(text, &d.message))?;
    let opens: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TokenKind::LBracket)
        .map(|(at, _)| at)
        .collect();
    let at = match opens.as_slice() {
        [] => {
            return Ok(Target {
                expr: text.to_string(),
                interval: None,
            });
        }
        [at] => *at,
        _ => return Err(misplaced(text)),
    };
    let own_ref = matches!(
        tokens.as_slice(),
        [
            Token {
                kind: TokenKind::Ref(_),
                ..
            },
            Token {
                kind: TokenKind::LParen,
                ..
            },
            ..
        ]
    );
    if at != 2 || !own_ref {
        return Err(misplaced(text));
    }
    let comma = position(&tokens, at, |k| *k == TokenKind::Comma).ok_or_else(|| misplaced(text))?;
    let close = position(&tokens, comma, |k| {
        matches!(k, TokenKind::RBracket | TokenKind::RParen)
    })
    .ok_or_else(|| misplaced(text))?;
    if closing(&tokens, close) != Some(tokens.len() - 1)
        || !matches!(
            tokens.get(close + 1).map(|t| &t.kind),
            Some(TokenKind::Comma | TokenKind::RParen)
        )
    {
        return Err(misplaced(text));
    }
    let start = edge(text, &tokens[at + 1..comma])?;
    let end = match &tokens[comma + 1..close] {
        [] => Edge::Inf,
        written => edge(text, written)?,
    };
    if start == Edge::Inf {
        return Err(CliError::Usage(format!(
            "`{text}` starts its interval at inf; start it at a time, as `[0, inf)`"
        )));
    }
    let expr = format!(
        "{}t{}",
        &text[..tokens[at].span.start],
        &text[tokens[close].span.end..]
    );
    Ok(Target {
        expr,
        interval: Some((start, end)),
    })
}

/// The index of the `)` closing the call the second token opens, the interval's own end
/// at `interval` aside.
fn closing(tokens: &[Token], interval: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (at, token) in tokens.iter().enumerate().skip(1) {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen if at != interval => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

fn position(tokens: &[Token], from: usize, found: impl Fn(&TokenKind) -> bool) -> Option<usize> {
    (from + 1..tokens.len()).find(|at| found(&tokens[*at].kind))
}

/// `inf`, or one literal of time, a minus before it allowed; `0` needs no unit.
fn edge(text: &str, written: &[Token]) -> Result<Edge, CliError> {
    let (sign, rest) = match written {
        [
            Token {
                kind: TokenKind::Minus,
                ..
            },
            rest @ ..,
        ] => (-1.0, rest),
        rest => (1.0, rest),
    };
    Ok(match rest {
        [
            Token {
                kind: TokenKind::Ident(inf),
                ..
            },
        ] if inf == "inf" && sign > 0.0 => Edge::Inf,
        [
            Token {
                kind: TokenKind::Num(n),
                ..
            },
        ] if *n == 0.0 => Edge::Secs(0.0),
        [
            Token {
                kind: TokenKind::Time(n, sva_ast::SpanUnit::Seconds),
                ..
            },
        ] => Edge::Secs(sign * n),
        [
            Token {
                kind: TokenKind::Time(n, sva_ast::SpanUnit::Bars),
                ..
            },
        ] => Edge::Bars(sign * n),
        [
            Token {
                kind: TokenKind::Samples(n),
                ..
            },
        ] => Edge::Samples(sign * n),
        _ => {
            let spelled = written
                .first()
                .zip(written.last())
                .map_or("", |(a, b)| &text[a.span.start..b.span.end]);
            return Err(CliError::Usage(format!(
                "`{spelled}` is no interval edge: write a time with its unit, as `2s`, \
                 `500ms`, `2b` or `88200sp`, or `inf`"
            )));
        }
    })
}

fn misplaced(text: &str) -> CliError {
    CliError::Usage(format!(
        "`{text}` holds an interval outside its own ref's argument; write one ref read over \
         it, as `@a([0, 2b], vel=0.5)`"
    ))
}

fn unparsed(text: &str, why: &str) -> CliError {
    CliError::BadProbe(format!("`{text}` does not parse as a target: {why}"))
}
