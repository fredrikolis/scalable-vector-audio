// Concern: bounds a written closed form's value from each instant on, constructor by constructor | Non-concern: forms an atom sum reaches, bounded atom by atom | IO: (&Body) -> Range, (t) -> [lo, hi]

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{Body, Fold, NodeId, Unary, Var};
use sva_samples::collapse::run::{bound, reach};

/// A written form compiled once, so every instant reads it without normalizing again.
pub(super) enum Range {
    Atoms(Vec<SpectralAtom>),
    /// A run's lines, the most they reach and its own rounding bound.
    Run(Vec<SpectralAtom>, f64, f64),
    Real(f64),
    Line,
    Node(NodeId),
    Add(Vec<Range>),
    Mul(Vec<Range>),
    Div(Box<Range>, Box<Range>),
    Pow(Box<Range>, i32),
    Map(Unary, Box<Range>),
    Max(Vec<Range>),
    Min(Vec<Range>),
    Crop(Box<Range>, f64, f64),
    Shift(Box<Range>, f64),
    Wide(Vec<Range>),
}

/// How a node's own bounds reach a written form, and the rate its lines are sampled at.
pub(super) struct Reads<'a> {
    pub(super) node: &'a dyn Fn(NodeId, f64) -> f64,
    pub(super) floor: &'a dyn Fn(NodeId) -> f64,
    pub(super) rate: f64,
}

/// One rounding's relative size, with room for the few a single constructor makes.
pub(super) const OP: f64 = 8.0 * f64::EPSILON;

/// The rounding a transform over up to 2^64 bins adds, counted in terms.
pub(super) const TRANSFORM_OPS: f64 = 64.0;

/// `[lo, hi]` holds the exact value at every instant from one on, and `err` bounds how far a
/// rendered value may sit from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub(super) lo: f64,
    pub(super) hi: f64,
    pub(super) err: f64,
}

impl Span {
    fn new(lo: f64, hi: f64, err: f64) -> Span {
        Span { lo, hi, err }
    }

    pub(super) fn reach(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }
}

