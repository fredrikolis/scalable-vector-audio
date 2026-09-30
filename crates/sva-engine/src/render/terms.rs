// Concern: a stream's note sum, one node per term, each retired term kept as its identity | Non-concern: when a term's support ends (stream.rs), rebuilding nodes | IO: (Expr) -> Handle; (gone) -> ()

use std::sync::atomic::{AtomicU32, Ordering};

use sva_ast::{Address, Arg, BinOp, ByteSpan, Expr, Literal};
use sva_formula::Hash;

use crate::typing::{SumSlot, Typing};

/// The node a stream defines as the sum of its terms, for its expression to read as `@notes`.
pub const NOTES: &str = "notes";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u32);

impl Handle {
    pub(super) fn node(self) -> String {
        format!("{NOTES}#{}", self.0)
    }
}

/// Every stream draws from one count, so a handle names one term of one stream.
static HANDLES: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct Term {
    handle: Handle,
    expr: Expr,
    /// False once removed: it plays on, cut there, under no handle.
    addressed: bool,
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

    pub(super) fn nodes(&self) -> impl Iterator<Item = (String, Expr)> {
        self.live().map(|t| (t.handle.node(), t.expr.clone()))
    }

    pub(super) fn handles(&self) -> impl Iterator<Item = Handle> {
        self.live().map(|t| t.handle)
    }

    pub(super) fn added(&self, expr: Expr) -> (Terms, Handle) {
        let mut next = self.clone();
        let handle = Handle(HANDLES.fetch_add(1, Ordering::Relaxed));
        next.slots.push(Slot::Live(Term {
            handle,
            expr,
            addressed: true,
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
        term.expr = expr(term.landed);
        Some(next)
    }

    /// The term cropped at `at` seconds on the sum's own clock.
    pub(super) fn removed(&self, handle: Handle, at: f64) -> Option<Terms> {
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
            span: SPAN,
        };
        term.addressed = false;
        Some(next)
    }

    pub(super) fn sum(&self) -> Expr {
        self.live()
            .map(|t| Expr::Ref {
                path: t.handle.node(),
                arg: Box::new(Expr::Var("t".to_string())),
                binds: Vec::new(),
                address: Address::Time,
                span: SPAN,
            })
            .reduce(|sum, t| Expr::Bin(BinOp::Add, Box::new(sum), Box::new(t)))
            .unwrap_or(Expr::Lit(Literal::Num(0.0)))
    }

    /// `notes` named by its slots, a retired term by the identity it had.
    pub(super) fn name(&self, tys: &mut Typing) {
        let Some(notes) = tys.id(NOTES) else {
            return;
        };
        let slots = self.slots.iter().map(|s| match s {
            Slot::Live(t) => tys.id(&t.handle.node()).map(SumSlot::Node),
            Slot::Retired(h) => Some(SumSlot::Retired(*h)),
        });
        if let Some(slots) = slots.collect::<Option<Vec<_>>>() {
            tys.name_sum(notes, slots);
        }
    }

    /// Retires every term `gone` names, bar the last where all are, so what reads `notes`
    /// keeps its shape, each as the identity `named` gives it. True where any went.
    pub(super) fn prune(
        &mut self,
        gone: &dyn Fn(Handle) -> bool,
        named: &dyn Fn(Handle) -> Option<Hash>,
    ) -> bool {
        let live = self.live().count();
        let keep = match self.live().all(|t| gone(t.handle)) {
            true => self.live().last().map(|t| t.handle),
            false => None,
        };
        self.slots = std::mem::take(&mut self.slots)
            .into_iter()
            .filter_map(|slot| match slot {
                Slot::Live(t) if gone(t.handle) && Some(t.handle) != keep => {
                    named(t.handle).map(Slot::Retired)
                }
                other => Some(other),
            })
            .collect();
        self.live().count() != live
    }
}

const SPAN: ByteSpan = ByteSpan { start: 0, end: 0 };

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
