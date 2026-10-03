// Concern: reads the polynomial a formula spells in the free variable, to degree two | Non-concern: lowering one to atoms (spectral_sum/build.rs) | IO: (&Body) -> Vec<Coeff> or nothing

use crate::closed_form::{Body, IndexId, Unary};
use crate::complex::C64;
use crate::through::{Opaque, Reads};

/// An affine argument makes the line and delta rows, a quadratic one the Gaussian row.
pub const MAX_DEGREE: usize = 2;

/// A series index is an unknown of known axis, which is all a growth row asks of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Real,
    Imaginary,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coeff {
    Exact(C64),
    Unknown(Axis),
}

impl Axis {
    pub fn of_codomain(codomain: crate::ty::Codomain) -> Axis {
        match codomain {
            crate::ty::Codomain::Real => Axis::Real,
            crate::ty::Codomain::Complex => Axis::Any,
        }
    }

    fn of(c: C64) -> Axis {
        match (c.is_real(), c.re == 0.0) {
            (true, _) => Axis::Real,
            (false, true) => Axis::Imaginary,
            _ => Axis::Any,
        }
    }

    fn times(self, other: Axis) -> Axis {
        match (self, other) {
            (Axis::Real, a) | (a, Axis::Real) => a,
            (Axis::Imaginary, Axis::Imaginary) => Axis::Real,
            _ => Axis::Any,
        }
    }

    fn plus(self, other: Axis) -> Axis {
        if self == other { self } else { Axis::Any }
    }
}

impl Coeff {
    pub const ZERO: Coeff = Coeff::Exact(C64::ZERO);
    pub const ONE: Coeff = Coeff::Exact(C64::ONE);

    pub fn exact(self) -> Option<C64> {
        match self {
            Coeff::Exact(c) => Some(c),
            Coeff::Unknown(_) => None,
        }
    }

    pub fn axis(self) -> Axis {
        match self {
            Coeff::Exact(c) => Axis::of(c),
            Coeff::Unknown(a) => a,
        }
    }

    pub fn is_zero(self) -> bool {
        matches!(self, Coeff::Exact(c) if c.is_zero())
    }

    fn add(self, other: Coeff) -> Coeff {
        match (self, other) {
            (Coeff::Exact(a), Coeff::Exact(b)) => Coeff::Exact(a + b),
            (a, b) if a.is_zero() => b,
            (a, b) if b.is_zero() => a,
            (a, b) => Coeff::Unknown(a.axis().plus(b.axis())),
        }
    }

    fn mul(self, other: Coeff) -> Coeff {
        match (self, other) {
            (Coeff::Exact(a), Coeff::Exact(b)) => Coeff::Exact(a * b),
            (a, b) if a.is_zero() || b.is_zero() => Coeff::ZERO,
            (a, b) => Coeff::Unknown(a.axis().times(b.axis())),
        }
    }

    fn div(self, other: Coeff) -> Coeff {
        match (self, other) {
            (Coeff::Exact(a), Coeff::Exact(b)) => Coeff::Exact(a / b),
            (a, b) => Coeff::Unknown(a.axis().times(b.axis())),
        }
    }

    fn scale(self, k: f64) -> Coeff {
        self.mul(Coeff::Exact(C64::real(k)))
    }
}

/// Which symbol a polynomial is read in: the closed form's own variable, or one series index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reading {
    Free,
    Index(IndexId),
}

pub fn polynomial(f: &Body) -> Option<Vec<Coeff>> {
    polynomial_in(f, Reading::Free)
}

pub fn polynomial_in(f: &Body, reading: Reading) -> Option<Vec<Coeff>> {
    polynomial_read(f, reading, &Opaque)
}

/// Ascending coefficients, or nothing above degree two. Read in the free variable, an index
/// is an unknown real; read in an index, the free variable is not a coefficient at all.
pub fn polynomial_read(f: &Body, reading: Reading, reads: &dyn Reads) -> Option<Vec<Coeff>> {
    read(f, reading, None, reads).map(|p| p.as_slice().to_vec())
}

/// A series term at index `k = value`: the constant it is with `value` written for `k`.
pub fn exact_constant_at(f: &Body, k: IndexId, value: f64, reads: &dyn Reads) -> Option<C64> {
    constant(read(f, Reading::Free, Some((k, value)), reads)?)
}

