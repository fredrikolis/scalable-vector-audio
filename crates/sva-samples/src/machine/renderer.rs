// Concern: the node renderer sva-engine hands this crate | Non-concern: building it (sva-engine lower.rs), running it (mod.rs, ops.rs) | IO: none

use sva_formula::{Body, C64, Shape, SpectralSum};

use crate::collapse::Extent;
use crate::error::CollapseError;
use crate::physics::Params;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteId(pub u32);

/// Reader sample `n` reads source sample `(a*n + b)/d`, `d > 0`: exactly, or rounded down or
/// to the nearest, ties to even. An exact map is whole, `d = 1`; nothing reads between samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Map {
    pub a: i128,
    pub b: i128,
    pub d: i128,
    pub between: Between,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Between {
    Exact,
    Floor,
    Even,
}

impl Map {
    pub const fn shift(b: i64) -> Map {
        Map {
            a: 1,
            b: b as i128,
            d: 1,
            between: Between::Exact,
        }
    }

    /// Lowest terms; `None` where a sample lands between two, or past what a map holds.
    pub fn new(a: i128, b: i128, d: i128) -> Option<Map> {
        Map::rounded(a, b, d, Between::Exact)
    }

    /// A rounding every sample takes alike is folded into an exact map.
    pub fn rounded(a: i128, b: i128, d: i128, between: Between) -> Option<Map> {
        let g = gcd(gcd(a.abs(), b.abs()), d.abs()).max(1);
        let sign = d.signum();
        let limit = 1i128 << 100;
        let map = Map {
            a: sign * a / g,
            b: sign * b / g,
            d: d.abs() / g,
            between,
        };
        let held = map.d > 0 && map.a.abs() < limit && map.b.abs() < limit && map.d < limit;
        held.then(|| map.settled())
            .filter(|m| m.d == 1 || m.between != Between::Exact)
    }

    fn settled(self) -> Map {
        let exact = match self.between {
            _ if self.d == 1 => true,
            Between::Exact => false,
            Between::Floor => self.a % self.d == 0,
            Between::Even => {
                let tie = 2 * self.b.rem_euclid(self.d) == self.d;
                self.a % self.d == 0 && (!tie || (self.a / self.d) % 2 == 0)
            }
        };
        match exact {
            true => Map {
                a: self.a / self.d,
                b: self.index_at(0),
                d: 1,
                between: Between::Exact,
            },
            false => self,
        }
    }

    pub fn at(self, n: i64) -> i64 {
        let clamp = |k: i128| k.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        clamp(self.index_at(i128::from(n)))
    }

    pub fn ahead(self) -> bool {
        self.a != self.d || self.lead() > 0
    }

    pub fn lead(self) -> i64 {
        let most = match self.between {
            Between::Exact | Between::Floor => self.b.div_euclid(self.d),
            Between::Even => (2 * self.b + self.d).div_euclid(2 * self.d),
        };
        most.clamp(i128::from(i64::MIN / 2), i128::from(i64::MAX / 2)) as i64
    }

    pub fn least(self) -> i64 {
        let least = match self.between {
            Between::Exact | Between::Floor => self.b.div_euclid(self.d),
            Between::Even => (2 * self.b + self.d - 1).div_euclid(2 * self.d),
        };
        least.clamp(i128::from(i64::MIN / 2), i128::from(i64::MAX / 2)) as i64
    }

    pub fn image(self, over: Extent) -> Extent {
        if over.is_empty() {
            return over;
        }
        if self.a == 0 {
            let at = self.at(0);
            return Extent::new(at, at.saturating_add(1));
        }
        let ends = (first(over), last(over));
        let at = |n: Option<i128>| n.map(|n| self.index_at(n));
        let (lo, hi) = match self.a > 0 {
            true => (at(ends.0), at(ends.1)),
            false => (at(ends.1), at(ends.0)),
        };
        extent(lo, hi.map(|h| h + 1))
    }

