// Concern: declares ClosedForm and its Body, as written before normalization | Non-concern: the canonical atom sum (spectral_sum/) | IO: none

use crate::complex::C64;
use crate::env::{NodeId, ParamId};
use crate::origin::Origin;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Var {
    T,
    F,
}

impl Var {
    pub fn as_str(self) -> &'static str {
        match self {
            Var::T => "t",
            Var::F => "f",
        }
    }
}

/// One `Var` for the whole of it: `t` and `f` never meet inside one.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedForm {
    pub var: Var,
    pub body: Body,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub origin: Origin,
    pub body: Box<Body>,
}

impl Part {
    pub fn new(origin: Origin, body: Body) -> Part {
        Part {
            origin,
            body: Box::new(body),
        }
    }

    pub fn bare(body: Body) -> Part {
        Part::new(Origin::UNKNOWN, body)
    }
}

/// An infinite bound is a variant, so no sort key or hash ever sees a non-finite `f64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    NegInf,
    At(u64),
    PosInf,
}

/// Numeric: by bit pattern a negative bound sorts above every positive one.
impl Ord for Edge {
    fn cmp(&self, other: &Edge) -> std::cmp::Ordering {
        self.value().total_cmp(&other.value())
    }
}

impl PartialOrd for Edge {
    fn partial_cmp(&self, other: &Edge) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Edge {
    pub fn at(x: f64) -> Edge {
        match x {
            x if x == f64::NEG_INFINITY => Edge::NegInf,
            x if x == f64::INFINITY => Edge::PosInf,
            x => Edge::At(crate::complex::canonical(x)),
        }
    }

    pub fn value(self) -> f64 {
        match self {
            Edge::NegInf => f64::NEG_INFINITY,
            Edge::At(bits) => f64::from_bits(bits),
            Edge::PosInf => f64::INFINITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Sin,
    Cos,
    Exp,
    Tanh,
    Sat,
    Abs,
    Log,
    Sqrt,
}

impl Unary {
    pub fn name(self) -> &'static str {
        match self {
            Unary::Sin => "sin",
            Unary::Cos => "cos",
            Unary::Exp => "exp",
            Unary::Tanh => "tanh",
            Unary::Sat => "sat",
            Unary::Abs => "abs",
            Unary::Log => "log",
            Unary::Sqrt => "sqrt",
        }
    }

    pub const ALL: &'static [Unary] = &[
        Unary::Sin,
        Unary::Cos,
        Unary::Exp,
        Unary::Tanh,
        Unary::Sat,
        Unary::Abs,
        Unary::Log,
        Unary::Sqrt,
    ];

    pub fn from_name(name: &str) -> Option<Unary> {
        Unary::ALL.iter().copied().find(|u| u.name() == name)
    }

    /// A is closed under these three on an affine argument and under nothing else.
    pub fn is_closed(self) -> bool {
        matches!(self, Unary::Sin | Unary::Cos | Unary::Exp)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Max,
    Min,
    Mod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IndexId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Finite(i64),
    Infinite,
}

/// A value, not an expansion: truncated once, at collapse.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub index: IndexId,
    pub lo: i64,
    pub hi: Bound,
    pub term: Part,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mode {
    pub omega: f64,
    pub tau: f64,
    pub amp: f64,
    pub phase: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Excitation {
    HammerPulse { f0: f64, t0: f64, contact: f64 },
    Impulse { t0: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModalBank {
    pub modes: Vec<Mode>,
    pub excite: Excitation,
}

/// Residues expand on demand, never stored, so one rational has one spelling.
#[derive(Clone, Debug, PartialEq)]
pub struct Rational {
    pub zeros: Vec<C64>,
    pub poles: Vec<C64>,
    pub gain: C64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Const(C64),
    Line,
    Index(IndexId),
    /// The unit-interval hash `rand` folds to, and the phase a noise line reads.
    Keyed {
        seed: u64,
        of: Part,
    },
    Param(ParamId),
    Node(NodeId),
    Add(Vec<Part>),
    Mul(Vec<Part>),
    Div(Part, Part),
    Pow(Part, i32),
    Apply(Unary, Part),
    Fold(Fold, Vec<Part>),
    Delta {
        at: Part,
        order: u16,
    },
    Pv(Part),
    Shift {
        by: f64,
        of: Part,
    },
    /// `of` read at the time `at` names, which is a shift only where `at` is `t` plus a
    /// constant.
    Warp {
        at: Part,
        of: Part,
    },
    Deriv {
        order: u16,
        of: Part,
    },
    /// `rise` and `fall` are raised-cosine shoulders inside `[l, r)`; zero is a hard edge.
    Crop {
        of: Part,
        l: Edge,
        r: Edge,
        rise: f64,
        fall: f64,
    },
    Join(Vec<Part>),
    Channel(Part, u8),
    Rational(Rational),
    Series(Box<Series>),
    Modal(ModalBank),
}

/// The subterms of one node: a walker states its recursion once, and every new variant
/// reaches every walker at once.
pub fn children(f: &Body) -> Vec<&Part> {
    match f {
        Body::Const(_)
        | Body::Line
        | Body::Index(_)
        | Body::Param(_)
        | Body::Node(_)
        | Body::Rational(_)
        | Body::Modal(_) => Vec::new(),
        Body::Keyed { of, .. } => vec![of],
        Body::Add(parts) | Body::Mul(parts) | Body::Join(parts) | Body::Fold(_, parts) => {
            parts.iter().collect()
        }
        Body::Div(a, b) | Body::Warp { at: a, of: b } => vec![a, b],
        Body::Pow(of, _)
        | Body::Apply(_, of)
        | Body::Pv(of)
        | Body::Channel(of, _)
        | Body::Delta { at: of, .. }
        | Body::Shift { of, .. }
        | Body::Deriv { of, .. }
        | Body::Crop { of, .. } => vec![of],
        Body::Series(s) => vec![&s.term],
    }
}

pub fn map_children(f: &Body, mut g: impl FnMut(&Part) -> Part) -> Body {
    let one = |p: &Part, g: &mut dyn FnMut(&Part) -> Part| g(p);
    match f {
        Body::Const(_)
        | Body::Line
        | Body::Index(_)
        | Body::Param(_)
        | Body::Node(_)
        | Body::Rational(_)
        | Body::Modal(_) => f.clone(),
        Body::Keyed { seed, of } => Body::Keyed {
            seed: *seed,
            of: one(of, &mut g),
        },
        Body::Add(parts) => Body::Add(parts.iter().map(g).collect()),
        Body::Mul(parts) => Body::Mul(parts.iter().map(g).collect()),
        Body::Join(parts) => Body::Join(parts.iter().map(g).collect()),
        Body::Fold(op, parts) => Body::Fold(*op, parts.iter().map(g).collect()),
        Body::Div(a, b) => Body::Div(g(a), g(b)),
        Body::Warp { at, of } => Body::Warp {
            at: g(at),
            of: g(of),
        },
        Body::Pow(of, n) => Body::Pow(g(of), *n),
        Body::Apply(op, of) => Body::Apply(*op, g(of)),
        Body::Pv(of) => Body::Pv(g(of)),
        Body::Channel(of, k) => Body::Channel(g(of), *k),
        Body::Delta { at, order } => Body::Delta {
            at: g(at),
            order: *order,
        },
        Body::Shift { by, of } => Body::Shift { by: *by, of: g(of) },
        Body::Deriv { order, of } => Body::Deriv {
            order: *order,
            of: g(of),
        },
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } => Body::Crop {
            of: g(of),
            l: *l,
            r: *r,
            rise: *rise,
            fall: *fall,
        },
        Body::Series(s) => Body::Series(Box::new(Series {
            term: g(&s.term),
            ..(**s).clone()
        })),
    }
}

/// `1[l,r)(u - by)` reads the free variable too, so a window moves its own bounds; a warp
/// moves only its time.
pub fn shift_line(f: &Body, by: f64) -> Body {
    let walk = |p: &Part| Part::new(p.origin, shift_line(&p.body, by));
    match f {
        Body::Line => Body::Add(vec![
            Part::bare(Body::Line),
            Part::bare(Body::Const(crate::complex::C64::real(-by))),
        ]),
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } => Body::Crop {
            of: walk(of),
            l: Edge::at(l.value() + by),
            r: Edge::at(r.value() + by),
            rise: *rise,
            fall: *fall,
        },
        Body::Warp { at, of } => Body::Warp {
            at: walk(at),
            of: of.clone(),
        },
        other => map_children(other, walk),
    }
}

/// An edge holds no formula, so a window is warped whole.
pub fn read_at(f: &Body, at: &Body) -> Body {
    match f {
        Body::Line => at.clone(),
        Body::Crop { .. } => Body::Warp {
            at: Part::bare(at.clone()),
            of: Part::bare(f.clone()),
        },
        Body::Warp { at: inner, of } => Body::Warp {
            at: Part::new(inner.origin, read_at(&inner.body, at)),
            of: of.clone(),
        },
        other => map_children(other, |p| Part::new(p.origin, read_at(&p.body, at))),
    }
}

/// A leaf substitution cannot walk into: the free variable hides behind a name.
pub fn shifts_opaquely(f: &Body) -> bool {
    matches!(
        f,
        Body::Node(_) | Body::Param(_) | Body::Rational(_) | Body::Modal(_)
    ) || children(f).iter().any(|p| shifts_opaquely(&p.body))
}
