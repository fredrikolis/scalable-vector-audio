// Concern: a stream's note sum, one term per handle, less the terms that ended | Non-concern: when a node ends (drive/), rebuilding nodes | IO: (Expr) -> Handle; (ended) -> ()

use sva_ast::{BinOp, Expr, Literal};
use sva_formula::NodeId;

use crate::instantiate::{Instances, Node};
use crate::typing::Typing;

/// The node a stream defines as the sum of its terms, for its expression to read as `@notes`.
pub const NOTES: &str = "notes";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u32);

#[derive(Clone)]
struct Term {
    handle: Handle,
    expr: Expr,
    node: Option<NodeId>,
}

#[derive(Clone, Default)]
pub(super) struct Terms {
    next: u32,
    held: Vec<Term>,
}

impl Terms {
    pub(super) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    pub(super) fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.held.iter().map(|t| &t.expr)
    }

    pub(super) fn added(&self, expr: Expr) -> (Terms, Handle) {
        let mut next = self.clone();
        let handle = Handle(next.next);
        next.next += 1;
        next.held.push(Term {
            handle,
            expr,
            node: None,
        });
        (next, handle)
    }

    /// `None` where the stream no longer holds `handle`.
    pub(super) fn replaced(&self, handle: Handle, expr: Expr) -> Option<Terms> {
        let mut next = self.clone();
        let term = next.held.iter_mut().find(|t| t.handle == handle)?;
        *term = Term {
            handle,
            expr,
            node: None,
        };
        Some(next)
    }

    pub(super) fn removed(&self, handle: Handle) -> Option<Terms> {
        let mut next = self.clone();
        let at = next.held.iter().position(|t| t.handle == handle)?;
        next.held.remove(at);
        Some(next)
    }

    pub(super) fn sum(&self) -> Expr {
        self.exprs()
            .cloned()
            .reduce(|sum, t| Expr::Bin(BinOp::Add, Box::new(sum), Box::new(t)))
            .unwrap_or(Expr::Lit(Literal::Num(0.0)))
    }

    pub(super) fn typed(&mut self, inst: &Instances, tys: &Typing) {
        let Some((body, cx)) = inst.instance_of(NOTES).ok().and_then(|path| inst.at(&path)) else {
            return;
        };
        let mut rest = body;
        for term in self.held.iter_mut().skip(1).rev() {
            let Expr::Bin(BinOp::Add, sum, last) = rest else {
                unreachable!("`notes` is the sum of its terms")
            };
            term.node = read(inst, tys, last, cx);
            rest = sum;
        }
        if let Some(first) = self.held.first_mut() {
            first.node = read(inst, tys, rest, cx);
        }
    }

    /// Drops every term whose node has ended, bar the last where all have, so whatever
    /// reads `notes` keeps its shape.
    pub(super) fn prune(&mut self, ended: &dyn Fn(NodeId) -> bool) {
        let last = self.held.last().map(|t| t.handle);
        let gone = |t: &Term| t.node.is_some_and(ended);
        if self.held.iter().all(gone) {
            self.held.retain(|t| Some(t.handle) == last);
        } else {
            self.held.retain(|t| !gone(t));
        }
    }
}

fn read(inst: &Instances, tys: &Typing, term: &Expr, cx: crate::instantiate::Cx) -> Option<NodeId> {
    match inst.node(term, cx) {
        Node::Read { path, .. } => tys.id(path),
        _ => None,
    }
}
