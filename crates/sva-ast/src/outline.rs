// Concern: the parse tree of one expression text, each node beside the bytes it was written in | Non-concern: parsing it (parser.rs), what any node means (sva-engine) | IO: (&str) -> Outline or a Diag

use crate::diag::{ByteSpan, Diag};
use crate::expr::{Arg, BinOp, Expr, Literal};
use crate::parser::{Mark, parse_marked};

/// One node of the tree `parse_expr` builds. `written` is false for the two nodes the parser
/// supplies itself: the `0` of a prefix minus and the `t` of a bare `@ref`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub span: ByteSpan,
    pub written: bool,
    pub form: Form,
}

/// `at` is the token naming the node: an operator, a callee, a ref's `@path`, or `self`.
#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    Literal(Literal),
    Name(String),
    Operator {
        op: BinOp,
        at: ByteSpan,
        left: Box<Outline>,
        right: Box<Outline>,
    },
    Call {
        name: String,
        at: ByteSpan,
        args: Vec<Argument>,
    },
    Ref {
        path: String,
        at: ByteSpan,
        arg: Box<Outline>,
        binds: Vec<Argument>,
    },
    SelfRef {
        at: ByteSpan,
        arg: Box<Outline>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Argument {
    Positional(Outline),
    Named {
        name: String,
        at: ByteSpan,
        value: Outline,
    },
}

pub fn outline(src: &str) -> Result<Outline, Diag> {
    let (expr, marks) = parse_marked(src)?;
    let mut marks = marks.into_iter();
    let tree = walk(&expr, &mut marks);
    debug_assert!(marks.next().is_none(), "every mark belongs to one node");
    Ok(tree)
}

fn walk(e: &Expr, marks: &mut impl Iterator<Item = Mark>) -> Outline {
    let form = match e {
        Expr::Lit(l) => Form::Literal(l.clone()),
        Expr::Var(name) => Form::Name(name.clone()),
        Expr::Bin(op, l, r) => {
            let (left, right) = (walk(l, marks), walk(r, marks));
            return node(marks, |at| Form::Operator {
                op: *op,
                at,
                left: Box::new(left),
                right: Box::new(right),
            });
        }
        Expr::Call { name, args, .. } => {
            let args = args.iter().map(|a| argument(a, marks)).collect();
            return node(marks, |at| Form::Call {
                name: name.clone(),
                at,
                args,
            });
        }
        Expr::Ref {
            path, arg, binds, ..
        } => {
            let arg = walk(arg, marks);
            let binds = binds.iter().map(|(k, v)| named(k, v, marks)).collect();
            return node(marks, |at| Form::Ref {
                path: path.clone(),
                at,
                arg: Box::new(arg),
                binds,
            });
        }
        Expr::SelfRef { arg, .. } => {
            let arg = walk(arg, marks);
            return node(marks, |at| Form::SelfRef {
                at,
                arg: Box::new(arg),
            });
        }
    };
    node(marks, |_| form)
}

fn argument(a: &Arg, marks: &mut impl Iterator<Item = Mark>) -> Argument {
    match a {
        Arg::Pos(x) => Argument::Positional(walk(x, marks)),
        Arg::Named(k, v) => named(k, v, marks),
    }
}

fn named(key: &str, value: &Expr, marks: &mut impl Iterator<Item = Mark>) -> Argument {
    let Some(Mark::Key(at)) = marks.next() else {
        unreachable!("the parser marks a key before its value")
    };
    Argument::Named {
        name: key.to_string(),
        at,
        value: walk(value, marks),
    }
}

fn node(marks: &mut impl Iterator<Item = Mark>, form: impl FnOnce(ByteSpan) -> Form) -> Outline {
    let Some(Mark::Node {
        span,
        head,
        written,
    }) = marks.next()
    else {
        unreachable!("the parser marks every node after its operands")
    };
    Outline {
        span,
        written,
        form: form(head.unwrap_or(span)),
    }
}
