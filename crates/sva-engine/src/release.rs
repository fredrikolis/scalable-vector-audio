// Concern: whether a sample before key-up can depend on `release`, and which terms it zeroes | Non-concern: binding its value (instantiate/) | IO: (Expr, Cx) -> the zeroed terms, or the offending term

use std::collections::HashSet;

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal, render_expr};

use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate::{Cx, Instances, Node, RELEASE, ScopeId};
use crate::loops::{Shift, shift_of};
use crate::typing::Typing;

/// How a subterm reads `release`, ordered by how much of it reaches the samples before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Use {
    /// It never reads `release`.
    Free,
    /// `release` itself, which only a crop's edge may hold.
    Bare,
    /// It reads `release`, and every sample before it is the one it would be without it.
    Causal,
    /// Zero at every sample before `release`; `never` where no caller binds one.
    Gated { never: bool },
}

/// The subterms a lowering replaces with zero: each is gated by a `release` nobody bound.
#[derive(Debug, Default)]
pub(crate) struct Never {
    zero: HashSet<(usize, ScopeId)>,
}

impl Never {
    pub(crate) fn holds(&self, e: &Expr, cx: Cx) -> bool {
        self.zero.contains(&key(e, cx))
    }
}

fn key(e: &Expr, cx: Cx) -> (usize, ScopeId) {
    (std::ptr::from_ref(e) as usize, cx.scope)
}

struct Offense {
    term: String,
    span: Option<ByteSpan>,
    why: &'static str,
}

/// A node that reads `release` may do so only where no sample before it changes: a crop's
/// end, a crop that opens there, a product with such a crop, and past reads of either.
pub(crate) fn check(
    inst: &Instances,
    typing: &Typing,
    path: &str,
    e: &Expr,
    cx: Cx,
) -> Result<(Use, Never), EngineError> {
    let mut walk = Walk {
        inst,
        typing,
        never: Never::default(),
    };
    match walk.term(e, cx) {
        Ok(Use::Bare) => Err(refusal(
            path,
            Offense {
                term: RELEASE.to_string(),
                span: None,
                why: "is the whole node, so every sample reads it",
            },
        )),
        Ok(found) => Ok((found, walk.never)),
        Err(offense) => Err(refusal(path, offense)),
    }
}

fn refusal(path: &str, offense: Offense) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.release_not_causal".to_string(),
        message: format!(
            "`{}` {}, so a sample before release would depend on when it comes.",
            offense.term, offense.why
        ),
        location: Located::at(path, offense.span),
        help: "read release only as a crop's end with no fall, as the start of a crop the \
               term lives in, or through a past read of such a term"
            .to_string(),
    })
}

struct Walk<'a, 'g> {
    inst: &'a Instances<'g>,
    typing: &'a Typing,
    never: Never,
}

impl Walk<'_, '_> {
    fn term(&mut self, e: &Expr, cx: Cx) -> Result<Use, Offense> {
        if matches!(e, Expr::Var(name) if name == RELEASE) {
            return Ok(Use::Bare);
        }
        if let Expr::Call { name, args, span } = e
            && self.inst.binds(cx.scope, name).is_some()
        {
            for a in args {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                if self.term(x, cx)? != Use::Free {
                    return Err(offense(
                        e,
                        Some(*span),
                        "reads a parameter at a time set by release",
                    ));
                }
            }
        }
        let inst = self.inst;
        if let Some(found) = inst.follow(e, cx, |e2, cx2| self.term(e2, cx2)) {
            return found;
        }
        let found = match inst.node(e, cx) {
            Node::Lit(_) | Node::Name(_) => Ok(Use::Free),
            Node::Bin(op, l, r) => self.binary(e, op, l, r, cx),
            Node::Call { name, args, span } => self.call(e, name, args, span, cx),
            Node::Read { path, arg, span } => self.read(e, path, arg, span, cx),
            Node::Own { arg, span } => match self.term(arg, cx)? {
                Use::Free => Ok(Use::Free),
                _ => Err(offense(
                    e,
                    Some(span),
                    "reads the node's own output at a time set by release",
                )),
            },
        };
        if let Ok(Use::Gated { never: true }) = found {
            self.never.zero.insert(key(e, cx));
        }
        found
    }

    fn binary(&mut self, e: &Expr, op: BinOp, l: &Expr, r: &Expr, cx: Cx) -> Result<Use, Offense> {
        let (left, right) = (self.term(l, cx)?, self.term(r, cx)?);
        let bare = || Err(offense(e, None, "does arithmetic on release itself"));
        match (op, left, right) {
            (BinOp::Mul, Use::Gated { never: a }, Use::Gated { never: b }) => {
                Ok(Use::Gated { never: a || b })
            }
            (BinOp::Mul, Use::Gated { never }, _) | (BinOp::Mul, _, Use::Gated { never }) => {
                Ok(Use::Gated { never })
            }
            (BinOp::Div, Use::Gated { never }, Use::Free | Use::Causal) => Ok(Use::Gated { never }),
            (_, Use::Bare, _) | (_, _, Use::Bare) => bare(),
            (BinOp::Div, _, Use::Gated { .. }) => Err(offense(
                e,
                None,
                "divides by a term that is zero before release",
            )),
            (BinOp::Add | BinOp::Sub, Use::Gated { never: a }, Use::Gated { never: b }) => {
                Ok(Use::Gated { never: a && b })
            }
            (_, Use::Free, Use::Free) => Ok(Use::Free),
            _ => Ok(Use::Causal),
        }
    }

