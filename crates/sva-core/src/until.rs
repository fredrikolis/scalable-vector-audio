// Concern: parses a stop condition into the engine's Until | Non-concern: where it holds (sva-engine), the expression grammar | IO: (text, rate, tempo) -> Until or CliError

use sva_ast::{LogUnit, SpanUnit, Token, TokenKind, tokenize};
use sva_engine::{At, Cmp, Term, Until};

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
        let left = self.term()?;
        let cmp = match self.peek() {
            Some(TokenKind::Lt) => Cmp::Lt,
            Some(TokenKind::Le) => Cmp::Le,
            Some(TokenKind::Gt) => Cmp::Gt,
            Some(TokenKind::Ge) => Cmp::Ge,
            _ => return Err(self.wrong("a comparison, `<`, `<=`, `>` or `>=`")),
        };
        self.at += 1;
        let right = self.term()?;
        Ok(Until::Holds(left, cmp, right))
    }

    fn term(&mut self) -> Result<Term, CliError> {
        if self.word("t") {
            self.at += 1;
            return Ok(Term::Time);
        }
        if self.word("envelope") {
            self.at += 1;
            self.take(&TokenKind::LParen)?;
            let at = self.instant()?;
            self.take(&TokenKind::RParen)?;
            return Ok(Term::Envelope(at));
        }
        for (name, make) in [("max", Term::Max as fn(At, At) -> Term), ("min", Term::Min)] {
            if self.word(name) {
                self.at += 1;
                self.take(&TokenKind::LParen)?;
                if !self.word("envelope") {
                    return Err(self.wrong("`envelope`"));
                }
                self.at += 1;
                self.take(&TokenKind::LParen)?;
                let (from, to) = self.interval()?;
                self.take(&TokenKind::RParen)?;
                self.take(&TokenKind::RParen)?;
                return Ok(make(from, to));
            }
        }
        self.number().map(Term::Number)
    }

    fn interval(&mut self) -> Result<(At, At), CliError> {
        self.take(&TokenKind::LBracket)?;
        let from = self.instant()?;
        self.take(&TokenKind::Comma)?;
        let to = match self.word("inf") {
            true => {
                self.at += 1;
                At::Inf
            }
            false => self.instant()?,
        };
        match self.peek() {
            Some(TokenKind::RBracket | TokenKind::RParen) => {
                self.at += 1;
                Ok((from, to))
            }
            _ => Err(self.wrong("`]` or `)`")),
        }
    }

    /// `t`, `t` plus or minus a time, or a time.
    fn instant(&mut self) -> Result<At, CliError> {
        if !self.word("t") {
            return self.number().map(At::Secs);
        }
        self.at += 1;
        let sign = match self.peek() {
            Some(TokenKind::Plus) => 1.0,
            Some(TokenKind::Minus) => -1.0,
            _ => return Ok(At::Now(0.0)),
        };
        self.at += 1;
        Ok(At::Now(sign * self.number()?))
    }

    /// A time in seconds or a level as amplitude, a minus before it allowed; `0` and a level
    /// need no unit.
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

fn spelled(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::LBracket => "[",
        TokenKind::Comma => ",",
        _ => "?",
    }
}

fn refused(text: &str, why: &str) -> CliError {
    CliError::Usage(format!(
        "`{text}` is no condition: {why}. write comparisons over `t`, `envelope(t)` and \
         `max`/`min(envelope([a, b]))`, joined by `and`/`or`, as \
         `max(envelope([t, inf))) < -96db`"
    ))
}