impl Range {
    /// The constructor no bound is derived for, where one is reached.
    pub(super) fn of(f: &Body) -> Result<Range, &'static str> {
        if let Body::Run(run) = f
            && let Ok(sum) = sva_formula::normalize(f, Var::T)
        {
            let atoms = sum.atoms().copied().collect();
            return Ok(Range::Run(atoms, reach(run), bound(run)));
        }
        if holds_no_run(f)
            && let Ok(sum) = sva_formula::normalize(f, Var::T)
        {
            let atoms: Vec<SpectralAtom> = sum.atoms().copied().collect();
            match atoms.as_slice() {
                [] => return Ok(Range::Real(0.0)),
                [atom] if atom.is_bare() && atom.c.im == 0.0 => return Ok(Range::Real(atom.c.re)),
                _ if atoms.iter().any(|a| !polynomial(a)) => return Ok(Range::Atoms(atoms)),
                _ => {}
            }
        }
        let each = |parts: &[sva_formula::Part]| {
            parts
                .iter()
                .map(|p| Range::of(&p.body))
                .collect::<Result<Vec<_>, _>>()
        };
        let one = |p: &sva_formula::Part| Range::of(&p.body).map(Box::new);
        Ok(match f {
            Body::Line => Range::Line,
            Body::Node(id) => Range::Node(*id),
            Body::Add(parts) => Range::Add(each(parts)?),
            Body::Mul(parts) => Range::Mul(each(parts)?),
            Body::Div(num, den) => Range::Div(one(num)?, one(den)?),
            Body::Pow(base, n) => Range::Pow(one(base)?, *n),
            Body::Apply(Unary::Sin | Unary::Cos, arg) if !real(&arg.body) => {
                return Err("a sine of a complex value");
            }
            Body::Apply(op, arg) => Range::Map(*op, one(arg)?),
            Body::Fold(Fold::Max, parts) => Range::Max(each(parts)?),
            Body::Fold(Fold::Min, parts) => Range::Min(each(parts)?),
            Body::Crop { of, l, r, .. } => Range::Crop(one(of)?, l.value(), r.value()),
            Body::Shift { by, of } => Range::Shift(one(of)?, *by),
            Body::Join(parts) => Range::Wide(each(parts)?),
            Body::Channel(of, _) => Range::Wide(vec![Range::of(&of.body)?]),
            _ => return Err("a written constructor with no bound"),
        })
    }

    pub(super) fn nodes(&self, out: &mut Vec<NodeId>) {
        match self {
            Range::Node(id) => out.push(*id),
            Range::Add(parts)
            | Range::Mul(parts)
            | Range::Max(parts)
            | Range::Min(parts)
            | Range::Wide(parts) => parts.iter().for_each(|p| p.nodes(out)),
            Range::Div(a, b) => {
                a.nodes(out);
                b.nodes(out);
            }
            Range::Pow(a, _) | Range::Map(_, a) | Range::Crop(a, ..) | Range::Shift(a, _) => {
                a.nodes(out)
            }
            Range::Atoms(_) | Range::Run(..) | Range::Real(_) | Range::Line => {}
        }
    }

    /// A level the value returns to forever: `last` is the latest instant a tail is read
    /// from, and each operand's own tail bounds what it returns to.
    pub(super) fn floor(&self, last: f64, reads: &Reads) -> f64 {
        let tail = |r: &Range| {
            r.from(last, reads.node)
                .map_or(f64::INFINITY, |s| s.reach() + s.err)
        };
        // A value that stays on one side of zero from `last` on stays that far from it.
        let apart = self.from(last, reads.node).map_or(0.0, |s| {
            let gap = match (s.lo > 0.0, s.hi < 0.0) {
                (true, _) => s.lo,
                (_, true) => -s.hi,
                _ => 0.0,
            };
            (gap - s.err).max(0.0)
        });
        let kept = match self {
            Range::Atoms(atoms) | Range::Run(atoms, ..) => {
                super::floor::of_atoms(atoms, reads.rate)
            }
            Range::Real(c) => c.abs(),
            Range::Line => f64::INFINITY,
            Range::Node(id) => (reads.floor)(*id),
            Range::Add(parts) => {
                let floors: Vec<f64> = parts.iter().map(|p| p.floor(last, reads)).collect();
                let tails: Vec<f64> = parts.iter().map(tail).collect();
                super::floor::summed(&floors, &tails)
            }
            Range::Mul(parts) => {
                let (constant, moving): (Vec<&Range>, Vec<&Range>) =
                    parts.iter().partition(|p| matches!(p, Range::Real(_)));
                let scale: f64 = constant
                    .iter()
                    .map(|p| match p {
                        Range::Real(c) => c.abs(),
                        _ => 1.0,
                    })
                    .product();
                match moving.as_slice() {
                    [one] => one.floor(last, reads) * scale,
                    _ => 0.0,
                }
            }
            Range::Div(num, den) => match **den {
                Range::Real(d) if d != 0.0 => num.floor(last, reads) / d.abs(),
                _ => 0.0,
            },
            Range::Pow(base, n) if *n >= 1 => base.floor(last, reads).powi(*n),
            Range::Map(op, arg) => super::floor::through(*op, arg.floor(last, reads)),
            Range::Crop(of, _, r) if r.is_infinite() => of.floor(last, reads),
            Range::Shift(of, _) => of.floor(last, reads),
            Range::Wide(parts) => parts
                .iter()
                .map(|p| p.floor(last, reads))
                .fold(0.0, f64::max),
            _ => 0.0,
        };
        kept.max(apart)
    }

    /// Interval arithmetic over `s >= t`: each operand's span holds over the same instants,
    /// so their combination holds too. `node` answers a node's magnitude from an instant on,
    /// its own rounding included. A time read slightly off is still an instant from `t` on.
    pub(super) fn from(&self, t: f64, node: &dyn Fn(NodeId, f64) -> f64) -> Option<Span> {
        let magnitude = |m: f64| Some(Span::new(-m, m, 0.0));
        match self {
            Range::Atoms(atoms) => {
                let mut sum = 0.0;
                for atom in atoms {
                    sum += sup_from(atom, t)?;
                }
                let terms = atoms.len() as f64 + TRANSFORM_OPS;
                Some(Span::new(-sum, sum, OP * terms * sum))
            }
            Range::Run(_, reach, err) => Some(Span::new(-reach, *reach, *err)),
            Range::Real(c) => Some(Span::new(*c, *c, 0.0)),
            Range::Line => Some(Span::new(t, f64::INFINITY, 0.0)),
            Range::Node(id) => magnitude(node(*id, t)),
            Range::Add(parts) => {
                let mut held = Span::new(0.0, 0.0, 0.0);
                for p in parts {
                    let s = p.from(t, node)?;
                    held = Span::new(held.lo + s.lo, held.hi + s.hi, held.err + s.err);
                    held.err += OP * held.reach();
                }
                Some(held)
            }
            Range::Mul(parts) => parts.iter().try_fold(Span::new(1.0, 1.0, 0.0), |held, p| {
                Some(times(held, p.from(t, node)?))
            }),
            Range::Div(num, den) => {
                let d = den.from(t, node)?;
                let least = d.lo.abs().min(d.hi.abs()) - d.err;
                if (d.lo <= 0.0 && d.hi >= 0.0) || least <= 0.0 {
                    return None;
                }
                let n = num.from(t, node)?;
                let (lo, hi) = product((n.lo, n.hi), (1.0 / d.hi, 1.0 / d.lo));
                let err = (n.err + n.reach() * d.err / least) / least;
                Some(Span::new(lo, hi, err + OP * (n.reach() + n.err) / least))
            }
            Range::Pow(base, n) => {
                let span = base.from(t, node)?;
                let mut held = Span::new(1.0, 1.0, 0.0);
                for _ in 0..n.unsigned_abs() {
                    held = times(held, span);
                }
                match *n >= 0 {
                    true => Some(held),
                    false => inverse(held),
                }
            }
            Range::Map(op, arg) => mapped(*op, arg.from(t, node)?),
            Range::Max(parts) => fold(parts, t, node, f64::max),
            Range::Min(parts) => fold(parts, t, node, f64::min),
            Range::Crop(of, l, r) => match t >= *r {
                true => Some(Span::new(0.0, 0.0, 0.0)),
                false => {
                    let s = of.from(t.max(*l), node)?;
                    Some(Span::new(
                        s.lo.min(0.0),
                        s.hi.max(0.0),
                        s.err + OP * s.reach(),
                    ))
                }
            },
            Range::Shift(of, by) => of.from(t - by, node),
            Range::Wide(parts) => {
                let (mut m, mut err) = (0.0f64, 0.0f64);
                for p in parts {
                    let s = p.from(t, node)?;
                    m = m.max(s.reach());
                    err = err.max(s.err);
                }
                Some(Span::new(-m, m, err))
            }
        }
    }
}

