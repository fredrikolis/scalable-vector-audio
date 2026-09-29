// Concern: what an index expression denotes: an integer, and the rounded line plus count it sums to | Non-concern: reading a value there (render/table/program.rs) | IO: (&Expr, Cx) -> bool, Index

use sva_ast::{Arg, BinOp, CEIL, Expr, FLOOR, INDEX, Literal};
pub use sva_samples::Round;
use sva_samples::{Between, Map};

use crate::instantiate::{Cx, Instances, Node};
use crate::time::{Affine, Grid, Lattice};

/// Sample index `round(time) + plus` on the reader's grid; a count has no time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Index {
    pub time: Option<Affine>,
    pub round: Round,
    pub plus: i64,
}

impl Index {
    fn count(plus: i64) -> Index {
        Index {
            time: None,
            round: Round::Even,
            plus,
        }
    }

    pub(crate) fn as_count(self) -> Option<i64> {
        self.time.is_none().then_some(self.plus)
    }

    fn negated(self) -> Option<Index> {
        Some(Index {
            time: self.time.map(|time| Affine {
                scale: time.scale.neg(),
                shift: time.shift.neg(),
            }),
            round: match self.round {
                Round::Even => Round::Even,
                Round::Floor => Round::Ceil,
                Round::Ceil => Round::Floor,
            },
            plus: self.plus.checked_neg()?,
        })
    }

    fn moved(self, by: i64) -> Option<Index> {
        Some(Index {
            plus: self.plus.checked_add(by)?,
            ..self
        })
    }

    /// Sample `n` of `grid` reads this index of it, rounded once from its exact position.
    pub fn map(self, grid: Grid) -> Option<Map> {
        let Some(time) = self.time else {
            return Map::new(0, i128::from(self.plus), 1);
        };
        let (a, b, d) = grid.landing(time, grid)?;
        let (b, between) = match self.round {
            Round::Even => (b, Between::Even),
            Round::Floor => (b, Between::Floor),
            Round::Ceil => (b.checked_add(d - 1)?, Between::Floor),
        };
        let plus = i128::from(self.plus).checked_mul(d)?;
        Map::rounded(a, b.checked_add(plus)?, d, between)
    }
}

/// Whether `e` is an integer: a whole literal, `idx(...)`, or `+`, `-` and `*` over integers.
pub(crate) fn integer(inst: &Instances, e: &Expr, cx: Cx) -> bool {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| integer(inst, e2, cx2)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Num(n)) => whole(*n).is_some(),
        Node::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul, l, r) => {
            integer(inst, l, cx) && integer(inst, r, cx)
        }
        Node::Call { name, .. } => name == INDEX,
        _ => false,
    }
}

/// The index an integer names where it is one `idx` of a line in `t`, negated or not, plus a
/// count; `None` for any other.
pub(crate) fn read(inst: &Instances, e: &Expr, cx: Cx) -> Option<Index> {
    if let Some(r) = inst.follow(e, cx, |e2, cx2| read(inst, e2, cx2)) {
        return r;
    }
    match inst.node(e, cx) {
        Node::Lit(Literal::Num(n)) => whole(*n).map(Index::count),
        Node::Call { name, args, .. } if name == INDEX => cast(inst, args, cx),
        Node::Bin(op, l, r) => {
            let (l, r) = (read(inst, l, cx)?, read(inst, r, cx)?);
            match op {
                BinOp::Add => sum(l, r),
                BinOp::Sub => sum(l, r.negated()?),
                BinOp::Mul => product(l, r),
                BinOp::Div | BinOp::Mod => None,
            }
        }
        _ => None,
    }
}

fn cast(inst: &Instances, args: &[Arg], cx: Cx) -> Option<Index> {
    let Some(Arg::Pos(time)) = args.first() else {
        return None;
    };
    Some(Index {
        time: Some(crate::loops::time_of(inst, time, cx)?),
        round: rounding(args.get(1))?,
        plus: 0,
    })
}

/// `idx`'s second argument.
pub(crate) fn rounding(arg: Option<&Arg>) -> Option<Round> {
    match arg {
        None => Some(Round::Even),
        Some(Arg::Pos(Expr::Var(word))) if word == FLOOR => Some(Round::Floor),
        Some(Arg::Pos(Expr::Var(word))) if word == CEIL => Some(Round::Ceil),
        Some(_) => None,
    }
}

fn sum(l: Index, r: Index) -> Option<Index> {
    match (l.as_count(), r.as_count()) {
        (Some(k), _) => r.moved(k),
        (_, Some(k)) => l.moved(k),
        _ => None,
    }
}

fn product(l: Index, r: Index) -> Option<Index> {
    let (k, other) = match (l.as_count(), r.as_count()) {
        (Some(a), Some(b)) => return a.checked_mul(b).map(Index::count),
        (Some(k), None) => (k, r),
        (None, Some(k)) => (k, l),
        (None, None) => return None,
    };
    match k {
        0 => Some(Index::count(0)),
        1 => Some(other),
        -1 => other.negated(),
        _ => None,
    }
}

fn whole(n: f64) -> Option<i64> {
    (n.is_finite() && n.fract() == 0.0 && n.abs() < 2f64.powi(62)).then_some(n as i64)
}