/// `at` reads one index as a number written in its place.
fn read(f: &Body, reading: Reading, at: Option<(IndexId, f64)>, reads: &dyn Reads) -> Option<Poly> {
    let polynomial = |g: &Body| read(g, reading, at, reads);
    match f {
        Body::Node(id) => Poly::of(&reads.polynomial(*id, reading)?),
        Body::Const(c) => Some(Poly::one(Coeff::Exact(*c))),
        Body::Index(k) => Some(match (at, reading) {
            (Some((bound, value)), _) if *k == bound => Poly::one(Coeff::Exact(C64::real(value))),
            (_, Reading::Index(wanted)) if *k == wanted => Poly::of(&[Coeff::ZERO, Coeff::ONE])?,
            _ => Poly::one(Coeff::Unknown(Axis::Real)),
        }),
        Body::Line => match reading {
            Reading::Free => Poly::of(&[Coeff::ZERO, Coeff::ONE]),
            Reading::Index(_) => None,
        },
        Body::Keyed { seed, of } => {
            match read(&of.body, reading, at, reads)
                .and_then(constant)
                .and_then(|at| crate::hash::draw_nearest(*seed, at.re))
            {
                Some(drawn) => Some(Poly::one(Coeff::Exact(C64::real(drawn)))),
                None => Some(Poly::one(Coeff::Unknown(Axis::Real))),
            }
        }
        Body::Apply(op, arg) => match polynomial(&arg.body)?.as_slice() {
            [c] => Some(Poly::one(match c.exact() {
                Some(x) => Coeff::Exact(apply_scalar(*op, x)),
                None => Coeff::Unknown(unary_axis(*op, c.axis())),
            })),
            _ => None,
        },
        Body::Add(parts) => parts.iter().try_fold(Poly::one(Coeff::ZERO), |acc, p| {
            Some(add(&acc, &polynomial(&p.body)?))
        }),
        Body::Mul(parts) => parts.iter().try_fold(Poly::one(Coeff::ONE), |acc, p| {
            mul(&acc, &polynomial(&p.body)?)
        }),
        Body::Div(num, den) => {
            let mut n = polynomial(&num.body)?;
            let [d] = *polynomial(&den.body)?.as_slice() else {
                return None;
            };
            n.c[..n.len].iter_mut().for_each(|c| *c = c.div(d));
            Some(n)
        }
        Body::Pow(base, n) => {
            let b = polynomial(&base.body)?;
            let n = u32::try_from(*n).ok()?;
            (0..n).try_fold(Poly::one(Coeff::ONE), |acc, _| mul(&acc, &b))
        }
        Body::Shift { by, of } => Some(shift(&polynomial(&of.body)?, *by)),
        _ => None,
    }
}

/// Ascending coefficients held in place: a polynomial read here never passes degree two.
#[derive(Clone, Copy)]
struct Poly {
    c: [Coeff; MAX_DEGREE + 1],
    len: usize,
}

impl Poly {
    fn zeros(len: usize) -> Poly {
        Poly {
            c: [Coeff::ZERO; MAX_DEGREE + 1],
            len,
        }
    }

    fn one(c: Coeff) -> Poly {
        let mut p = Poly::zeros(1);
        p.c[0] = c;
        p
    }

    fn of(cs: &[Coeff]) -> Option<Poly> {
        let mut p = Poly::zeros(cs.len());
        p.c.get_mut(..cs.len())?.copy_from_slice(cs);
        Some(p)
    }

    fn as_slice(&self) -> &[Coeff] {
        &self.c[..self.len]
    }
}

fn constant(p: Poly) -> Option<C64> {
    match p.as_slice() {
        [b] => b.exact(),
        _ => None,
    }
}

