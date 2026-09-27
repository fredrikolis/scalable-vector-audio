// Concern: parses a stop condition into the engine's Until | Non-concern: where it holds (sva-engine), the expression grammar | IO: (text, rate, tempo) -> Until or CliError

use sva_ast::{LogUnit, SpanUnit, Token, TokenKind, tokenize};
use sva_engine::{Cmp, Term, Until};

use crate::cli_error::CliError;

/// `t` in seconds is compared with times, a level with levels; `and` binds tighter than `or`.
pub fn until(text: &str, rate: u32, seconds_per_bar: Option<f64>) -> Result<Until, CliError> {
    let tokens = tokenize(text).map_err(|d| refused(text, &d.message))?;
    let mut reading = Reading {
        text,
        tokens: &tokens,
        at: 0,
        rate,
        seconds_per_bar,
    };
    let held = reading.either()?;
    if reading.at != tokens.len() {
        return Err(reading.wrong("the condition ended"));
    }
    held.checked().map_err(|why| refused(text, &why))?;
    Ok(held)
}

struct Reading<'a> {
    text: &'a str,
    tokens: &'a [Token],
    at: usize,
    rate: u32,
    seconds_per_bar: Option<f64>,
}

impl Reading<'_> {
    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.at).map(|t| &t.kind)
    }

    fn word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(TokenKind::Ident(w)) if w == word)
    }

    fn take(&mut self, kind: &TokenKind) -> Result<(), CliError> {
        match self.peek() {
            Some(k) if k == kind => {
                self.at += 1;
                Ok(())
            }
            _ => Err(self.wrong(&format!("`{}`", spelled(kind)))),
        }
    }

    fn either(&mut self) -> Result<Until, CliError> {
        let mut held = self.both()?;
        while self.word("or") {
            self.at += 1;
            held = Until::Any(Box::new(held), Box::new(self.both()?));
        }
        Ok(held)
    }

    fn both(&mut self) -> Result<Until, CliError> {
        let mut held = self.one()?;
        while self.word("and") {
            self.at += 1;
            held = Until::All(Box::new(held), Box::new(self.one()?));
        }
        Ok(held)
    }

    fn one(&mut self) -> Result<Until, CliError> {
        if self.peek() == Some(&TokenKind::LParen) {
            self.at += 1;
            let held = self.either()?;
            self.take(&TokenKind::RParen)?;
            return Ok(held);
        }
        let from = self.at;
        let left = self.term()?;
        let left_bare = self.bare_number(from);
        let cmp = match self.peek() {
            Some(TokenKind::Lt) => Cmp::Lt,
            Some(TokenKind::Le) => Cmp::Le,
            Some(TokenKind::Gt) => Cmp::Gt,
            Some(TokenKind::Ge) => Cmp::Ge,
            _ => return Err(self.wrong("a comparison, `<`, `<=`, `>` or `>=`")),
        };
        self.at += 1;
        let from = self.at;
        let right = self.term()?;
        let bare = match (level(&left), level(&right)) {
            (true, false) => self.bare_number(from),
            (false, true) => left_bare,
            _ => None,
        };
        if let Some((start, end)) = bare {
            return Err(refused(
                self.text,
                &format!(
                    "`{}` (at bytes {start}..{end}) is compared with a level and has no level \
                     unit; write it in `db`",
                    &self.text[start..end]
                ),
            ));
        }
        Ok(Until::Holds(left, cmp, right))
    }

    /// The bytes of the number `term` read from token `from` on, where it was written
    /// with no unit; `0` is zero in every unit.
    fn bare_number(&self, from: usize) -> Option<(usize, usize)> {
        let read = &self.tokens[from..self.at];
        let unitless = read
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Num(n) if n != 0.0));
        match (unitless, read.first(), read.last()) {
            (true, Some(first), Some(last)) => Some((first.span.start, last.span.end)),
            _ => None,
        }
    }

    fn term(&mut self) -> Result<Term, CliError> {
        if self.word("t") {
            self.at += 1;
            return Ok(Term::Time);
        }
        if self.word("envelope") {
            self.at += 1;
            self.take(&TokenKind::LParen)?;
            if !self.word("t") {
                return Err(self.wrong("`t`"));
            }
            self.at += 1;
            self.take(&TokenKind::RParen)?;
            return Ok(Term::Envelope);
        }
        self.number().map(Term::Number)
    }

    /// A time in seconds or a level in `db`, a minus before it allowed; `0` needs no unit.
    fn number(&mut self) -> Result<f64, CliError> {
        let sign = match self.peek() {
            Some(TokenKind::Minus) => {
                self.at += 1;
                -1.0
            }
            _ => 1.0,
        };
        let value = match self.peek() {
            Some(TokenKind::Num(n)) => sign * n,
            Some(TokenKind::Time(n, SpanUnit::Seconds)) => sign * n,
            Some(TokenKind::Samples(n)) => sign * n / f64::from(self.rate),
            Some(TokenKind::Time(n, SpanUnit::Bars)) => match self.seconds_per_bar {
                Some(per) => sign * n * per,
                None => {
                    return Err(CliError::BadTempo(format!(
                        "`{}` is written in bars and nothing here declares a tempo; state \
                         `variables/bpm` and `variables/meter`, or write seconds",
                        self.text
                    )));
                }
            },
            Some(TokenKind::Log(n, LogUnit::Decibels)) => LogUnit::Decibels.resolve(sign * n),
            _ => return Err(self.wrong("a number, a time or a level")),
        };
        self.at += 1;
        Ok(value)
    }

    fn wrong(&self, wanted: &str) -> CliError {
        let found = self.tokens.get(self.at).map_or("the end".to_string(), |t| {
            format!("`{}`", &self.text[t.span.start..t.span.end])
        });
        refused(self.text, &format!("expected {wanted}, found {found}"))
    }
}

fn level(term: &Term) -> bool {
    matches!(term, Term::Envelope)
}

fn spelled(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        _ => "?",
    }
}

fn refused(text: &str, why: &str) -> CliError {
    CliError::Usage(format!(
        "`{text}` is no condition: {why}. write comparisons over `t` and `envelope(t)`, \
         joined by `and`/`or`, as `envelope(t) < -60db and t > 1s`"
    ))
}