    fn call(
        &mut self,
        e: &Expr,
        name: &str,
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
    ) -> Result<Use, Offense> {
        let positional: Vec<&Expr> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Pos(x) => Some(x),
                Arg::Named(..) => None,
            })
            .collect();
        if let ("crop", [of, start, end]) = (name, positional.as_slice()) {
            return self.crop(e, [of, start, end], args, span, cx);
        }
        let mut held = Use::Free;
        for a in args {
            let (Arg::Pos(x) | Arg::Named(_, x)) = a;
            held = match self.term(x, cx)? {
                Use::Bare if matches!(a, Arg::Named(key, _) if key == RELEASE && lands(name)) => {
                    Use::Causal
                }
                Use::Bare => {
                    return Err(offense(
                        e,
                        Some(span),
                        "hands release to a builtin as a number",
                    ));
                }
                Use::Free => held,
                _ => Use::Causal,
            };
        }
        if held != Use::Free && WHOLE_SIGNAL.contains(&name) {
            return Err(offense(
                e,
                Some(span),
                "transforms the whole signal, the part after release included",
            ));
        }
        Ok(held)
    }

    /// A crop opening at `release` is zero before it whatever it holds; one closing there is
    /// its operand until then, provided no fall shoulder reaches back from the edge.
    fn crop(
        &mut self,
        e: &Expr,
        [of, start, end]: [&Expr; 3],
        args: &[Arg],
        span: ByteSpan,
        cx: Cx,
    ) -> Result<Use, Offense> {
        let (start_use, end_use) = (self.term(start, cx)?, self.term(end, cx)?);
        for a in args {
            let Arg::Named(key, x) = a else {
                continue;
            };
            if self.term(x, cx)? != Use::Free {
                return Err(offense(e, Some(span), "shapes a shoulder by release"));
            }
            if key == "fall" && end_use == Use::Bare && !zero(x) {
                return Err(offense(
                    e,
                    Some(span),
                    "fades out into release, and the fade begins before it",
                ));
            }
        }
        match (start_use, end_use) {
            (Use::Bare, Use::Free | Use::Bare) => Ok(Use::Gated {
                never: never(self.inst, cx.scope),
            }),
            (Use::Free, Use::Bare) => match self.term(of, cx)? {
                Use::Bare => Err(offense(e, Some(span), "crops release itself")),
                _ => Ok(Use::Causal),
            },
            (Use::Free, Use::Free) => match self.term(of, cx)? {
                Use::Bare => Err(offense(e, Some(span), "crops release itself")),
                inner => Ok(inner),
            },
            _ => Err(offense(
                e,
                Some(span),
                "places a window edge by a term that reads release",
            )),
        }
    }

    /// A node that reads `release` may be heard at its own time or later, never earlier.
    fn read(
        &mut self,
        e: &Expr,
        path: &str,
        arg: &Expr,
        span: ByteSpan,
        cx: Cx,
    ) -> Result<Use, Offense> {
        if self.term(arg, cx)? != Use::Free {
            return Err(offense(
                e,
                Some(span),
                "reads a node at a time set by release",
            ));
        }
        let held = self.typing.release_use(path);
        if held == Use::Free {
            return Ok(Use::Free);
        }
        match shift_of(self.inst, arg, cx) {
            Some(Shift::Now) => Ok(held),
            Some(Shift::Secs(by)) if by >= 0.0 => Ok(held),
            Some(Shift::Steps(by)) if by >= 0.0 => Ok(held),
            _ => Err(offense(
                e,
                Some(span),
                "reads a node that uses release ahead of its own time",
            )),
        }
    }
}

/// Whether the `release` a scope reads comes to no number: unbound there, or handed down as
/// `release=release` from a scope where it is.
pub(crate) fn never(inst: &Instances, scope: ScopeId) -> bool {
    let mut scope = scope;
    loop {
        match inst.binds(scope, RELEASE) {
            None => return true,
            Some(bound) => match bound.expr {
                Expr::Var(name) if name == RELEASE => scope = bound.scope,
                _ => return false,
            },
        }
    }
}

/// Its `release=` changes no earlier sample.
fn lands(name: &str) -> bool {
    name == "chaigne_askenfelt"
}

/// A transform whose every output reads every input instant, the future included.
const WHOLE_SIGNAL: [&str; 4] = ["fourier", "ifourier", "stft", "istft"];

fn zero(e: &Expr) -> bool {
    matches!(e, Expr::Lit(Literal::Num(v)) if *v == 0.0)
}

fn offense(e: &Expr, span: Option<ByteSpan>, why: &'static str) -> Offense {
    Offense {
        term: render_expr(e),
        span,
        why,
    }
}