/// Every unary this language folds on a scalar, in one place, so `normalize` and `infer`
/// cannot disagree about what a constant argument gives.
pub fn apply_scalar(op: Unary, x: C64) -> C64 {
    match op {
        Unary::Sin => ((C64::I * x).exp() - (-C64::I * x).exp()) / C64::I.scale(2.0),
        Unary::Cos => ((C64::I * x).exp() + (-C64::I * x).exp()).scale(0.5),
        Unary::Exp => x.exp(),
        Unary::Tanh => {
            let (p, m) = (x.exp(), (-x).exp());
            (p - m) / (p + m)
        }
        Unary::Sat => C64::real(x.re.clamp(-1.0, 1.0)),
        Unary::Abs => C64::real(x.abs()),
        Unary::Log => C64::new(x.abs().ln(), x.im.atan2(x.re)),
        Unary::Sqrt => {
            let r = x.abs().sqrt();
            let half = x.im.atan2(x.re) / 2.0;
            C64::new(r * half.cos(), r * half.sin())
        }
        Unary::Step => C64::real(step(x.re)),
    }
}

fn constant_in(f: &Body, reading: Reading) -> Option<C64> {
    constant_read(f, reading, &Opaque)
}

fn constant_read(f: &Body, reading: Reading, reads: &dyn Reads) -> Option<C64> {
    constant(read(f, reading, None, reads)?)
}

pub fn affine(f: &Body) -> Option<(Coeff, Coeff)> {
    affine_in(f, Reading::Free)
}

pub fn affine_in(f: &Body, reading: Reading) -> Option<(Coeff, Coeff)> {
    affine_read(f, reading, &Opaque)
}

pub fn affine_read(f: &Body, reading: Reading, reads: &dyn Reads) -> Option<(Coeff, Coeff)> {
    match polynomial_read(f, reading, reads)?[..] {
        [b] => Some((Coeff::ZERO, b)),
        [b, a] => Some((a, b)),
        _ => None,
    }
}

/// The offset of a time spelled as the variable plus a constant, an index counting as one.
pub fn slide(at: &Body) -> Option<Coeff> {
    slide_read(at, &Opaque)
}

pub(crate) fn slide_read(at: &Body, reads: &dyn Reads) -> Option<Coeff> {
    match affine_read(at, Reading::Free, reads)? {
        (Coeff::Exact(slope), offset) if slope.re == 1.0 && slope.im == 0.0 => Some(offset),
        _ => None,
    }
}

pub fn exact_affine(f: &Body) -> Option<(C64, C64)> {
    exact_affine_read(f, &Opaque)
}

pub(crate) fn exact_affine_read(f: &Body, reads: &dyn Reads) -> Option<(C64, C64)> {
    let (a, b) = affine_read(f, Reading::Free, reads)?;
    Some((a.exact()?, b.exact()?))
}

pub fn exact_constant(f: &Body) -> Option<C64> {
    constant_in(f, Reading::Free)
}

pub fn exact_constant_read(f: &Body, reads: &dyn Reads) -> Option<C64> {
    constant_read(f, Reading::Free, reads)
}

/// Whether a hash keyed on this formula names more than one value. The free variable moves
/// it; so does a node or a parameter, whose own closed form this judgment cannot see.
pub fn key_moves(f: &Body) -> bool {
    key_moves_read(f, &Opaque)
}

pub(crate) fn key_moves_read(f: &Body, reads: &dyn Reads) -> bool {
    match f {
        Body::Line | Body::Param(_) => true,
        Body::Node(id) => reads.key_moves(*id),
        other => crate::closed_form::children(other)
            .iter()
            .any(|p| key_moves_read(&p.body, reads)),
    }
}

/// `exp(q)` with the square completed: a real width and centre, the rest an exponential.
pub struct Squared {
    pub a: f64,
    pub mu: f64,
    pub omega: f64,
    pub amplitude: C64,
}

pub fn completed_square(f: &Body) -> Option<Squared> {
    completed_square_read(f, &Opaque)
}

pub(crate) fn completed_square_read(f: &Body, reads: &dyn Reads) -> Option<Squared> {
    let [a0, a1, a2] = polynomial_read(f, Reading::Free, reads)?[..] else {
        return None;
    };
    let (a0, a1, a2) = (a0.exact()?, a1.exact()?, a2.exact()?);
    if !a2.is_real() || a2.re >= 0.0 {
        return None;
    }
    let a = -a2.re;
    let mu = a1.re / (2.0 * a);
    Some(Squared {
        a,
        mu,
        omega: a1.im,
        amplitude: (a0 + C64::real(a * mu * mu)).exp(),
    })
}

