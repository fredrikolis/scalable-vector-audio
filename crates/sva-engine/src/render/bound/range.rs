// Concern: bounds a written closed form's value from each instant on, constructor by constructor | Non-concern: forms an atom sum reaches, bounded atom by atom | IO: (&Body) -> Range, (t) -> [lo, hi]

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::sup_from;
use sva_formula::{Body, Edge, Fold, Indicator, NodeId, Unary, Var};
use sva_samples::collapse::run::{bound, reach};

/// A written form compiled once, so every instant reads it without normalizing again.
pub(super) enum Range {
    Atoms(Vec<SpectralAtom>),
    /// The most a run's lines reach, and its own rounding bound.
    Run(f64, f64),
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
    Warp(Box<Range>, Box<Range>),
    Wide(Vec<Range>),
}

/// One rounding's relative size, with room for the few a single constructor makes.
pub(super) const OP: f64 = 8.0 * f64::EPSILON;

/// The rounding a transform over up to 2^64 bins adds, counted in terms.
pub(super) const TRANSFORM_OPS: f64 = 64.0;

/// `[lo, hi]` holds the exact value `v` at every instant from one on, and `err + rel * |v|`
/// bounds how far a rendered value may sit from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub(super) lo: f64,
    pub(super) hi: f64,
    pub(super) err: f64,
    rel: f64,
}

impl Span {
    fn new(lo: f64, hi: f64, err: f64) -> Span {
        Span {
            lo,
            hi,
            err,
            rel: 0.0,
        }
    }

