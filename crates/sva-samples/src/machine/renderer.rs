// Concern: the node renderer sva-engine hands this crate | Non-concern: building it (sva-engine lower.rs), running it (mod.rs, ops.rs) | IO: none

use sva_formula::Shape;

use crate::collapse::Extent;
use crate::physics::Params;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteId(pub u32);

/// Reader sample `n` reads source position `(a*n + b)/d`, `d > 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Map {
    pub a: i128,
    pub b: i128,
    pub d: i128,
}

impl Map {
    pub const fn shift(b: i64) -> Map {
        Map {
            a: 1,
            b: b as i128,
            d: 1,
        }
    }

    /// Lowest terms, so two spellings of one map are one map.
    pub fn new(a: i128, b: i128, d: i128) -> Option<Map> {
        let g = gcd(gcd(a.abs(), b.abs()), d.abs()).max(1);
        let sign = d.signum();
        let limit = 1i128 << 100;
        let map = Map {
            a: sign * a / g,
            b: sign * b / g,
            d: d.abs() / g,
        };
        (map.d > 0 && map.a.abs() < limit && map.b.abs() < limit && map.d < limit).then_some(map)
    }

    pub fn whole(self) -> bool {
        self.d == 1
    }

    pub fn at(self, n: i64) -> (i64, i128) {
        let num = self.a.saturating_mul(i128::from(n)).saturating_add(self.b);
        let floor = num
            .div_euclid(self.d)
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX));
        (floor as i64, num.rem_euclid(self.d))
    }

    /// Whether some sample reads a position past its own.
    pub fn ahead(self) -> bool {
        self.a != self.d || self.b > 0
    }

    pub fn lead(self, half_width: usize) -> i64 {
        let floor = self.b.div_euclid(self.d);
        let floor = floor.clamp(i128::from(i64::MIN / 2), i128::from(i64::MAX / 2)) as i64;
        match self.whole() {
            true => floor,
            false => floor + half_width as i64,
        }
    }

    /// The source samples read over a reader's `over`.
    pub fn image(self, over: Extent, reach: usize) -> Extent {
        if over.is_empty() {
            return over;
        }
        if self.a == 0 {
            let (floor, rem) = self.at(0);
            return widened(Extent::new(floor, floor.saturating_add(1)), rem != 0, reach);
        }
        let ends = (first(over), last(over));
        let at = |n: Option<i128>| n.map(|n| self.floor_at(n));
        let (lo, hi) = match self.a > 0 {
            true => (at(ends.0), at(ends.1)),
            false => (at(ends.1), at(ends.0)),
        };
        widened(extent(lo, hi.map(|h| h + 1)), !self.whole(), reach)
    }

    /// The reader samples whose reading touches `into`.
    pub fn preimage(self, into: Extent, reach: usize) -> Extent {
        if into.is_empty() {
            return into;
        }
        let into = reached_from(into, !self.whole(), reach);
        if self.a == 0 {
            let (floor, _) = self.at(0);
            return match into.contains(floor) {
                true => Extent::EVERYWHERE,
                false => Extent::NOWHERE,
            };
        }
        let (a, b, d) = (self.a, self.b, self.d);
        let (first, last) = (first(into), last(into));
        let lowest = |m: i128| ceil_div(m * d - b, a);
        let highest = |m: i128| floor_div((m + 1) * d - 1 - b, a);
        let (lo, hi) = match self.a > 0 {
            true => (first.map(lowest), last.map(highest)),
            false => (
                last.map(|m| ceil_div((m + 1) * d - 1 - b, a)),
                first.map(|m| floor_div(m * d - b, a)),
            ),
        };
        extent(lo, hi.map(|h| h + 1))
    }

    fn floor_at(self, n: i128) -> i128 {
        self.a
            .saturating_mul(n)
            .saturating_add(self.b)
            .div_euclid(self.d)
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    match b {
        0 => a,
        b => gcd(b, a % b),
    }
}