pub fn is_squared(f: &Body) -> bool {
    polynomial(f).is_some_and(|p| p.len() == MAX_DEGREE + 1)
}

/// Where a unary lands a subterm sitting on `of`. A root and a logarithm leave the real
/// line on a negative argument, whose sign this judgment does not hold; the rest do not.
fn unary_axis(op: Unary, of: Axis) -> Axis {
    match (op, of) {
        (Unary::Sqrt | Unary::Log, _) => Axis::Any,
        (_, Axis::Real) => Axis::Real,
        _ => Axis::Any,
    }
}

/// Where a whole subterm sits in the complex plane, reading an index or a bare variable as
/// real. `Any` is "could be either", never "is both".
pub fn axis(f: &Body, env: &dyn crate::env::Env) -> Axis {
    axis_read(f, env, &Opaque)
}

/// The same, a ref `reads` knows the form of sitting where that form sits.
pub fn axis_read(f: &Body, env: &dyn crate::env::Env, reads: &dyn Reads) -> Axis {
    let of = |p: &crate::closed_form::Part| axis_read(&p.body, env, reads);
    let axis = |f: &Body| axis_read(f, env, reads);
    match f {
        Body::Const(c) => Axis::of(*c),
        Body::Index(_) | Body::Line | Body::Keyed { .. } => Axis::Real,
        Body::Node(id) => reads
            .axis(*id)
            .unwrap_or_else(|| Axis::of_codomain(env.node(*id).codomain)),
        Body::Param(id) => Axis::of_codomain(env.param(*id).codomain),
        Body::Add(parts) => parts
            .iter()
            .map(of)
            .reduce(Axis::plus)
            .unwrap_or(Axis::Real),
        Body::Mul(parts) => parts
            .iter()
            .map(of)
            .reduce(Axis::times)
            .unwrap_or(Axis::Real),
        Body::Div(a, b) => of(a).times(of(b)),
        Body::Shift { of: inner, .. } | Body::Crop { of: inner, .. } => axis(&inner.body),
        Body::Fold(..) => Axis::Real,
        Body::Apply(op, inner) => unary_axis(*op, axis(&inner.body)),
        Body::Pow(base, n) => match (axis(&base.body), n % 2 == 0) {
            (Axis::Real, _) => Axis::Real,
            (Axis::Imaginary, true) => Axis::Real,
            (Axis::Imaginary, false) => Axis::Imaginary,
            _ => Axis::Any,
        },
        _ => Axis::Any,
    }
}

fn add(x: &Poly, y: &Poly) -> Poly {
    let mut out = Poly::zeros(x.len.max(y.len));
    for (k, c) in x
        .as_slice()
        .iter()
        .enumerate()
        .chain(y.as_slice().iter().enumerate())
    {
        out.c[k] = out.c[k].add(*c);
    }
    trim(out)
}

fn mul(x: &Poly, y: &Poly) -> Option<Poly> {
    if x.len + y.len > MAX_DEGREE + 2 {
        return None;
    }
    let mut out = Poly::zeros(x.len + y.len - 1);
    for (i, a) in x.as_slice().iter().enumerate() {
        for (j, b) in y.as_slice().iter().enumerate() {
            out.c[i + j] = out.c[i + j].add(a.mul(*b));
        }
    }
    Some(trim(out))
}

/// `x -> x - by`, by the binomial expansion of each term.
fn shift(p: &Poly, by: f64) -> Poly {
    let mut out = Poly::zeros(p.len);
    for (n, c) in p.as_slice().iter().enumerate() {
        let mut binomial = 1.0f64;
        for k in (0..=n).rev() {
            let power = i32::try_from(n - k).unwrap_or(0);
            out.c[k] = out.c[k].add(c.scale(binomial * (-by).powi(power)));
            binomial = binomial * k as f64 / (n - k + 1) as f64;
        }
    }
    trim(out)
}

fn trim(mut p: Poly) -> Poly {
    while p.len > 1 && p.c[p.len - 1].is_zero() {
        p.len -= 1;
    }
    p
}

pub fn step(x: f64) -> f64 {
    match x >= 0.0 {
        true => 1.0,
        false => 0.0,
    }
}
