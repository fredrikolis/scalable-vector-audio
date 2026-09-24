// Concern: the node renderer sva-engine hands this crate | Non-concern: building it (sva-engine lower.rs), running it (mod.rs, ops.rs) | IO: none

use sva_formula::Shape;

use crate::physics::Params;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Sin,
    Cos,
    Exp,
    Sqrt,
    Abs,
    Tanh,
    Log,
    Sat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binary {
    Max,
    Min,
    Mod,
}

/// Every closed form-typed subterm was collapsed to a `Buffer` before this tree was built, so there
/// is no oscillator, no series, no delta and no rate here: `rand` arrives folded.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeRenderer {
    Const(f64),
    Time,
    Buffer {
        id: BufId,
        shift: i64,
    },
    SelfAt {
        steps: u32,
    },
    Add(Vec<NodeRenderer>),
    Mul(Vec<NodeRenderer>),
    Sub(Box<NodeRenderer>, Box<NodeRenderer>),
    Div(Box<NodeRenderer>, Box<NodeRenderer>),
    Pow(Box<NodeRenderer>, Box<NodeRenderer>),
    Map(Unary, Box<NodeRenderer>),
    Zip(Binary, Box<NodeRenderer>, Box<NodeRenderer>),
    /// `x` over the window `[a, b)` with a raised-cosine `rise` and `fall` inside it.
    Crop {
        x: Box<NodeRenderer>,
        a: f64,
        b: f64,
        rise: f64,
        fall: f64,
    },
    Join(Vec<NodeRenderer>),
    Channel {
        x: Box<NodeRenderer>,
        k: usize,
    },
    Filter {
        site: SiteId,
        x: Box<NodeRenderer>,
        cutoff: Box<NodeRenderer>,
        q: Box<NodeRenderer>,
        gain: Box<NodeRenderer>,
    },
    Physics {
        site: SiteId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Site {
    Filter(Shape),
    Physics(Box<Params>),
}

impl From<sva_formula::Unary> for Unary {
    fn from(written: sva_formula::Unary) -> Unary {
        match written {
            sva_formula::Unary::Sin => Unary::Sin,
            sva_formula::Unary::Cos => Unary::Cos,
            sva_formula::Unary::Exp => Unary::Exp,
            sva_formula::Unary::Sqrt => Unary::Sqrt,
            sva_formula::Unary::Abs => Unary::Abs,
            sva_formula::Unary::Tanh => Unary::Tanh,
            sva_formula::Unary::Log => Unary::Log,
            sva_formula::Unary::Sat => Unary::Sat,
        }
    }
}

impl Unary {
    pub fn apply(self, x: f64) -> f64 {
        match self {
            Unary::Sin => x.sin(),
            Unary::Cos => x.cos(),
            Unary::Exp => x.exp(),
            Unary::Sqrt => x.sqrt(),
            Unary::Abs => x.abs(),
            Unary::Tanh => x.tanh(),
            Unary::Log => x.ln(),
            Unary::Sat => x.clamp(-1.0, 1.0),
        }
    }
}

impl Binary {
    pub fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Binary::Max => a.max(b),
            Binary::Min => a.min(b),
            Binary::Mod => a.rem_euclid(b),
        }
    }
}
