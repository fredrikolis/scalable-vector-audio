// Concern: writes out a `sum` whose index a node's binding reads, one term per index | Non-concern: a sum no binding reads, which stays a series | IO: (&Expr) -> Expr or a Diag

use crate::diag::{ByteSpan, Diag, DiagCode};
use crate::expr::{Arg, BinOp, Binds, Expr, Literal, SERIES, map_children};
use crate::ingest::occurs_free;
use crate::vocabulary::is_builtin;

/// The most terms one body's sums write out, nesting multiplied, a bound on the text a
/// parse holds.
const MAX_WRITTEN_TERMS: i64 = 4096;

/// One argument tuple per index is one node per index, which no series stands for.
pub fn written_out(e: &Expr) -> Result<Expr, Diag> {
    let mut left = MAX_WRITTEN_TERMS;
    rewritten(e, &mut left)
}

/// Top down, so an inner sum's bounds are the outer index's numbers before it is read.
fn rewritten(e: &Expr, left: &mut i64) -> Result<Expr, Diag> {
    let Expr::Call { name, args, span } = e else {
        return map_children(e, Binds::Substitute, |c| rewritten(c, left));
    };
    match args.as_slice() {
        [
            Arg::Pos(Expr::Var(index)),
            Arg::Pos(lo),
            Arg::Pos(hi),
            Arg::Pos(term),
        ] if name == SERIES && binds(term, index) => {
            let bound = |at: &Expr| whole(at).ok_or_else(|| unwritten(index, *span, None));
            let (lo, hi) = (bound(lo)?, bound(hi)?);
            let count = hi.saturating_sub(lo).saturating_add(1).max(0);
            *left -= count.min(MAX_WRITTEN_TERMS + 1);
            if *left < 0 {
                return Err(unwritten(index, *span, Some(count)));
            }
            let mut terms = Vec::new();
            for k in lo..=hi {
                terms.push(rewritten(&substitute(term, index, k), left)?);
            }
            let summed = terms
                .into_iter()
                .reduce(|a, b| Expr::Bin(BinOp::Add, Box::new(a), Box::new(b)));
            Ok(summed.unwrap_or(Expr::Lit(Literal::Num(0.0))))
        }
        _ => map_children(e, Binds::Substitute, |c| rewritten(c, left)),
    }
}

/// Whether `index` is read free inside a ref's binding or a node call's named argument, or
/// bounds an inner sum that is written out.
fn binds(e: &Expr, index: &str) -> bool {
    match e {
        Expr::Call { name, args, .. } if name == SERIES => match args.as_slice() {
            [Arg::Pos(Expr::Var(inner)), lo, hi, term] => {
                let [lo, hi, term] = [lo, hi, term].map(|(Arg::Pos(x) | Arg::Named(_, x))| x);
                let written = binds(term, inner);
                let bounds_it = occurs_free(lo, index) || occurs_free(hi, index);
                (written && bounds_it)
                    || binds(lo, index)
                    || binds(hi, index)
                    || (inner != index && binds(term, index))
            }
            _ => args.iter().any(|a| binds(arg(a), index)),
        },
        Expr::Call { name, args, .. } if !is_builtin(name) => args.iter().any(|a| match a {
            Arg::Named(_, x) => occurs_free(x, index),
            Arg::Pos(x) => binds(x, index),
        }),
        Expr::Ref { arg, binds: bs, .. } => {
            binds(arg, index) || bs.iter().any(|(_, x)| occurs_free(x, index))
        }
        _ => crate::expr::children(e, Binds::Substitute)
            .into_iter()
            .any(|c| binds(c, index)),
    }
}

fn arg(a: &Arg) -> &Expr {
    let (Arg::Pos(x) | Arg::Named(_, x)) = a;
    x
}

/// `e` with each free `index` the number `k`; a `sum` binding the same name keeps its term.
fn substitute(e: &Expr, index: &str, k: i64) -> Expr {
    match e {
        Expr::Var(name) if name == index => Expr::Lit(Literal::Num(k as f64)),
        Expr::Call { name, args, span } if name == SERIES => match args.as_slice() {
            [Arg::Pos(Expr::Var(inner)), lo, hi, term] if inner == index => Expr::Call {
                name: name.clone(),
                args: vec![
                    Arg::Pos(Expr::Var(inner.clone())),
                    Arg::Pos(substitute(arg(lo), index, k)),
                    Arg::Pos(substitute(arg(hi), index, k)),
                    term.clone(),
                ],
                span: *span,
            },
            _ => crate::expr::map_children_ok(e, Binds::Substitute, |c| substitute(c, index, k)),
        },
        _ => crate::expr::map_children_ok(e, Binds::Substitute, |c| substitute(c, index, k)),
    }
}

fn whole(e: &Expr) -> Option<i64> {
    let v = folded(e)?;
    (v.is_finite() && v.fract() == 0.0 && v.abs() < 2f64.powi(53)).then_some(v as i64)
}

fn folded(e: &Expr) -> Option<f64> {
    match e {
        Expr::Lit(Literal::Num(n)) => Some(*n),
        Expr::Bin(op, l, r) => {
            let (a, b) = (folded(l)?, folded(r)?);
            match op {
                BinOp::Add => Some(a + b),
                BinOp::Sub => Some(a - b),
                BinOp::Mul => Some(a * b),
                BinOp::Div => Some(a / b),
                BinOp::Mod => None,
            }
        }
        _ => None,
    }
}

fn unwritten(index: &str, span: ByteSpan, terms: Option<i64>) -> Diag {
    let why = match terms {
        None => "so its bounds must be whole-number literals".to_string(),
        Some(n) => {
            format!("and {n} more terms is past the {MAX_WRITTEN_TERMS} one body writes out")
        }
    };
    Diag::new(
        DiagCode::UnwrittenSeries,
        span,
        format!(
            "`{SERIES}`'s index `{index}` reaches a node's binding, so the sum is written out one \
             node per index, {why}"
        ),
    )
}
