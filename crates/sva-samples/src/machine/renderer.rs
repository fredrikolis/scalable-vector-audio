// Concern: the node renderer sva-engine hands this crate | Non-concern: building it (sva-engine lower.rs), running it (mod.rs, ops.rs) | IO: none

use sva_formula::Shape;

use crate::collapse::Extent;
use crate::physics::Params;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteId(pub u32);

/// Which sample of a buffer a reader's sample `n` reads: `scale*n + shift`. A scale other than
/// one reflects or strides the read, and reads ahead of the sample it is taken at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Remap {
    pub scale: i64,
    pub shift: i64,
}

impl Remap {
    pub const fn shift(shift: i64) -> Remap {
        Remap { scale: 1, shift }
    }

    pub fn at(self, n: i64) -> i64 {
        self.scale.saturating_mul(n).saturating_add(self.shift)
    }

    /// Whether some sample reads one not yet written when it is.
    pub fn ahead(self) -> bool {
        self.scale != 1 || self.shift > 0
    }

    /// The samples read over a reader's `over`.
    pub fn image(self, over: Extent) -> Extent {
        if self.scale == 1 || over.is_empty() {
            return over.shifted(self.shift);
        }
        if self.scale == 0 {
            return Extent::new(self.shift, self.shift.saturating_add(1));
        }
        let (s, k) = (i128::from(self.scale), i128::from(self.shift));
        let (first, last) = (first(over), last(over));
        let ends = (first.map(|a| s * a + k), last.map(|b| s * b + k));
        let (lo, hi) = match self.scale > 0 {
            true => ends,
            false => (ends.1, ends.0),
        };
        extent(lo, hi.map(|h| h + 1))
    }

    /// The reader samples that read inside `into`.
    pub fn preimage(self, into: Extent) -> Extent {
        if self.scale == 1 || into.is_empty() {
            return into.shifted(self.shift.saturating_neg());
        }
        if self.scale == 0 {
            return match into.contains(self.shift) {
                true => Extent::EVERYWHERE,
                false => Extent::NOWHERE,
            };
        }
        let (s, k) = (i128::from(self.scale), i128::from(self.shift));
        let (first, last) = (first(into), last(into));
        let (lo, hi) = match self.scale > 0 {
            true => (
                first.map(|a| ceil_div(a - k, s)),
                last.map(|b| floor_div(b - k, s)),
            ),
            false => (
                last.map(|b| ceil_div(b - k, s)),
                first.map(|a| floor_div(a - k, s)),
            ),
        };
        extent(lo, hi.map(|h| h + 1))
    }
}

/// An extent's first and last sample, `None` where that edge is unbounded.
fn first(e: Extent) -> Option<i128> {
    (e.start != i64::MIN).then(|| i128::from(e.start))
}

fn last(e: Extent) -> Option<i128> {
    (e.end != i64::MAX).then(|| i128::from(e.end) - 1)
}

fn floor_div(num: i128, den: i128) -> i128 {
    match den < 0 {
        true => (-num).div_euclid(-den),
        false => num.div_euclid(den),
    }
}

fn ceil_div(num: i128, den: i128) -> i128 {
    -floor_div(-num, den)
}

fn extent(lo: Option<i128>, hi: Option<i128>) -> Extent {
    let clamp = |n: i128| n.clamp(i128::from(i64::MIN + 1), i128::from(i64::MAX - 1)) as i64;
    let (start, end) = (lo.map_or(i64::MIN, clamp), hi.map_or(i64::MAX, clamp));
    match start < end {
        true => Extent::new(start, end),
        false => Extent::NOWHERE,
    }
}

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
    Step,
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
        at: Remap,
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
    /// Zero, stepping nothing, before `from`, where its state starts.
    Filter {
        site: SiteId,
        from: i64,
        x: Box<NodeRenderer>,
        cutoff: Box<NodeRenderer>,
        q: Box<NodeRenderer>,
        gain: Box<NodeRenderer>,
    },
    Physics {
        site: SiteId,
        from: i64,
        args: Vec<NodeRenderer>,
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
            sva_formula::Unary::Step => Unary::Step,
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
            Unary::Step => sva_formula::affine::step(x),
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
