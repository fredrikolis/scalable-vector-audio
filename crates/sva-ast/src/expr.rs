// Concern: declares Expr, the source-free math-expression meaning | Non-concern: lexing, parsing, binding a ref path to a node (graph.rs) | IO: none

use crate::diag::ByteSpan;

pub const JOIN: &str = "join";

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl PartialEq for BinOp {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (BinOp::Add, BinOp::Add)
                | (BinOp::Sub, BinOp::Sub)
                | (BinOp::Mul, BinOp::Mul)
                | (BinOp::Div, BinOp::Div)
                | (BinOp::Mod, BinOp::Mod)
        )
    }
}
impl Eq for BinOp {}

/// `Bars` lives only until `Graph::resolve_bar_spans` rewrites it.
/// `Samples` counts the observation's own grid, which no parse can settle.
#[derive(Clone, Debug)]
pub enum Literal {
    Num(f64),
    Bars(f64),
    Samples(f64),
    Str(String),
}

impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Literal::Num(a), Literal::Num(b))
            | (Literal::Bars(a), Literal::Bars(b))
            | (Literal::Samples(a), Literal::Samples(b)) => a.to_bits() == b.to_bits(),
            (Literal::Str(a), Literal::Str(b)) => a == b,
            _ => false,
        }
    }
}
impl Eq for Literal {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Pos(Expr),
    Named(String, Expr),
}

/// Meaning only: which node a path names lives in `graph.rs`, so two structurally identical
/// expressions from different files compare equal, `span` excluded from `Eq`. `binds` holds
/// an invocation's named arguments in written order: `@lp-def(t, cutoff=800)`.
#[derive(Clone, Debug)]
pub enum Expr {
    Lit(Literal),
    Var(String),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call {
        name: String,
        args: Vec<Arg>,
        span: ByteSpan,
    },
    Ref {
        path: String,
        arg: Box<Expr>,
        binds: Vec<(String, Expr)>,
        span: ByteSpan,
    },
    SelfRef {
        arg: Box<Expr>,
        span: ByteSpan,
    },
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Expr::Lit(a), Expr::Lit(b)) => a == b,
            (Expr::Var(a), Expr::Var(b)) => a == b,
            (Expr::Bin(o1, l1, r1), Expr::Bin(o2, l2, r2)) => o1 == o2 && l1 == l2 && r1 == r2,
            (
                Expr::Call {
                    name: n1, args: a1, ..
                },
                Expr::Call {
                    name: n2, args: a2, ..
                },
            ) => n1 == n2 && a1 == a2,
            (
                Expr::Ref {
                    path: p1,
                    arg: a1,
                    binds: b1,
                    ..
                },
                Expr::Ref {
                    path: p2,
                    arg: a2,
                    binds: b2,
                    ..
                },
            ) => p1 == p2 && a1 == a2 && b1 == b2,
            (Expr::SelfRef { arg: a1, .. }, Expr::SelfRef { arg: a2, .. }) => a1 == a2,
            _ => false,
        }
    }
}
impl Eq for Expr {}

/// Which instance a ref's `binds` name: `Keep` moves a read, `Substitute` picks one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binds {
    Keep,
    Substitute,
}

/// One level down, in written order; `Substitute` makes a `Ref`'s `binds` children.
pub fn children(e: &Expr, binds: Binds) -> Vec<&Expr> {
    match e {
        Expr::Lit(_) | Expr::Var(_) => Vec::new(),
        Expr::Bin(_, l, r) => vec![l, r],
        Expr::SelfRef { arg, .. } => vec![arg],
        Expr::Ref { arg, binds: bs, .. } => {
            let mut out = vec![arg.as_ref()];
            if binds == Binds::Substitute {
                out.extend(bs.iter().map(|(_, v)| v));
            }
            out
        }
        Expr::Call { args, .. } => args
            .iter()
            .map(|a| {
                let (Arg::Pos(x) | Arg::Named(_, x)) = a;
                x
            })
            .collect(),
    }
}

/// Every child replaced by `f`'s answer, one level down; the caller owns its recursion.
pub fn map_children<E>(
    e: &Expr,
    binds: Binds,
    mut f: impl FnMut(&Expr) -> Result<Expr, E>,
) -> Result<Expr, E> {
    Ok(match e {
        Expr::Lit(_) | Expr::Var(_) => e.clone(),
        Expr::Bin(op, l, r) => Expr::Bin(*op, Box::new(f(l)?), Box::new(f(r)?)),
        Expr::SelfRef { arg, span } => Expr::SelfRef {
            arg: Box::new(f(arg)?),
            span: *span,
        },
        Expr::Ref {
            path,
            arg,
            binds: bs,
            span,
        } => {
            let arg = Box::new(f(arg)?);
            let mut rebuilt = Vec::with_capacity(bs.len());
            for (k, v) in bs {
                rebuilt.push((
                    k.clone(),
                    if binds == Binds::Substitute {
                        f(v)?
                    } else {
                        v.clone()
                    },
                ));
            }
            Expr::Ref {
                path: path.clone(),
                arg,
                binds: rebuilt,
                span: *span,
            }
        }
        Expr::Call { name, args, span } => {
            let mut rebuilt = Vec::with_capacity(args.len());
            for a in args {
                rebuilt.push(match a {
                    Arg::Pos(x) => Arg::Pos(f(x)?),
                    Arg::Named(k, x) => Arg::Named(k.clone(), f(x)?),
                });
            }
            Expr::Call {
                name: name.clone(),
                args: rebuilt,
                span: *span,
            }
        }
    })
}

pub(crate) fn map_children_ok(e: &Expr, binds: Binds, mut f: impl FnMut(&Expr) -> Expr) -> Expr {
    let mapped: Result<Expr, ()> = map_children(e, binds, |c| Ok(f(c)));
    mapped.expect("an infallible map never refuses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_equality_ignores_spans() {
        let a = Expr::Ref {
            path: "kick".to_string(),
            arg: Box::new(Expr::Var("t".to_string())),
            binds: Vec::new(),
            span: ByteSpan::new(0, 4),
        };
        let b = Expr::Ref {
            path: "kick".to_string(),
            arg: Box::new(Expr::Var("t".to_string())),
            binds: Vec::new(),
            span: ByteSpan::new(99, 200),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn float_literals_compare_by_bit_pattern() {
        assert_eq!(Literal::Num(0.0), Literal::Num(0.0));
        assert_ne!(Literal::Num(0.0), Literal::Num(-0.0), "-0.0 != 0.0 exactly");
        assert_eq!(Literal::Num(f64::NAN), Literal::Num(f64::NAN));
    }
}
