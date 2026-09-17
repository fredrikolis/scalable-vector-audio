// Concern: builds an Expr from one expression string's tokens | Non-concern: lexing, ref resolution, TSV desugaring (tsv.rs) | IO: (&str) -> Expr or a located Diag

use crate::diag::{ByteSpan, Diag, DiagCode};
use crate::expr::{Arg, BinOp, Expr, Literal};
use crate::filename::SpanUnit;
use crate::lexer::{Token, TokenKind, tokenize};

pub const MAX_DEPTH: u32 = 128;
pub const MAX_TOKENS: usize = 4096;

pub fn parse(src: &str) -> Result<Expr, Diag> {
    let tokens = tokenize(src)?;
    if tokens.is_empty() {
        return Err(Diag::new(
            DiagCode::EmptyExpression,
            ByteSpan::at(src.len()),
            "the expression is empty",
        ));
    }
    if tokens.len() > MAX_TOKENS {
        return Err(Diag::new(
            DiagCode::RecursionLimit,
            tokens[MAX_TOKENS].span,
            format!("expression exceeds the {MAX_TOKENS}-token size bound"),
        ));
    }

    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        depth: 0,
        end: src.len(),
    };
    let expr = p.parse_expr(0)?;
    if let Some(t) = p.peek() {
        let (code, msg) = match t.kind {
            TokenKind::RParen => (
                DiagCode::UnbalancedParen,
                "a ) has no matching (".to_string(),
            ),
            _ => (
                DiagCode::UnexpectedToken,
                "unexpected trailing input after a complete expression".to_string(),
            ),
        };
        return Err(Diag::new(code, t.span, msg));
    }
    Ok(expr)
}

/// The units a time position accepts, in the order a refusal names them.
pub const TIME_UNITS: &str = "b, ms, s, m, h or sp";

/// One argument beside the tokens it was written from, which is what a time check reads.
struct Placed {
    arg: Arg,
    from: usize,
    to: usize,
}

impl Placed {
    /// A chained receiver was written before the argument list and holds no tokens of it.
    fn receiver(e: Expr) -> Placed {
        Placed {
            arg: Arg::Pos(e),
            from: 0,
            to: 0,
        }
    }
}

fn bare(args: Vec<Placed>) -> Vec<Arg> {
    args.into_iter().map(|p| p.arg).collect()
}

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
    depth: u32,
    end: usize,
}

