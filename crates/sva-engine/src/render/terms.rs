// Concern: a stream's note sum, one term per handle, each term that ended kept only as its identity | Non-concern: when a value ends (table/), rebuilding nodes | IO: (Expr) -> Handle; (ended) -> ()

use sva_ast::{Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::{Hash, NodeId};

use crate::instantiate::Instances;
use crate::typing::{SumSlot, Typing, Value};

/// The node a stream defines as the sum of its terms, for its expression to read as `@notes`.
pub const NOTES: &str = "notes";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u32);

#[derive(Clone)]
struct Term {
    handle: Handle,
    expr: Expr,
    /// False once removed: it plays on, cut there, under no handle.
    addressed: bool,
    leaf: Option<NodeId>,
    ends: Option<NodeId>,
    /// The sample a removal cut it at.
    cut: Option<i64>,
    /// The sample its add landed at.
    landed: i64,
}

impl Term {
    fn holds(&self, handle: Handle) -> bool {
        self.addressed && self.handle == handle
    }
}

#[derive(Clone)]
enum Slot {
    Live(Term),
    Retired(Hash),
}

#[derive(Clone, Default)]
pub(super) struct Terms {
    next: u32,
    slots: Vec<Slot>,
}

impl Terms {
    fn live(&self) -> impl Iterator<Item = &Term> {
        self.slots.iter().filter_map(|s| match s {
            Slot::Live(t) => Some(t),
            Slot::Retired(_) => None,
        })
    }

    fn live_mut(&mut self) -> impl Iterator<Item = &mut Term> {
        self.slots.iter_mut().filter_map(|s| match s {
            Slot::Live(t) => Some(t),
            Slot::Retired(_) => None,
        })
    }

    pub(super) fn count(&self) -> usize {
        self.live().count()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.live().next().is_none()
    }