    /// The reader samples whose reading lands in `into`. Rounding to even reads at most one
    /// below rounding half up.
    pub fn preimage(self, into: Extent) -> Extent {
        if into.is_empty() {
            return into;
        }
        if self.between == Between::Even {
            let up = Map {
                a: 2 * self.a,
                b: 2 * self.b + self.d,
                d: 2 * self.d,
                between: Between::Floor,
            };
            let end = match into.end {
                i64::MAX => i64::MAX,
                e => e.saturating_add(1),
            };
            return up.preimage(Extent::new(into.start, end));
        }
        if self.a == 0 {
            return match into.contains(self.at(0)) {
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

    fn index_at(self, n: i128) -> i128 {
        let num = self.a.saturating_mul(n).saturating_add(self.b);
        let (floor, rem) = (num.div_euclid(self.d), num.rem_euclid(self.d));
        match self.between {
            Between::Exact | Between::Floor => floor,
            Between::Even => match (2 * rem).cmp(&self.d) {
                std::cmp::Ordering::Less => floor,
                std::cmp::Ordering::Greater => floor + 1,
                std::cmp::Ordering::Equal => floor + floor.rem_euclid(2),
            },
        }
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    match b {
        0 => a,
        b => gcd(b, a % b),
    }
}

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

/// Sample `n` stands at `(a*n + b)/d` samples of `rate`, in lowest terms, `a, d > 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Grid {
    pub rate: u32,
    pub a: i128,
    pub b: i128,
    pub d: i128,
}

impl Grid {
    pub const fn of(rate: u32) -> Grid {
        Grid {
            rate,
            a: 1,
            b: 0,
            d: 1,
        }
    }

    pub fn is_rate(&self) -> bool {
        (self.a, self.b, self.d) == (1, 0, 1)
    }

    fn exact(&self, n: i64) -> Option<(i128, i128)> {
        let num = self.a.checked_mul(i128::from(n))?.checked_add(self.b)?;
        Some((num, self.d.checked_mul(i128::from(self.rate))?))
    }

    /// One quotient: correctly rounded while both integers are under 2^53; past that each
    /// integer rounds once converting and the quotient once more.
    pub fn instant(&self, n: i64) -> f64 {
        let num = self.a.saturating_mul(i128::from(n)).saturating_add(self.b);
        num as f64 / self.d.saturating_mul(i128::from(self.rate)) as f64
    }

    pub fn stepped(&self, n: i64) -> f64 {
        self.position(n) * (1.0 / f64::from(self.rate))
    }

    pub fn position(&self, n: i64) -> f64 {
        self.a.saturating_mul(i128::from(n)).saturating_add(self.b) as f64 / self.d as f64
    }

    pub fn sr(&self) -> f64 {
        f64::from(self.rate) * self.d as f64 / self.a as f64
    }

    pub fn count(&self, t: f64) -> f64 {
        (t * f64::from(self.rate) * self.d as f64 - self.b as f64) / self.a as f64
    }

    /// The step `t` falls nearest, rounded exactly from `t`'s own binary value; `None` where
    /// that is past what the integers hold.
    pub fn step_at(&self, t: f64, round: Round) -> Option<i64> {
        if !t.is_finite() {
            return None;
        }
        let bits = t.abs().to_bits();
        let (exp, frac) = ((bits >> 52) as i32, (bits & ((1 << 52) - 1)) as i128);
        let (mantissa, shift) = match exp {
            0 => (frac, 1074),
            e => (frac | (1 << 52), 1075 - e),
        };
        let zeros = mantissa.trailing_zeros().min(127) as i32;
        let (mantissa, shift) = match mantissa {
            0 => (0, 0),
            m => (m >> zeros, shift - zeros),
        };
        let mantissa = if t < 0.0 { -mantissa } else { mantissa };
        let (num, den) = match shift {
            s if s <= 0 => (mantissa.checked_mul(1i128.checked_shl((-s) as u32)?)?, 1),
            s if s < 127 => (mantissa, 1i128 << s),
            _ => return None,
        };
        let top = num
            .checked_mul(self.d.checked_mul(i128::from(self.rate))?)?
            .checked_sub(self.b.checked_mul(den)?)?;
        let bottom = self.a.checked_mul(den)?;
        let (floor, rem) = (top.div_euclid(bottom), top.rem_euclid(bottom));
        let k = match round {
            Round::Floor => floor,
            Round::Ceil => floor + i128::from(rem != 0),
            Round::Even => match (2 * rem).cmp(&bottom) {
                std::cmp::Ordering::Less => floor,
                std::cmp::Ordering::Greater => floor + 1,
                std::cmp::Ordering::Equal => floor + floor.rem_euclid(2),
            },
        };
        i64::try_from(k).ok()
    }
}

/// `scale*t + shift + gain*((inner_scale*t + inner_shift) mod period)` at sample `n`'s
/// instant, each a rational `(num, den)` with `den > 0` and `period > 0`. The remainder and
/// the sum are integers over one denominator, so which side of a jump an instant falls on is
/// decided exactly and only the quotient that states the sum rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Wrap {
    pub scale: (i128, i128),
    pub shift: (i128, i128),
    pub gain: (i128, i128),
    pub inner: [(i128, i128); 2],
    pub period: (i128, i128),
}

impl Wrap {
    /// `None` where the integers it is computed in would overflow.
    pub fn at(self, n: i64, grid: Grid) -> Option<f64> {
        let (n, rate) = grid.exact(n)?;
        let [(s, sd), (o, od)] = self.inner;
        let over = sd.checked_mul(rate)?;
        let den = lcm(lcm(over, od)?, self.period.1)?;
        let x = s
            .checked_mul(n)?
            .checked_mul(den / over)?
            .checked_add(o.checked_mul(den / od)?)?;
        let rem = x.rem_euclid(self.period.0.checked_mul(den / self.period.1)?);
        let line = self.scale.1.checked_mul(rate)?;
        let wrapped = self.gain.1.checked_mul(den)?;
        let whole = lcm(lcm(line, self.shift.1)?, wrapped)?;
        let sum = self
            .scale
            .0
            .checked_mul(n)?
            .checked_mul(whole / line)?
            .checked_add(self.shift.0.checked_mul(whole / self.shift.1)?)?
            .checked_add(self.gain.0.checked_mul(rem)?.checked_mul(whole / wrapped)?)?;
        Some(sum as f64 / whole as f64)
    }

    /// The largest magnitude it takes over instants no later than `t`.
    pub fn most(self, t: f64) -> f64 {
        let q = |(num, den): (i128, i128)| num as f64 / den as f64;
        q(self.scale).abs() * t.abs() + q(self.shift).abs() + q(self.gain).abs() * q(self.period)
    }
}

fn lcm(a: i128, b: i128) -> Option<i128> {
    (a / gcd(a, b)).checked_mul(b)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Round {
    Even,
    Floor,
    Ceil,
}

/// Another node's samples, or this node's own past.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Read(BufId),
    Own,
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

/// What the machine evaluates at an instant it computes, a closed form truncated to the band
/// or the noise.
#[derive(Clone, Debug, PartialEq)]
pub enum Formula {
    Sum(Box<SpectralSum>),
    Written(Box<Body>),
    Drawn { seed: u64, rate: u32 },
}

impl Formula {
    pub fn at(&self, component: usize, t: f64) -> Result<f64, CollapseError> {
        let value: C64 = match self {
            Formula::Drawn { seed, rate } => {
                return Ok(sva_formula::draw(*seed, t * f64::from(*rate)));
            }
            Formula::Sum(sum) => crate::collapse::eval_spectral_sum_at(sum, component, t)?,
            Formula::Written(body) => {
                crate::collapse::eval_written_at(body, component, t, &crate::collapse::NoRefs)?
            }
        };
        Ok(value.re)
    }

    /// One operation per atom or written subterm.
    pub fn ops(&self) -> usize {
        fn terms(body: &Body) -> usize {
            1 + sva_formula::closed_form::children(body)
                .iter()
                .map(|p| terms(&p.body))
                .sum::<usize>()
        }
        match self {
            Formula::Sum(sum) => sum.atoms().count().max(1),
            Formula::Written(body) => terms(body),
            Formula::Drawn { .. } => 1,
        }
    }
}

/// Every closed form-typed subterm was collapsed to a buffer, inlined or held as a formula
/// before this tree was built, so there is no oscillator, no series and no delta here.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeRenderer {
    Const(f64),
    Time,
    Wrap(Wrap),
    Read {
        slot: Slot,
        map: Map,
    },
    /// A closed form at the instant `time` names, `width` components wide.
    Formula {
        formula: Formula,
        width: usize,
        time: Box<NodeRenderer>,
    },
    Noise(u64),
    /// The stored sample nearest the instant `time` names, moved by `plus`; `reach` holds
    /// every offset from the sample being written it lands at.
    Nearest {
        slot: Slot,
        time: Box<NodeRenderer>,
        round: Round,
        plus: i64,
        reach: (i64, i64),
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

impl NodeRenderer {
    /// Holds no call site and reads none of its own past, so skipping a sample of it changes
    /// no later one.
    pub fn stateless(&self) -> bool {
        match self {
            NodeRenderer::Filter { .. } | NodeRenderer::Physics { .. } => false,
            NodeRenderer::Read {
                slot: Slot::Own, ..
            } => false,
            NodeRenderer::Formula { time, .. } => time.stateless(),
            NodeRenderer::Nearest {
                slot: Slot::Own, ..
            } => false,
            NodeRenderer::Nearest { time, .. } => time.stateless(),
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
            | NodeRenderer::Wrap(_)
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