impl<'t> Parser<'t> {
    fn peek(&self) -> Option<&'t Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&'t Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eof_span(&self) -> ByteSpan {
        ByteSpan::at(self.tokens.last().map_or(self.end, |t| t.span.end))
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, Diag> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            let span = self.peek().map_or_else(|| self.eof_span(), |t| t.span);
            return Err(Diag::new(
                DiagCode::RecursionLimit,
                span,
                format!("expression nests deeper than the {MAX_DEPTH}-level bound"),
            ));
        }
        let r = self.parse_expr_inner(min_bp);
        self.depth -= 1;
        r
    }

    fn parse_expr_inner(&mut self, min_bp: u8) -> Result<Expr, Diag> {
        let mut lhs = self.parse_prefix()?;

        loop {
            lhs = self.try_chain_dot(lhs)?;

            let Some(tok) = self.peek() else { break };
            let kind = tok.kind.clone();
            let Some((l_bp, r_bp)) = infix_bp(&kind) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_expr(r_bp)?;
            lhs = Expr::Bin(bin_op(&kind), Box::new(lhs), Box::new(rhs));
        }

        Ok(lhs)
    }

    /// `.name(args)` after a primary is sugar for `name(primary, args)` — desugared here so
    /// the tree only ever holds plain nested `Call`s (per FORMAT.md).
    fn try_chain_dot(&mut self, receiver: Expr) -> Result<Expr, Diag> {
        let mut lhs = receiver;
        while matches!(self.peek().map(|t| &t.kind), Some(TokenKind::Dot)) {
            self.advance();
            let name_span = self.peek().map_or_else(|| self.eof_span(), |t| t.span);
            let name = match self.advance().map(|t| &t.kind) {
                Some(TokenKind::Ident(n)) => n.clone(),
                _ => {
                    return Err(Diag::new(
                        DiagCode::UnexpectedToken,
                        name_span,
                        "`.` must be followed by a function name",
                    ));
                }
            };
            if !matches!(self.peek().map(|t| &t.kind), Some(TokenKind::LParen)) {
                return Err(Diag::new(
                    DiagCode::UnexpectedToken,
                    self.peek().map_or_else(|| self.eof_span(), |t| t.span),
                    "chain notation `.name(...)` requires a call",
                ));
            }
            let mut args = vec![Placed::receiver(lhs)];
            args.extend(self.parse_call_args()?);
            self.refuse_bare_times(&name, &args)?;
            lhs = Expr::Call {
                name,
                args: bare(args),
                span: name_span,
            };
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, Diag> {
        let Some(tok) = self.advance() else {
            return Err(Diag::new(
                DiagCode::UnexpectedEof,
                self.eof_span(),
                "input ended where a value was expected",
            ));
        };
        let span = tok.span;
        if let Some(lit) = literal(&tok.kind, 1.0) {
            return Ok(Expr::Lit(lit));
        }
        match &tok.kind {
            TokenKind::Minus => {
                if let Some(folded) = self.peek().and_then(|t| literal(&t.kind, -1.0)) {
                    self.advance();
                    return Ok(Expr::Lit(folded));
                }
                let rhs = self.parse_expr(PREFIX_BP)?;
                Ok(Expr::Bin(
                    BinOp::Sub,
                    Box::new(Expr::Lit(Literal::Num(0.0))),
                    Box::new(rhs),
                ))
            }
            TokenKind::Plus => self.parse_expr(PREFIX_BP),
            TokenKind::LParen => {
                let inner = self.parse_expr(0)?;
                match self.peek().map(|t| &t.kind) {
                    Some(TokenKind::RParen) => {
                        self.advance();
                        Ok(inner)
                    }
                    _ => Err(Diag::new(
                        DiagCode::UnclosedParen,
                        self.eof_span(),
                        "a ( was never closed",
                    )),
                }
            }
            TokenKind::Ref(path) => {
                let (arg, binds) =
                    if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::LParen)) {
                        self.parse_invocation(path, span)?
                    } else {
                        (Expr::Var("t".to_string()), Vec::new())
                    };
                Ok(Expr::Ref {
                    path: path.clone(),
                    arg: Box::new(arg),
                    binds,
                    span,
                })
            }
            TokenKind::Ident(name) if name == "self" => {
                if !matches!(self.peek().map(|t| &t.kind), Some(TokenKind::LParen)) {
                    return Err(Diag::new(
                        DiagCode::UnexpectedToken,
                        self.peek().map_or_else(|| self.eof_span(), |t| t.span),
                        "`self` requires a bounded self-reference argument: self(t - 1sp)",
                    ));
                }
                let mut args = self.parse_call_args()?;
                if args.len() != 1 {
                    return Err(Diag::new(
                        DiagCode::BadArity,
                        span,
                        format!("`self` takes exactly 1 argument, got {}", args.len()),
                    ));
                }
                let placed = args.remove(0);
                self.refuse_bare_time(placed.from, placed.to)?;
                let Arg::Pos(arg) = placed.arg else {
                    return Err(Diag::new(
                        DiagCode::BadArity,
                        span,
                        "`self`'s argument must be positional",
                    ));
                };
                Ok(Expr::SelfRef {
                    arg: Box::new(arg),
                    span,
                })
            }
            TokenKind::Ident(name) => {
                if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::LParen)) {
                    let args = self.parse_call_args()?;
                    self.refuse_bare_times(name, &args)?;
                    Ok(Expr::Call {
                        name: name.clone(),
                        args: bare(args),
                        span,
                    })
                } else {
                    Ok(Expr::Var(name.clone()))
                }
            }
            TokenKind::RParen => Err(Diag::new(
                DiagCode::UnbalancedParen,
                span,
                "a ) has no matching (",
            )),
            _ => Err(Diag::new(
                DiagCode::UnexpectedToken,
                span,
                "expected a value, ref, or ( here",
            )),
        }
    }

    fn parse_call_args(&mut self) -> Result<Vec<Placed>, Diag> {
        debug_assert!(matches!(
            self.peek().map(|t| &t.kind),
            Some(TokenKind::LParen)
        ));
        self.advance();

        let mut args: Vec<Placed> = Vec::new();
        if matches!(self.peek().map(|t| &t.kind), Some(TokenKind::RParen)) {
            self.advance();
            return Ok(args);
        }
        loop {
            let from = self.pos;
            let arg = self.parse_one_arg()?;
            args.push(Placed {
                arg,
                from,
                to: self.pos,
            });
            match self.peek().map(|t| &t.kind) {
                Some(TokenKind::Comma) => {
                    self.advance();
                }
                Some(TokenKind::RParen) => {
                    self.advance();
                    break;
                }
                Some(_) => {
                    let t = self.peek().unwrap();
                    return Err(Diag::new(
                        DiagCode::UnexpectedToken,
                        t.span,
                        "expected , or ) in an argument list",
                    ));
                }
                None => {
                    return Err(Diag::new(
                        DiagCode::UnexpectedEof,
                        self.eof_span(),
                        "an argument list was not closed",
                    ));
                }
            }
        }
        Ok(args)
    }

    /// `@f(t, cutoff=800)`: one positional argument, the time to sample at, then named
    /// bindings. A second positional refuses — a file states no parameter ORDER, only which
    /// names it leaves free, so there is nothing for a second position to mean.
    fn parse_invocation(
        &mut self,
        path: &str,
        span: ByteSpan,
    ) -> Result<(Expr, Vec<(String, Expr)>), Diag> {
        let args = self.parse_call_args()?;
        let mut arg = None;
        let mut binds: Vec<(String, Expr)> = Vec::new();
        for placed in args {
            if matches!(placed.arg, Arg::Pos(_)) && arg.is_none() && binds.is_empty() {
                self.refuse_bare_time(placed.from, placed.to)?;
            }
            match placed.arg {
                Arg::Pos(e) if arg.is_none() && binds.is_empty() => arg = Some(e),
                Arg::Pos(_) => {
                    return Err(Diag::new(
                        DiagCode::BadArity,
                        span,
                        format!(
                            "`@{path}` takes one positional argument (the time to read it at); \
                             every other argument must be named"
                        ),
                    ));
                }
                Arg::Named(k, e) => {
                    if binds.iter().any(|(seen, _)| *seen == k) {
                        return Err(Diag::new(
                            DiagCode::BadArity,
                            span,
                            format!("`@{path}` binds `{k}` twice"),
                        ));
                    }
                    binds.push((k, e));
                }
            }
        }
        match arg {
            Some(arg) => Ok((arg, binds)),
            None => Err(Diag::new(
                DiagCode::BadArity,
                span,
                format!("`@{path}(...)` needs a time argument first, as in `@{path}(t, ...)`"),
            )),
        }
    }

    /// `crop`'s bounds are the only time arguments a builtin takes; every other slot is a
    /// level, a frequency or an index, and a bare number there means what it says.
    fn refuse_bare_times(&self, name: &str, args: &[Placed]) -> Result<(), Diag> {
        if name != "crop" {
            return Ok(());
        }
        let mut at = 0usize;
        for placed in args {
            let timed = match &placed.arg {
                Arg::Pos(_) => {
                    at += 1;
                    at == 2 || at == 3
                }
                Arg::Named(k, _) => k == "start" || k == "end",
            };
            if timed {
                self.refuse_bare_time(placed.from, placed.to)?;
            }
        }
        Ok(())
    }

    /// Syntactic, never dimensional: a scaled number is a factor, anything else here is a
    /// duration whose meaning moves when `bpm` does. A call's arguments are its own. A group
    /// takes that factor test plus a clause — a scaled group writing no time of its own is
    /// dimensionless and not walked, so `tau*(1 - exp(-t/tau))` stands while `2*(4 - t)`
    /// refuses on its `4`.
    fn refuse_bare_time(&self, from: usize, to: usize) -> Result<(), Diag> {
        let mut at = from;
        while at < to {
            let kind = &self.tokens[at].kind;
            if matches!(kind, TokenKind::LParen) {
                let close = self.close_of(at, to);
                let skip = if self.opens_call(at, from) {
                    true
                } else {
                    self.is_factor(at, close, from, to) && !self.carries_time(at + 1, close)
                };
                at = if skip { close } else { at + 1 };
                continue;
            }
            let TokenKind::Num(n) = kind else {
                at += 1;
                continue;
            };
            if self.is_factor(at, at, from, to) {
                at += 1;
                continue;
            }
            return Err(Diag::new(
                DiagCode::BareTime,
                self.tokens[at].span,
                format!("`{n}` is a time here and needs a unit: {TIME_UNITS}"),
            ));
        }
        Ok(())
    }

    /// The index of the `)` closing the `(` at `open`, or `to` when the range ends first.
    fn close_of(&self, open: usize, to: usize) -> usize {
        let mut depth = 0usize;
        for at in open..to {
            match self.tokens[at].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return at;
                    }
                }
                _ => {}
            }
        }
        to
    }

    /// A `(` directly after a name opens that call's own arguments; anything else groups.
    /// `join` is the exception: its arguments are the caller's slot, one per component.
    fn opens_call(&self, open: usize, from: usize) -> bool {
        open > from
            && match &self.tokens[open - 1].kind {
                TokenKind::Ident(name) => name != crate::expr::JOIN,
                TokenKind::Ref(_) => true,
                _ => false,
            }
    }

    /// A term spanning `[first, last]` is a factor when a scaling operator sits on either
    /// side of it — the one position in an arithmetic expression a time cannot occupy.
    fn is_factor(&self, first: usize, last: usize, from: usize, to: usize) -> bool {
        let scaling = |k: Option<&TokenKind>| {
            matches!(
                k,
                Some(TokenKind::Star | TokenKind::Slash | TokenKind::Percent)
            )
        };
        let before = (first > from).then(|| &self.tokens[first - 1].kind);
        let after = self
            .tokens
            .get(last + 1)
            .filter(|_| last + 1 < to)
            .map(|t| &t.kind);
        scaling(before) || scaling(after)
    }

    /// Whether a range writes a time of its own: bare `t`, or a literal already carrying a
    /// span unit. A nested call's arguments are its own and do not count.
    fn carries_time(&self, from: usize, to: usize) -> bool {
        let mut at = from;
        while at < to {
            match &self.tokens[at].kind {
                TokenKind::LParen if self.opens_call(at, from) => {
                    at = self.close_of(at, to);
                }
                TokenKind::Time(..) | TokenKind::Samples(..) => return true,
                TokenKind::Ident(name) if name == "t" => return true,
                _ => at += 1,
            }
        }
        false
    }

    /// `name = expr` is a named argument; anything else is positional.
    fn parse_one_arg(&mut self) -> Result<Arg, Diag> {
        if let (
            Some(Token {
                kind: TokenKind::Ident(name),
                ..
            }),
            Some(Token {
                kind: TokenKind::Eq,
                ..
            }),
        ) = (self.tokens.get(self.pos), self.tokens.get(self.pos + 1))
        {
            let name = name.clone();
            self.pos += 2;
            let value = self.parse_expr(0)?;
            return Ok(Arg::Named(name, value));
        }
        Ok(Arg::Pos(self.parse_expr(0)?))
    }
}

