// Concern: drops a stream's summands whose node has ended from its expression | Non-concern: when a node ends (drive/), rebuilding nodes | IO: (Instances, root) -> Terms; (Expr, ended) -> Expr

use sva_ast::{BinOp, Binds, Expr, map_children};
use sva_formula::NodeId;

use crate::instantiate::{Cx, Instances, Node};
use crate::typing::Typing;

/// Each summand of a stream's target that is one read, and the node it reads.
#[derive(Default)]
pub(super) struct Terms {
    reads: Vec<(Expr, NodeId)>,
}

impl Terms {
    pub(super) fn of(inst: &Instances, tys: &Typing, root: &str) -> Terms {
        let (body, cx) = inst.at(root).expect("the stream's root is an instance");
        let mut terms = Terms::default();
        terms.walk(inst, tys, body, cx);
        terms
    }

    fn walk(&mut self, inst: &Instances, tys: &Typing, e: &Expr, cx: Cx) {
        if matches!(e, Expr::Bin(BinOp::Add, ..)) {
            for term in summands(e) {
                if let Node::Read { path, .. } = inst.node(term, cx)
                    && let Some(id) = tys.id(path)
                {
                    self.reads.push((term.clone(), id));
                }
                self.below(inst, tys, term, cx);
            }
            return;
        }
        self.below(inst, tys, e, cx);
    }

    fn below(&mut self, inst: &Instances, tys: &Typing, e: &Expr, cx: Cx) {
        for child in sva_ast::children(e, Binds::Substitute) {
            self.walk(inst, tys, child, cx);
        }
    }

    /// `e` with every summand whose node has ended dropped, and `None` where none has. A sum
    /// keeps its last summand where all have ended, so whatever reads it keeps its shape.
    pub(super) fn pruned(&self, e: &Expr, ended: &dyn Fn(NodeId) -> bool) -> Option<Expr> {
        let dead = |term: &Expr| {
            self.reads
                .iter()
                .any(|(read, id)| read == term && ended(*id))
        };
        let mut dropped = false;
        let out = self.rewrite(e, &dead, &mut dropped);
        dropped.then_some(out)
    }

    fn rewrite(&self, e: &Expr, dead: &dyn Fn(&Expr) -> bool, dropped: &mut bool) -> Expr {
        if !matches!(e, Expr::Bin(BinOp::Add, ..)) {
            let mapped: Result<Expr, ()> =
                map_children(e, Binds::Substitute, |c| Ok(self.rewrite(c, dead, dropped)));
            return mapped.expect("an infallible map never refuses");
        }
        let terms = summands(e);
        let mut live: Vec<&Expr> = terms.iter().copied().filter(|t| !dead(t)).collect();
        if live.is_empty() {
            live.extend(terms.last());
        }
        *dropped |= live.len() < terms.len();
        live.into_iter()
            .map(|t| self.rewrite(t, dead, dropped))
            .reduce(|sum, t| Expr::Bin(BinOp::Add, Box::new(sum), Box::new(t)))
            .expect("a sum has a summand")
    }
}

fn summands(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Bin(BinOp::Add, l, r) => {
            let mut out = summands(l);
            out.extend(summands(r));
            out
        }
        other => vec![other],
    }
}