/// A run is summed by its own evaluator, so its lines' atoms never stand for it.
fn holds_no_run(f: &Body) -> bool {
    !matches!(f, Body::Run(_))
        && sva_formula::closed_form::children(f)
            .iter()
            .all(|p| holds_no_run(&p.body))
}

/// `1 / x` over a span that stays clear of zero by more than its own rounding.
fn inverse(x: Span) -> Option<Span> {
    let least = x.lo.abs().min(x.hi.abs()) - x.err;
    if (x.lo <= 0.0 && x.hi >= 0.0) || least <= 0.0 {
        return None;
    }
    let err = x.err / (least * least) + OP / least;
    Some(Span::new(1.0 / x.hi, 1.0 / x.lo, err))
}

/// A real power of `t` alone: its sign survives constructor by constructor, and a
/// magnitude would lose it.
fn polynomial(a: &SpectralAtom) -> bool {
    a.c.im == 0.0 && a.exp.is_none() && a.gauss.is_none() && a.ind.is_none() && a.pole.is_none()
}

fn real(f: &Body) -> bool {
    sva_formula::affine::axis(f, &Unread) == sva_formula::affine::Axis::Real
}

/// A node's own type is not read here, so it counts as complex.
struct Unread;

impl sva_formula::Env for Unread {
    fn node(&self, _: NodeId) -> sva_formula::Ty {
        sva_formula::Ty::form(Var::T, false, sva_formula::Codomain::Complex)
    }