/// A literal token as the node it names, scaled by the sign a prefix minus folded into it.
/// Folding rather than desugaring the minus to `0 - x` keeps one node per written literal, so
/// a printed negative reparses to the node it was printed from.
fn literal(kind: &TokenKind, sign: f64) -> Option<Literal> {
    Some(match kind {
        TokenKind::Num(n) | TokenKind::Time(n, SpanUnit::Seconds) => Literal::Num(sign * n),
        TokenKind::Time(n, SpanUnit::Bars) => Literal::Bars(sign * n),
        TokenKind::Samples(n) => Literal::Samples(sign * n),
        TokenKind::Log(n, unit) => Literal::Num(unit.resolve(sign * n)),
        _ => return None,
    })
}

const PREFIX_BP: u8 = 50;

fn infix_bp(kind: &TokenKind) -> Option<(u8, u8)> {
    Some(match kind {
        TokenKind::Plus | TokenKind::Minus => (10, 11),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (20, 21),
        _ => return None,
    })
}

fn bin_op(kind: &TokenKind) -> BinOp {
    match kind {
        TokenKind::Plus => BinOp::Add,
        TokenKind::Minus => BinOp::Sub,
        TokenKind::Star => BinOp::Mul,
        TokenKind::Slash => BinOp::Div,
        TokenKind::Percent => BinOp::Mod,
        _ => unreachable!("bin_op called with a non-infix token"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_grouping() {
        assert_eq!(
            parse("1+2*3").unwrap(),
            Expr::Bin(
                BinOp::Add,
                Box::new(Expr::Lit(Literal::Num(1.0))),
                Box::new(Expr::Bin(
                    BinOp::Mul,
                    Box::new(Expr::Lit(Literal::Num(2.0))),
                    Box::new(Expr::Lit(Literal::Num(3.0))),
                )),
            )
        );
        assert_eq!(parse("1+2*3").unwrap(), parse("(1+(2*3))").unwrap());
        assert_ne!(parse("(1+2)*3").unwrap(), parse("1+2*3").unwrap());
    }

    #[test]
    fn unary_minus_desugars_to_a_binary_subtraction() {
        assert_eq!(
            parse("-t").unwrap(),
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Lit(Literal::Num(0.0))),
                Box::new(Expr::Var("t".to_string())),
            )
        );
    }

    /// An invocation needs no new syntax beyond a ref taking an argument LIST: one positional
    /// time, then names. A second positional has nothing to mean — a file states no order.
    #[test]
    fn a_ref_takes_named_bindings_after_its_time_argument() {
        let Expr::Ref {
            path, arg, binds, ..
        } = parse("@lp-def(t, cutoff=800)").unwrap()
        else {
            panic!("expected a Ref")
        };
        assert_eq!(path, "lp-def");
        assert_eq!(*arg, Expr::Var("t".to_string()));
        assert_eq!(
            binds,
            vec![("cutoff".to_string(), Expr::Lit(Literal::Num(800.0)))]
        );

        assert_eq!(
            parse("@f(t - 1s, k=@g*2)").unwrap(),
            parse("@f(t - 1s, k=@g*2)").unwrap()
        );
        for bad in ["@f(t, 800)", "@f()", "@f(t, k=1, k=2)"] {
            assert_eq!(
                parse(bad).unwrap_err().code,
                DiagCode::BadArity,
                "{bad} must refuse"
            );
        }
    }

    /// The one dimension where a value's meaning changes under an edit somewhere else.
    #[test]
    fn a_bare_number_in_a_time_position_refuses_naming_the_units() {
        for src in [
            "@kick(t - 0.5)",
            "self(t - 0.01)",
            "crop(@a, 0, 2s)",
            "crop(@a, 0s, 2)",
            "crop(@a, start=0s, end=2)",
            "@a.crop(0s, 2)",
            "@kick(2*(4 - t))",
            "@kick(t - (0.5 + 0.25))",
        ] {
            let d = parse(src).unwrap_err();
            assert_eq!(d.code, DiagCode::BareTime, "{src} must refuse");
            assert!(d.message.contains("b, ms, s, m, h or sp"), "{src}");
        }
        for src in [
            "@kick(t - 0.5s)",
            "@kick(t - 128sp)",
            "@kick(t - 128sp)",
            "@kick(2*T - t)",
            "@kick(t + 0.005*sin(2*pi*0.5*t))",
            "crop(@a, 0s, 2b)",
            "crop(sin(1), 0s, 2s)",
            "@kick(t % 1b)",
            "lowpass(@a, 800, 2)",
            "rand(0, seed=3)",
            "@src(1s + 2s*(1 - exp(-t/2s)))",
            "@src((1 - exp(-t/2s))*2s + 1s)",
            "crop(@a, 0s, 2s*(1 - 0.5))",
            "self(t - 0.01s*(1 + 0.5*sin(2*pi*t)))",
        ] {
            assert!(parse(src).is_ok(), "{src} names no bare time");
        }
    }

    /// A series is a value, not an abbreviation: it reaches the tree as the call it was
    /// written as, index and bounds intact, and prints back to itself.
    #[test]
    fn sum_survives_parse_unexpanded() {
        let src = "sum(k, 0, 3, k*2)";
        let Expr::Call { name, args, .. } = parse(src).unwrap() else {
            panic!("a series parses as a call")
        };
        assert_eq!(name, "sum");
        assert_eq!(args.len(), 4);
        assert!(matches!(&args[0], Arg::Pos(Expr::Var(k)) if k == "k"));
        assert_eq!(crate::render_expr(&parse(src).unwrap()), src);
    }

    #[test]
    fn a_unit_is_a_number_by_the_time_the_tree_holds_it() {
        assert_eq!(parse("2ms").unwrap(), parse("0.002").unwrap());
        assert_eq!(
            parse("2sp").unwrap(),
            Expr::Lit(Literal::Samples(2.0)),
            "a sample count is the one unit no parse settles"
        );
        assert_eq!(parse("2khz").unwrap(), parse("2000").unwrap());
        assert_eq!(parse("0db").unwrap(), parse("1").unwrap());
        assert_eq!(parse("0st").unwrap(), parse("1").unwrap());
    }

    /// `-3db` is 0.708. Read as `0 - 3db` it would be -1.41, which is the wrong sign AND the
    /// wrong magnitude, so the minus belongs inside a logarithmic unit.
    #[test]
    fn a_minus_before_a_logarithmic_unit_is_part_of_the_literal() {
        let Expr::Lit(Literal::Num(v)) = parse("-3db").unwrap() else {
            panic!("a folded literal, not a subtraction");
        };
        assert!((v - 0.70794578).abs() < 1e-8, "{v}");
        let Expr::Lit(Literal::Num(v)) = parse("-12st").unwrap() else {
            panic!("a folded literal");
        };
        assert!((v - 0.5).abs() < 1e-12, "{v}");
        assert_eq!(
            parse("1 - 3db").unwrap(),
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Lit(Literal::Num(1.0))),
                Box::new(Expr::Lit(Literal::Num(10f64.powf(3.0 / 20.0))))
            ),
            "only a PREFIX minus folds; a subtraction is still a subtraction"
        );
    }

    #[test]
    fn refs_bare_and_time_shifted() {
        assert_eq!(
            parse("@kick").unwrap(),
            Expr::Ref {
                path: "kick".to_string(),
                arg: Box::new(Expr::Var("t".to_string())),
                binds: Vec::new(),
                span: ByteSpan::new(0, 5),
            }
        );
        let e = parse("@lead-dry(t - 0.375s)").unwrap();
        let Expr::Ref { path, arg, .. } = e else {
            panic!("expected Ref")
        };
        assert_eq!(path, "lead-dry");
        assert_eq!(
            *arg,
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Var("t".to_string())),
                Box::new(Expr::Lit(Literal::Num(0.375))),
            )
        );
    }

    /// Seconds ARE the engine's unit, so `1.5s` collapses to the number and only bars survive
    /// parsing as their own literal for tempo resolution to rewrite.
    #[test]
    fn a_seconds_literal_is_a_plain_number_and_a_bars_literal_is_not() {
        assert_eq!(parse("1.5s").unwrap(), parse("1.5").unwrap());
        assert_eq!(parse("0.25b").unwrap(), Expr::Lit(Literal::Bars(0.25)));
        assert_ne!(parse("1b").unwrap(), parse("1").unwrap());

        let Expr::Ref { arg, .. } = parse("@kick(t - 0.02b)").unwrap() else {
            panic!("expected a Ref")
        };
        assert_eq!(
            *arg,
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Var("t".to_string())),
                Box::new(Expr::Lit(Literal::Bars(0.02))),
            )
        );
    }

    #[test]
    fn self_ref_requires_exactly_one_positional_arg() {
        let e = parse("self(t - 1sp)").unwrap();
        assert!(matches!(e, Expr::SelfRef { .. }));
        assert_eq!(parse("self()").unwrap_err().code, DiagCode::BadArity);
        assert_eq!(parse("self(t, t)").unwrap_err().code, DiagCode::BadArity);
        assert_eq!(parse("self").unwrap_err().code, DiagCode::UnexpectedToken);
    }

    #[test]
    fn chain_dot_desugars_to_nested_calls() {
        let chained = parse("saw(220*t).lp(cutoff=800)").unwrap();
        let nested = parse("lp(saw(220*t), cutoff=800)").unwrap();
        assert_eq!(chained, nested);
    }

    #[test]
    fn calls_mix_positional_and_named_args() {
        let Expr::Call { args, .. } = parse("lp(saw(t), cutoff=800, q=1)").unwrap() else {
            panic!("expected Call")
        };
        assert_eq!(args.len(), 3);
        assert!(matches!(args[0], Arg::Pos(_)));
        assert!(matches!(args[1], Arg::Named(ref n, _) if n == "cutoff"));
        assert!(matches!(args[2], Arg::Named(ref n, _) if n == "q"));
    }

    #[test]
    fn located_refusals() {
        assert_eq!(parse("").unwrap_err().code, DiagCode::EmptyExpression);
        assert_eq!(parse("1+").unwrap_err().code, DiagCode::UnexpectedEof);
        assert_eq!(parse("(1").unwrap_err().code, DiagCode::UnclosedParen);
        assert_eq!(parse("1)").unwrap_err().code, DiagCode::UnbalancedParen);
        assert_eq!(parse("1 2").unwrap_err().code, DiagCode::UnexpectedToken);
        assert_eq!(parse("sin(1").unwrap_err().code, DiagCode::UnexpectedEof);
    }

    #[test]
    fn nesting_is_bounded_not_a_stack_overflow() {
        let deep = format!("{}1{}", "(".repeat(300), ")".repeat(300));
        assert_eq!(parse(&deep).unwrap_err().code, DiagCode::RecursionLimit);
        let flat = format!("1{}", "+1".repeat(MAX_TOKENS));
        assert_eq!(parse(&flat).unwrap_err().code, DiagCode::RecursionLimit);
    }
}