fn widened(e: Extent, fractional: bool, reach: usize) -> Extent {
    if !fractional || reach == 0 || e.is_empty() {
        return e;
    }
    let r = reach as i64;
    let start = match e.start {
        i64::MIN => i64::MIN,
        s => s.saturating_sub(r - 1),
    };
    let end = match e.end {
        i64::MAX => i64::MAX,
        e => e.saturating_add(r),
    };
    Extent::new(start, end)
}

/// The floors whose taps reach `e`: a floor `f` touches `f - reach + 1 ..= f + reach`.
fn reached_from(e: Extent, fractional: bool, reach: usize) -> Extent {
    if !fractional || reach == 0 || e.is_empty() {
        return e;
    }
    let r = reach as i64;
    let start = match e.start {
        i64::MIN => i64::MIN,
        s => s.saturating_sub(r),
    };
    let end = match e.end {
        i64::MAX => i64::MAX,
        e => e.saturating_add(r - 1),
    };
    Extent::new(start, end)
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

/// Another node's samples, or this node's own past.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Read(BufId),
    Own,
}

/// A map fixed by the lattices, or a time in seconds each sample; on a `line`, less the
/// sample's own instant.
#[derive(Clone, Debug, PartialEq)]
pub enum At {
    Map(Map),
    Moving {
        per_sec: f64,
        line: bool,
        time: Box<NodeRenderer>,
    },
}

impl At {
    /// `by_delay`, a time spelled `t + r` is read as the sample less a delay, keeping its fraction.
    pub fn moving(per_sec: f64, reader: f64, time: NodeRenderer, by_delay: bool) -> At {
        match time.less_time().filter(|_| by_delay && per_sec == reader) {
            Some(rest) => At::Moving {
                per_sec,
                line: true,
                time: Box::new(rest),
            },
            None => At::Moving {
                per_sec,
                line: false,
                time: Box::new(time),
            },
        }
    }

    pub fn position(line: bool, per_sec: f64, n: i64, time: f64) -> (i64, f64) {
        match line {
            true => (n, time * per_sec),
            false => (0, time * per_sec),
        }
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

/// Every closed form-typed subterm was collapsed to a buffer or inlined before this tree was
/// built, so there is no oscillator, no series and no delta here.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeRenderer {
    Const(f64),
    Time,
    Read {
        slot: Slot,
        at: At,
        half_width: usize,
    },
    Noise(u64),
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

impl NodeRenderer {
    pub fn less_time(&self) -> Option<NodeRenderer> {
        match self {
            NodeRenderer::Time => Some(NodeRenderer::Const(0.0)),
            NodeRenderer::Add(parts) => parts.iter().enumerate().find_map(|(at, p)| {
                let mut rest = parts.clone();
                rest[at] = p.less_time()?;
                Some(NodeRenderer::Add(rest))
            }),
            NodeRenderer::Sub(a, b) => Some(NodeRenderer::Sub(Box::new(a.less_time()?), b.clone())),
            _ => None,
        }
    }

    /// Holds no call site and reads none of its own past, so skipping a sample of it changes
    /// no later one.
    pub fn stateless(&self) -> bool {
        match self {
            NodeRenderer::Filter { .. } | NodeRenderer::Physics { .. } => false,
            NodeRenderer::Read {
                slot: Slot::Own, ..
            } => false,
            NodeRenderer::Read {
                at: At::Moving { time, .. },
                ..
            } => time.stateless(),
            NodeRenderer::Add(set) | NodeRenderer::Mul(set) | NodeRenderer::Join(set) => {
                set.iter().all(NodeRenderer::stateless)
            }
            NodeRenderer::Sub(a, b)
            | NodeRenderer::Div(a, b)
            | NodeRenderer::Pow(a, b)
            | NodeRenderer::Zip(_, a, b) => a.stateless() && b.stateless(),
            NodeRenderer::Map(_, x)
            | NodeRenderer::Crop { x, .. }
            | NodeRenderer::Channel { x, .. } => x.stateless(),
            NodeRenderer::Const(_)
            | NodeRenderer::Time
            | NodeRenderer::Noise(_)
            | NodeRenderer::Read { .. } => true,
        }
    }
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