    fn param(&self, _: sva_formula::ParamId) -> sva_formula::Ty {
        sva_formula::Ty::form(Var::T, false, sva_formula::Codomain::Complex)
    }
}

fn fold(
    parts: &[Range],
    t: f64,
    node: &dyn Fn(NodeId, f64) -> f64,
    pick: fn(f64, f64) -> f64,
) -> Option<Span> {
    let mut it = parts.iter();
    let first = it.next()?.from(t, node)?;
    it.try_fold(first, |held, p| {
        let s = p.from(t, node)?;
        Some(Span::new(
            pick(held.lo, s.lo),
            pick(held.hi, s.hi),
            held.err.max(s.err),
        ))
    })
}

/// A zero factor is zero whatever the other spans, infinite ones included.
fn product(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    if a == (0.0, 0.0) || b == (0.0, 0.0) {
        return (0.0, 0.0);
    }
    let corners = [a.0 * b.0, a.0 * b.1, a.1 * b.0, a.1 * b.1];
    let clean = |v: f64| if v.is_nan() { 0.0 } else { v };
    let lo = corners
        .iter()
        .map(|v| clean(*v))
        .fold(f64::INFINITY, f64::min);
    let hi = corners
        .iter()
        .map(|v| clean(*v))
        .fold(f64::NEG_INFINITY, f64::max);
    (lo, hi)
}

/// `|a'b' - ab| <= e_a |b'| + |a| e_b`, and the product's own rounding beside it.
fn times(a: Span, b: Span) -> Span {
    let (lo, hi) = product((a.lo, a.hi), (b.lo, b.hi));
    if (lo, hi) == (0.0, 0.0) && (a.err == 0.0 || b.err == 0.0) {
        return Span::new(0.0, 0.0, 0.0);
    }
    let err = a.err * (b.reach() + b.err) + a.reach() * b.err;
    let reach = lo.abs().max(hi.abs());
    Span::new(lo, hi, err + OP * (reach + err))
}

/// Each map is Lipschitz over the span its argument can reach, rounding and all.
fn mapped(op: Unary, s: Span) -> Option<Span> {
    let (lo, hi, err) = (s.lo, s.hi, s.err);
    let next = |lo: f64, hi: f64, slope: f64| {
        let reach = lo.abs().max(hi.abs());
        Some(Span::new(lo, hi, slope * err + OP * reach))
    };
    match op {
        Unary::Exp => next(lo.exp(), hi.exp(), (hi + err).exp()),
        Unary::Tanh => next(lo.tanh(), hi.tanh(), 1.0),
        Unary::Sat => next(lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0), 1.0),
        Unary::Sin | Unary::Cos => next(-1.0, 1.0, 1.0),
        Unary::Abs if lo >= 0.0 => next(lo, hi, 1.0),
        Unary::Abs if hi <= 0.0 => next(-hi, -lo, 1.0),
        Unary::Abs => next(0.0, hi.max(-lo), 1.0),
        Unary::Sqrt if lo >= 0.0 => {
            let reach = hi.sqrt();
            Some(Span::new(lo.sqrt(), reach, err.sqrt() + OP * reach))
        }
        Unary::Log if lo - err > 0.0 => next(lo.ln(), hi.ln(), 1.0 / (lo - err)),
        Unary::Sqrt | Unary::Log => None,
    }
}