    pub(super) fn reach(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    fn absolute(self) -> Span {
        match self.rel == 0.0 {
            true => self,
            false => Span::new(self.lo, self.hi, self.err + self.rel * self.reach()),
        }
    }

    fn constant(self) -> Option<(f64, f64)> {
        let s = self.absolute();
        let rho = s.err / s.lo.abs();
        (s.lo == s.hi && s.lo != 0.0 && s.lo.is_finite() && rho < 0.5).then_some((s.lo, rho))
    }
}

impl Range {
    /// The constructor no bound is derived for, where one is reached.
    pub(super) fn of(f: &Body) -> Result<Range, &'static str> {
        if let Body::Run(run) = f
            && sva_formula::normalize(f, Var::T).is_ok()
        {
            return Ok(Range::Run(reach(run), bound(run)));
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
            Body::Warp { at, of } => Range::Warp(Box::new(Range::time(&at.body)?), one(of)?),
            Body::Join(parts) => Range::Wide(each(parts)?),
            // A rendered term may sit anywhere its magnitude bound allows.
            Body::Banded(b) if b.reach.is_finite() => {
                let rounded = 1.0 + OP * (b.widest as f64 + TRANSFORM_OPS);
                Range::Run(b.reach, 2.0 * b.reach * rounded)
            }
            Body::Channel(of, _) => Range::Wide(vec![Range::of(&of.body)?]),
            _ => return Err("a written constructor with no bound"),
        })
    }

    /// A sum keeps `t` apart from what moves it: one magnitude over both would lose `t`.
    fn time(f: &Body) -> Result<Range, &'static str> {
        match f {
            Body::Add(parts) => parts
                .iter()
                .map(|p| Range::of(&p.body))
                .collect::<Result<Vec<_>, _>>()
                .map(Range::Add),
            other => Range::of(other),
        }
    }

    /// Where its span is `[-m, m]` at every instant: a magnitude `m` never falls under, each
    /// node's own read by `node`. Scaling repeats the span's own roundings, which are monotone.
    pub(super) fn floor(&self, node: &dyn Fn(NodeId) -> f64) -> Option<f64> {
        let most = |parts: &[Range]| parts.iter().map(|p| p.floor(node)).try_fold(0.0, max);
        match self {
            Range::Node(id) => Some(node(*id)),
            Range::Atoms(atoms) => Some(super::steady(atoms)),
            Range::Run(..) => Some(0.0),
            Range::Add(parts) => most(parts),
            Range::Wide(parts) => Some(
                parts
                    .iter()
                    .filter_map(|p| p.floor(node))
                    .fold(0.0, f64::max),
            ),
            Range::Crop(of, _, r) if *r == f64::INFINITY => of.floor(node),
            Range::Crop(..) => Some(0.0),
            Range::Shift(of, _) => of.floor(node),
            Range::Div(num, den) => match **den {
                Range::Real(c) if c != 0.0 && c.is_finite() => {
                    Some(num.floor(node)? * (1.0 / c).abs())
                }
                _ => None,
            },
            Range::Mul(parts) => {
                let (mut constant, mut floor) = (1.0f64, None);
                for part in parts {
                    match (part, floor) {
                        (Range::Real(c), None) => constant *= c,
                        (Range::Real(c), Some(f)) => floor = Some(f * c.abs()),
                        (other, None) if constant != 0.0 && constant.is_finite() => {
                            floor = Some(other.floor(node)? * constant.abs());
                        }
                        _ => return None,
                    }
                }
                floor
            }
            _ => None,
        }
    }

    pub(super) fn nodes(&self, out: &mut Vec<NodeId>) {
        match self {
            Range::Node(id) => out.push(*id),
            Range::Add(parts)
            | Range::Mul(parts)
            | Range::Max(parts)
            | Range::Min(parts)
            | Range::Wide(parts) => parts.iter().for_each(|p| p.nodes(out)),
            Range::Div(a, b) | Range::Warp(a, b) => {
                a.nodes(out);
                b.nodes(out);
            }
            Range::Pow(a, _) | Range::Map(_, a) | Range::Crop(a, ..) | Range::Shift(a, _) => {
                a.nodes(out)
            }
            Range::Atoms(_) | Range::Run(..) | Range::Real(_) | Range::Line => {}
        }
    }

    /// Interval arithmetic over `s >= t`: each operand's span holds over the same instants,
    /// so their combination holds too. `node` answers a node's magnitude from an instant on,
    /// its own rounding included. A time read slightly off is still an instant from `t` on.
    pub(super) fn from(&self, t: f64, node: &dyn Fn(NodeId, f64) -> f64) -> Option<Span> {
        self.over(t, f64::INFINITY, node)
    }

    fn over(&self, t: f64, to: f64, node: &dyn Fn(NodeId, f64) -> f64) -> Option<Span> {
        self.span(t, to, node).map(Span::absolute)
    }

    fn span(&self, t: f64, to: f64, node: &dyn Fn(NodeId, f64) -> f64) -> Option<Span> {
        let magnitude = |m: f64| Some(Span::new(-m, m, 0.0));
        match self {
            Range::Atoms(atoms) => {
                let mut sum = 0.0;
                for atom in atoms {
                    sum += sup_from(&until(atom, to), t)?;
                }
                let terms = atoms.len() as f64 + TRANSFORM_OPS;
                Some(Span::new(-sum, sum, OP * terms * sum))
            }
            Range::Run(reach, err) => Some(Span::new(-reach, *reach, *err)),
            Range::Real(c) => Some(Span::new(*c, *c, 0.0)),
            Range::Line => Some(Span::new(t, to.max(t), 0.0)),
            Range::Node(id) => magnitude(node(*id, t)),
            Range::Add(parts) => {
                let spans: Option<Vec<Span>> = parts.iter().map(|p| p.span(t, to, node)).collect();
                Some(summed(&spans?))
            }
            Range::Mul(parts) => parts.iter().try_fold(Span::new(1.0, 1.0, 0.0), |held, p| {
                let s = p.span(t, to, node)?;
                Some(match (held.constant(), s.constant()) {
                    (Some((c, rho)), _) => scaled(s, c, rho),
                    (_, Some((c, rho))) => scaled(held, c, rho),
                    _ => times(held.absolute(), s.absolute()),
                })
            }),
            Range::Div(num, den) => {
                let d = den.over(t, to, node)?;
                if let Some((c, rho)) = d.constant() {
                    return Some(scaled(num.span(t, to, node)?, 1.0 / c, rho / (1.0 - rho)));
                }
                let least = d.lo.abs().min(d.hi.abs()) - d.err;
                if (d.lo <= 0.0 && d.hi >= 0.0) || least <= 0.0 {
                    return None;
                }
                let n = num.over(t, to, node)?;
                let (lo, hi) = product((n.lo, n.hi), (1.0 / d.hi, 1.0 / d.lo));
                let err = (n.err + n.reach() * d.err / least) / least;
                Some(Span::new(lo, hi, err + OP * (n.reach() + n.err) / least))
            }
            Range::Pow(base, n) => {
                let span = base.over(t, to, node)?;
                let mut held = Span::new(1.0, 1.0, 0.0);
                for _ in 0..n.unsigned_abs() {
                    held = times(held, span);
                }
                match *n >= 0 {
                    true => Some(held),
                    false => inverse(held),
                }
            }
            Range::Map(Unary::Exp, arg) => Some(exp(arg.span(t, to, node)?)),
            Range::Map(op, arg) => mapped(*op, arg.over(t, to, node)?),
            Range::Max(parts) => fold(parts, (t, to), node, f64::max),
            Range::Min(parts) => fold(parts, (t, to), node, f64::min),
            Range::Crop(of, l, r) => match t >= *r || to < *l {
                true => Some(Span::new(0.0, 0.0, 0.0)),
                false => {
                    let s = of.over(t.max(*l), to.min(*r), node)?;
                    Some(Span::new(
                        s.lo.min(0.0),
                        s.hi.max(0.0),
                        s.err + OP * s.reach(),
                    ))
                }
            },
            Range::Shift(of, by) => of.span(t - by, to - by, node),
            // A rounded time reads anywhere `of` reaches near the exact one.
            Range::Warp(at, of) => {
                let when = at.over(t, to, node)?;
                let s = of.over(when.lo - when.err, when.hi + when.err, node)?;
                Some(Span::new(s.lo, s.hi, s.hi - s.lo + s.err))
            }
            Range::Wide(parts) => {
                let (mut m, mut err) = (0.0f64, 0.0f64);
                for p in parts {
                    let s = p.over(t, to, node)?;
                    m = m.max(s.reach());
                    err = err.max(s.err);
                }
                Some(Span::new(-m, m, err))
            }
        }
    }
}