    pub(super) fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.live().map(|t| &t.expr)
    }

    pub(super) fn added(&self, expr: Expr) -> (Terms, Handle) {
        let mut next = self.clone();
        let handle = Handle(next.next);
        next.next += 1;
        next.slots.push(Slot::Live(Term {
            handle,
            expr,
            addressed: true,
            leaf: None,
            ends: None,
            cut: None,
            landed: 0,
        }));
        (next, handle)
    }

    fn held(&mut self, handle: Handle) -> Option<&mut Term> {
        self.live_mut().find(|t| t.holds(handle))
    }

    pub(super) fn landed(&self, handle: Handle) -> Option<i64> {
        Some(self.live().find(|t| t.holds(handle))?.landed)
    }

    pub(super) fn land(&mut self, handle: Handle, at: i64) {
        if let Some(term) = self.held(handle) {
            term.landed = at;
        }
    }

    /// `None` where the stream no longer holds `handle`; `expr` of the sample its add landed at.
    pub(super) fn replaced(&self, handle: Handle, expr: impl FnOnce(i64) -> Expr) -> Option<Terms> {
        let mut next = self.clone();
        let term = next.held(handle)?;
        (term.expr, term.leaf, term.ends) = (expr(term.landed), None, None);
        Some(next)
    }

    /// The term cut at sample `cut` of the sum's own clock.
    pub(super) fn removed(&self, handle: Handle, cut: i64, rate: u32) -> Option<Terms> {
        let at = cut as f64 / f64::from(rate);
        let mut next = self.clone();
        let term = next.held(handle)?;
        let never = Expr::Bin(
            BinOp::Sub,
            Box::new(Expr::Lit(Literal::Num(0.0))),
            Box::new(Expr::Var("inf".to_string())),
        );
        term.expr = Expr::Call {
            name: "crop".to_string(),
            args: vec![
                Arg::Pos(term.expr.clone()),
                Arg::Pos(never),
                Arg::Pos(Expr::Lit(Literal::Num(at))),
            ],
            span: ByteSpan { start: 0, end: 0 },
        };
        (term.addressed, term.leaf, term.ends, term.cut) = (false, None, None, Some(cut));
        Some(next)
    }

    pub(super) fn sum(&self) -> Expr {
        self.exprs()
            .cloned()
            .reduce(|sum, t| Expr::Bin(BinOp::Add, Box::new(sum), Box::new(t)))
            .unwrap_or(Expr::Lit(Literal::Num(0.0)))
    }

    /// Each term's own node down the lowered sum's spine, none where the lowering merged it;
    /// where each has one, `notes` is named by its slots.
    pub(super) fn typed(&mut self, inst: &Instances, tys: &mut Typing) {
        let Some(notes) = inst.instance_of(NOTES).ok().and_then(|path| tys.id(&path)) else {
            return;
        };
        let mut at = match tys.value(notes) {
            Value::Read {
                source,
                at: crate::typing::When::At(time),
                ..
            } if *time == crate::time::Affine::NOW => Some(*source),
            _ => Some(notes),
        };
        let count = self.live().count();
        let mut leaves = vec![None; count];
        for k in (0..count).rev() {
            let Some(id) = at else {
                break;
            };
            match (k, tys.value(id)) {
                (0, _) => (leaves[0], at) = (Some(id), None),
                (_, Value::Op { name, args }) if name == "+" && args.len() == 2 => {
                    (leaves[k], at) = (Some(args[1]), Some(args[0]));
                }
                _ => at = None,
            }
        }
        for (term, leaf) in self.live_mut().zip(leaves) {
            term.leaf = leaf;
            term.ends = leaf.map(|id| match tys.value(id) {
                Value::Read { source, .. } => *source,
                _ => id,
            });
        }
        let slots = self
            .slots
            .iter()
            .map(|s| match s {
                Slot::Live(t) => t.leaf.map(SumSlot::Node),
                Slot::Retired(h) => Some(SumSlot::Retired(*h)),
            })
            .collect::<Option<Vec<_>>>();
        if let Some(slots) = slots {
            tys.name_sum(notes, slots);
        }
    }

    /// Retires every term whose node has ended or whose cut `now` has passed, bar the last
    /// where all have, so what reads `notes` keeps its shape and its identity; one with no node
    /// of its own just goes. True where any went.
    pub(super) fn prune(
        &mut self,
        now: i64,
        ended: &dyn Fn(NodeId) -> bool,
        named: &dyn Fn(NodeId) -> Option<Hash>,
    ) -> bool {
        let live = self.live().count();
        let gone = |t: &Term| t.cut.is_some_and(|cut| cut <= now) || t.ends.is_some_and(ended);
        let keep = match self.live().all(gone) {
            true => self.live().last().map(|t| t.handle),
            false => None,
        };
        self.slots = std::mem::take(&mut self.slots)
            .into_iter()
            .filter_map(|slot| match slot {
                Slot::Live(t) if gone(&t) && Some(t.handle) != keep => {
                    t.leaf.and_then(named).map(Slot::Retired)
                }
                other => Some(other),
            })
            .collect();
        self.live().count() != live
    }
}

/// `expr` with its sample 0 at sample `at` of the stream: every `t` in it read `at` earlier.
pub(super) fn placed(expr: &Expr, at: i64) -> Expr {
    let moved = |e: &Expr| Box::new(placed(e, at));
    match expr {
        Expr::Var(name) if name == "t" && at != 0 => Expr::Bin(
            BinOp::Sub,
            Box::new(expr.clone()),
            Box::new(Expr::Lit(Literal::Samples(at as f64))),
        ),
        Expr::Lit(_) | Expr::Var(_) => expr.clone(),
        Expr::Bin(op, l, r) => Expr::Bin(*op, moved(l), moved(r)),
        Expr::Call { name, args, span } => Expr::Call {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| match arg {
                    Arg::Pos(e) => Arg::Pos(placed(e, at)),
                    Arg::Named(n, e) => Arg::Named(n.clone(), placed(e, at)),
                })
                .collect(),
            span: *span,
        },
        Expr::Ref {
            path,
            arg,
            binds,
            address,
            span,
        } => Expr::Ref {
            path: path.clone(),
            arg: moved(arg),
            binds: binds
                .iter()
                .map(|(n, e)| (n.clone(), placed(e, at)))
                .collect(),
            address: *address,
            span: *span,
        },
        Expr::SelfRef { arg, address, span } => Expr::SelfRef {
            arg: moved(arg),
            address: *address,
            span: *span,
        },
        Expr::Indexed { name, arg, span } => Expr::Indexed {
            name: name.clone(),
            arg: moved(arg),
            span: *span,
        },
    }
}
