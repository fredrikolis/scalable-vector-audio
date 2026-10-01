// Concern: a stream's note sum, one node per term, those it retired folded into one slot | Non-concern: when a term's support ends (stream.rs), rebuilding nodes | IO: (Expr) -> Handle; (gone) -> ()

use std::sync::atomic::{AtomicU32, Ordering};

use sva_ast::{Address, Arg, BinOp, ByteSpan, Expr, Literal};
use sva_samples::Extent;

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

#[derive(Clone, Default)]
pub(super) struct Terms {
    terms: Vec<Term>,
    /// The hull of the supports of every term it retired.
    retired: Option<Extent>,
}

impl Terms {
    pub(super) fn count(&self) -> usize {
        self.terms.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub(super) fn exprs(&self) -> impl Iterator<Item = &Expr> {
        self.terms.iter().map(|t| &t.expr)
    }

    pub(super) fn nodes(&self) -> impl Iterator<Item = (String, Expr)> {
        self.terms.iter().map(|t| (t.handle.node(), t.expr.clone()))
    }

    pub(super) fn handles(&self) -> impl Iterator<Item = Handle> {
        self.terms.iter().map(|t| t.handle)
    }

    pub(super) fn added(&self, expr: Expr) -> (Terms, Handle) {
        let mut next = self.clone();
        let handle = Handle(HANDLES.fetch_add(1, Ordering::Relaxed));
        next.terms.push(Term {
            handle,
            expr,
            addressed: true,
            landed: 0,
        });
        (next, handle)
    }

    fn held(&mut self, handle: Handle) -> Option<&mut Term> {
        self.terms.iter_mut().find(|t| t.holds(handle))
    }

    pub(super) fn landed(&self, handle: Handle) -> Option<i64> {
        Some(self.terms.iter().find(|t| t.holds(handle))?.landed)
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
        self.terms
            .iter()
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

    /// `notes` named by its terms, and by the supports of those it retired.
    pub(super) fn name(&self, tys: &mut Typing) {
        let Some(notes) = tys.id(NOTES) else {
            return;
        };
        let live = self.terms.iter();
        let live = live.map(|t| tys.id(&t.handle.node()).map(SumSlot::Node));
        let retired = self.retired.map(|support| Some(SumSlot::Retired(support)));
        if let Some(slots) = live.chain(retired).collect::<Option<Vec<_>>>() {
            tys.name_sum(notes, slots);
        }
    }

    /// Drops every term `gone` names, its support, as `support` gives it, kept in the hull of
    /// those retired. True where any went.
    pub(super) fn prune(
        &mut self,
        gone: &dyn Fn(Handle) -> bool,
        support: &dyn Fn(Handle) -> Option<Extent>,
    ) -> bool {
        let live = self.terms.len();
        let (went, kept): (Vec<Term>, _) = std::mem::take(&mut self.terms)
            .into_iter()
            .partition(|t| gone(t.handle));
        self.terms = kept;
        for ended in went.iter().filter_map(|t| support(t.handle)) {
            self.retired = Some(self.retired.map_or(ended, |hull| hull.hull(ended)));
        }
        self.terms.len() != live
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

#[cfg(test)]
mod tests {
    use super::*;

    /// However many terms came and went, `notes` is named by those sounding and one slot for
    /// the rest: the hull of their supports.
    #[test]
    fn notes_is_named_by_its_sounding_terms_and_one_retired_slot() {
        let mut terms = Terms::default();
        let mut last = None;
        for k in 0..1_000 {
            let (next, handle) = terms.added(Expr::Lit(Literal::Num(1.0)));
            terms = next;
            let ended = |h: Handle| Some(h) == last;
            assert_eq!(terms.prune(&ended, &|_| Some(Extent::new(k, k + 1))), k > 0);
            last = Some(handle);
        }
        let dir = std::env::temp_dir().join(format!("sva-terms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("one"), "1\n").expect("a node file");
        let mut graph = sva_ast::parse_composition(&dir).expect("a composition");
        for (name, body) in std::iter::once((NOTES.to_string(), terms.sum())).chain(terms.nodes()) {
            assert!(graph.define(&name, body));
        }
        let mut tys = crate::types(&graph, NOTES).expect("typed");
        terms.name(&mut tys);
        let (notes, sounding) = (tys.id(NOTES), last.and_then(|h| tys.id(&h.node())));
        let slots = notes.and_then(|notes| tys.sum_slots(notes)).expect("named");
        let sounding = SumSlot::Node(sounding.expect("the last term"));
        assert_eq!(slots, [sounding, SumSlot::Retired(Extent::new(1, 1_000))]);
    }
}