/// With one part unbounded, it and each partial sum is under `|v| + rest` plus rounding.
fn summed(spans: &[Span]) -> Span {
    let (lo, hi) = spans
        .iter()
        .fold((0.0, 0.0), |(lo, hi), s| (lo + s.lo, hi + s.hi));
    let unbounded: Vec<usize> = (0..spans.len())
        .filter(|j| !spans[*j].reach().is_finite())
        .collect();
    let adds = spans.len() as f64 * OP;
    if let ([k], true) = (unbounded.as_slice(), adds < 0.5) {
        let others = spans.iter().enumerate().filter(|(j, _)| j != k);
        let rest: f64 = others.clone().map(|(_, s)| s.reach()).sum();
        let own = others.map(|(_, s)| s.absolute().err).sum::<f64>()
            + spans[*k].err
            + spans[*k].rel * rest;
        let grown = adds / (1.0 - adds);
        let rel = spans[*k].rel;
        return Span {
            lo,
            hi,
            err: own + grown * (rest + own),
            rel: rel + grown * (1.0 + rel),
        };
    }
    let mut held = Span::new(0.0, 0.0, 0.0);
    for s in spans.iter().map(|s| s.absolute()) {
        held = Span::new(held.lo + s.lo, held.hi + s.hi, held.err + s.err);
        held.err += OP * held.reach();
    }
    held
}

/// `|c' v' - c v| <= |c| err (1 + rho) + (rho + rel (1 + rho)) |c v|`, then one rounding.
fn scaled(s: Span, c: f64, rho: f64) -> Span {
    let (lo, hi) = product((s.lo, s.hi), (c, c));
    Span {
        lo,
        hi,
        err: c.abs() * s.err * (1.0 + rho) * (1.0 + OP),
        rel: (rho + s.rel * (1.0 + rho)) * (1.0 + OP) + OP,
    }
}

/// Error at most `g(x) = exp(x + d) (d + OP)`, `d = err + rel |x|`, which rises on `x >= 0`
/// and on `x <= 0` until `(err + OP) / rel - 1 / (1 - rel)`.
fn exp(s: Span) -> Span {
    let (err, rel) = (s.err, s.rel);
    if rel >= 0.5 || (rel > 0.0 && !s.hi.is_finite()) {
        return mapped(Unary::Exp, s.absolute()).expect("exp maps every span");
    }
    let g = |x: f64| {
        let d = err + rel * x.abs();
        (x + d).exp() * (d + OP)
    };
    let mut most = g(s.hi);
    if s.lo < 0.0 {
        let top = s.hi.min(0.0);
        most = most.max(g(top));
        if rel > 0.0 {
            let turn = ((err + OP) / rel - 1.0 / (1.0 - rel)).clamp(s.lo, top);
            most = most.max(g(turn));
        }
    }
    let most = if most.is_nan() { f64::INFINITY } else { most };
    Span::new(s.lo.exp(), s.hi.exp(), most)
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

fn max(held: f64, floor: Option<f64>) -> Option<f64> {
    Some(held.max(floor?))
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
    (t, to): (f64, f64),
    node: &dyn Fn(NodeId, f64) -> f64,
    pick: fn(f64, f64) -> f64,
) -> Option<Span> {
    let mut it = parts.iter();
    let first = it.next()?.span(t, to, node)?;
    it.try_fold(first, |held, p| {
        Some(picked(held, p.span(t, to, node)?, pick))
    })
}

fn until(atom: &SpectralAtom, to: f64) -> SpectralAtom {
    if to == f64::INFINITY {
        return *atom;
    }
    let l = atom.ind.map_or(Edge::at(f64::NEG_INFINITY), |i| i.l);
    let r = atom.ind.map_or(to, |i| i.r.value().min(to));
    SpectralAtom {
        ind: Some(Indicator { l, r: Edge::at(r) }),
        ..*atom
    }
}

/// Rounding picks the other operand only where `|v_other| <= |v|` plus both errors.
fn picked(a: Span, b: Span, pick: fn(f64, f64) -> f64) -> Span {
    let rel = a.rel.max(b.rel);
    let (a, b, rel) = match rel < 0.5 {
        true => (a, b, rel),
        false => (a.absolute(), b.absolute(), 0.0),
    };
    let grown = (1.0 + rel) / (1.0 - rel);
    Span {
        lo: pick(a.lo, b.lo),
        hi: pick(a.hi, b.hi),
        err: a.err.max(b.err) * grown,
        rel: rel * grown,
    }
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
        // Exact wherever its argument cannot round across zero; there, anything in [0, 1].
        Unary::Step if lo - err >= 0.0 => Some(Span::new(1.0, 1.0, 0.0)),
        Unary::Step if hi + err < 0.0 => Some(Span::new(0.0, 0.0, 0.0)),
        // The jump is the error.
        Unary::Step => Some(Span::new(0.0, 1.0, 1.0)),
    }
}
