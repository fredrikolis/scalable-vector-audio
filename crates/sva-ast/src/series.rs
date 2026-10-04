// Concern: writes out a `sum` whose index a node's binding reads, one term per index | Non-concern: a sum no binding reads, which stays a series | IO: (&Expr) -> Expr or a Diag

use crate::diag::{ByteSpan, Diag, DiagCode};
use crate::expr::{Arg, BinOp, Binds, Expr, Literal, SERIES, map_children};
use crate::ingest::occurs_free;
use crate::vocabulary::is_builtin;

/// The most terms one body's sums write out, nesting multiplied, a bound on the text a
/// parse holds.
const MAX_WRITTEN_TERMS: i64 = 4096;

/// One argument tuple per index is one node per index, which no series stands for. A sum
/// whose bounds name parameters waits for the instance that gives them numbers.
pub fn written_out(e: &Expr) -> Result<Expr, Diag> {
    let mut left = MAX_WRITTEN_TERMS;
    rewritten(e, &mut left, None)
}

/// `written_out` for one instance, each name a bound reads the number `named` gives it.
pub fn written_out_with(e: &Expr, named: &dyn Fn(&str) -> Option<f64>) -> Result<Expr, Diag> {
    let mut left = MAX_WRITTEN_TERMS;
    rewritten(e, &mut left, Some(named))
}

pub fn waits_for_instance(e: &Expr) -> bool {
    !bounds_waiting(e).is_empty()
}

/// Each bound of a sum `written_out` left for an instance to write.
pub fn bounds_waiting(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::Call { name, args, .. } if name == SERIES => match args.as_slice() {
            [
                Arg::Pos(Expr::Var(index)),
                Arg::Pos(lo),
                Arg::Pos(hi),
                Arg::Pos(term),
            ] if binds(term, index) => {
                vec![lo, hi]
            }
            _ => args.iter().flat_map(|a| bounds_waiting(arg(a))).collect(),
        },
        _ => crate::expr::children(e, Binds::Substitute)
            .into_iter()
            .flat_map(bounds_waiting)
            .collect(),
    }
}

type Named<'a> = Option<&'a dyn Fn(&str) -> Option<f64>>;

/// Top down, so an inner sum's bounds are the outer index's numbers before it is read.
fn rewritten(e: &Expr, left: &mut i64, named: Named) -> Result<Expr, Diag> {
    let Expr::Call { name, args, span } = e else {
        return map_children(e, Binds::Substitute, |c| rewritten(c, left, named));
    };
    match args.as_slice() {
        [
            Arg::Pos(Expr::Var(index)),
            Arg::Pos(lo),
            Arg::Pos(hi),
            Arg::Pos(term),
        ] if name == SERIES && binds(term, index) => {
            let none = |_: &str| None;
            let resolve = named.unwrap_or(&none);
            if named.is_none()
                && [lo, hi].iter().any(|b| whole(b, resolve).is_none())
                && [lo, hi]
                    .iter()
                    .all(|b| names_only(b) && !occurs_free(b, index))
            {
                return Ok(e.clone());
            }
            let bound = |at: &Expr| whole(at, resolve).ok_or_else(|| unwritten(index, *span, None));
            let (lo, hi) = (bound(lo)?, bound(hi)?);
            let count = hi.saturating_sub(lo).saturating_add(1).max(0);
            *left -= count.min(MAX_WRITTEN_TERMS + 1);
            if *left < 0 {
                return Err(unwritten(index, *span, Some(count)));
            }
            let mut terms = Vec::new();
            for k in lo..=hi {
                terms.push(rewritten(&substitute(term, index, k), left, named)?);
            }
            let summed = terms
                .into_iter()
                .reduce(|a, b| Expr::Bin(BinOp::Add, Box::new(a), Box::new(b)));
            Ok(summed.unwrap_or(Expr::Lit(Literal::Num(0.0))))
        }
        _ => map_children(e, Binds::Substitute, |c| rewritten(c, left, named)),
    }
}

/// Arithmetic over literals and names, which an instance folds.
fn names_only(e: &Expr) -> bool {
    match e {
        Expr::Lit(Literal::Num(_)) => true,
        Expr::Var(name) => !crate::vocabulary::is_reserved(name),
        Expr::Bin(op, l, r) => *op != BinOp::Mod && names_only(l) && names_only(r),
        _ => false,
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

fn whole(e: &Expr, named: &dyn Fn(&str) -> Option<f64>) -> Option<i64> {
    let v = folded(e, named)?;
    (v.is_finite() && v.fract() == 0.0 && v.abs() < 2f64.powi(53)).then_some(v as i64)
}

/// Arithmetic over literals, each name the number `named` gives it.
pub fn folded(e: &Expr, named: &dyn Fn(&str) -> Option<f64>) -> Option<f64> {
    match e {
        Expr::Lit(Literal::Num(n)) => Some(*n),
        Expr::Var(name) => named(name),
        Expr::Bin(op, l, r) => {
            let (a, b) = (folded(l, named)?, folded(r, named)?);
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
        None => {
            "so its bounds must fold to whole numbers, from literals and parameters".to_string()
        }
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
