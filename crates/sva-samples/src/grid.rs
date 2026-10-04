// Concern: the sample grid: a sample's instant, the sample an instant or edge lands on, spans of samples | Non-concern: what a sample holds | IO: (n or t) -> t or n

/// No pull, stored chunk or machine block crosses a multiple.
pub const BLOCK: i64 = 1 << 12;

pub fn block_end(n: i64) -> i64 {
    (n.div_euclid(BLOCK) + 1).saturating_mul(BLOCK)
}

/// Samples `[start, end)` of the grid, whose sample 0 is t = 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub start: i64,
    pub end: i64,
}

impl Extent {
    /// `i64::MIN` and `i64::MAX` stand for no edge.
    pub const EVERYWHERE: Extent = Extent {
        start: i64::MIN,
        end: i64::MAX,
    };

    pub const NOWHERE: Extent = Extent { start: 0, end: 0 };

    pub fn new(start: i64, end: i64) -> Extent {
        assert!(start <= end, "an extent [{start}, {end}) runs backwards");
        Extent { start, end }
    }

    pub fn from(start: i64) -> Extent {
        Extent::new(start, i64::MAX)
    }

    pub fn is_bounded(&self) -> bool {
        self.start != i64::MIN && self.end != i64::MAX
    }

    pub fn contains(&self, n: i64) -> bool {
        self.start <= n && n < self.end
    }

    pub fn intersect(self, other: Extent) -> Extent {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        match start < end {
            true => Extent { start, end },
            false => Extent::NOWHERE,
        }
    }

    pub fn hull(self, other: Extent) -> Extent {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => Extent {
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            },
        }
    }

    /// Every sample moved `by` later; an unbounded edge stays unbounded.
    pub fn shifted(self, by: i64) -> Extent {
        if self.is_empty() {
            return self;
        }
        let edge = |n: i64| match n {
            i64::MIN | i64::MAX => n,
            n => n.saturating_add(by).clamp(i64::MIN + 1, i64::MAX - 1),
        };
        Extent {
            start: edge(self.start),
            end: edge(self.end),
        }
    }

    pub fn secs(rate: u32, start_secs: f64, end_secs: f64) -> Extent {
        let at = |secs: f64| (secs * f64::from(rate)).round() as i64;
        Extent::new(at(start_secs), at(end_secs))
    }

    pub fn len(&self) -> usize {
        debug_assert!(self.is_bounded(), "an unbounded extent has no length");
        (self.end - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    pub fn start_secs(&self, rate: u32) -> f64 {
        self.start as f64 / f64::from(rate)
    }

    pub fn span_secs(&self, rate: u32) -> f64 {
        self.len() as f64 / f64::from(rate)
    }
}

/// Sample `n` stands at `a*n/d` samples of `rate`, in lowest terms, `a, d > 0`: every grid
/// starts at t = 0, so a sample index is the one clock every read and key counts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Grid {
    pub rate: u32,
    pub a: i128,
    pub d: i128,
}

impl Grid {
    pub const fn of(rate: u32) -> Grid {
        Grid { rate, a: 1, d: 1 }
    }

    pub fn finer(rate: u32, scale: usize) -> Grid {
        Grid {
            rate,
            a: 1,
            d: scale as i128,
        }
    }

    pub fn is_rate(&self) -> bool {
        (self.a, self.d) == (1, 1)
    }

    pub(crate) fn exact(&self, n: i64) -> Option<(i128, i128)> {
        let num = self.a.checked_mul(i128::from(n))?;
        Some((num, self.d.checked_mul(i128::from(self.rate))?))
    }

    /// One quotient: correctly rounded while both integers are under 2^53; past that each
    /// integer rounds once converting and the quotient once more.
    pub fn instant(&self, n: i64) -> f64 {
        if self.is_rate() {
            return n as f64 / f64::from(self.rate);
        }
        let num = self.a.saturating_mul(i128::from(n));
        num as f64 / self.d.saturating_mul(i128::from(self.rate)) as f64
    }

    /// The first sample whose exact instant is at or past `edge` read as the decimal it prints
    /// as: the one rule every crop and indicator edge meets the grid by.
    pub fn first_at(&self, edge: f64) -> i64 {
        let beyond = if edge < 0.0 { i64::MIN } else { i64::MAX };
        if !edge.is_finite() {
            return beyond;
        }
        match self.decimal_steps(edge) {
            Some(n) => i64::try_from(n).unwrap_or(beyond),
            None => self.step_at(edge, Round::Ceil).unwrap_or(beyond),
        }
    }

    /// An edge `first_at` meets at sample `n` exactly, beside `n`'s instant.
    pub fn edge_at(&self, n: i64) -> Option<f64> {
        let t = self.instant(n);
        [t, t.next_down(), t.next_up()]
            .into_iter()
            .find(|edge| self.first_at(*edge) == n)
    }

    /// `ceil(edge * d * rate / a)`, `edge` as the shortest decimal that prints it.
    fn decimal_steps(&self, edge: f64) -> Option<i128> {
        const LIMIT: i128 = 1 << 120;
        let text = format!("{edge:e}");
        let (mantissa, exp) = text.split_once('e')?;
        let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits: i128 = format!("{whole}{frac}").parse().ok()?;
        let shift = exp.parse::<i32>().ok()? - frac.len() as i32;
        let ten = 10i128
            .checked_pow(shift.unsigned_abs())
            .filter(|p| *p < LIMIT)?;
        let (num, den) = match shift >= 0 {
            true => (digits.checked_mul(ten)?, 1),
            false => (digits, ten),
        };
        let top = num.checked_mul(self.d.checked_mul(i128::from(self.rate))?)?;
        let bottom = den.checked_mul(self.a)?;
        Some(top.div_euclid(bottom) + i128::from(top.rem_euclid(bottom) != 0))
    }

    /// Whether sample `n` lies in `[l, r)` by `first_at`, which only a tie of its instant needs.
    pub(crate) fn inside(&self, n: i64, l: f64, r: f64) -> bool {
        let t = self.instant(n);
        let reached = |edge: f64| match t == edge {
            true => n >= self.first_at(edge),
            false => t > edge,
        };
        reached(l) && !reached(r)
    }

    pub fn position(&self, n: i64) -> f64 {
        self.a.saturating_mul(i128::from(n)) as f64 / self.d as f64
    }

    pub fn sr(&self) -> f64 {
        f64::from(self.rate) * self.d as f64 / self.a as f64
    }

    pub fn count(&self, t: f64) -> f64 {
        t * f64::from(self.rate) * self.d as f64 / self.a as f64
    }

    /// The step `t` falls nearest, rounded exactly from `t`'s own binary value; `None` where
    /// that is past what the integers hold.
    pub fn step_at(&self, t: f64, round: Round) -> Option<i64> {
        match self.is_rate() {
            true => rated(t, self.rate, round).or_else(|| self.wide(t, round)),
            false => self.wide(t, round),
        }
    }

    fn wide(&self, t: f64, round: Round) -> Option<i64> {
        #[cfg(test)]
        WIDE.with(|w| w.set(w.get() + 1));
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
        let top = num.checked_mul(self.d.checked_mul(i128::from(self.rate))?)?;
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

/// `t*rate` rounded from the double nearest it and its exact error: halves of `t` times `rate`
/// exactly, summed exactly; the error decides only a sum on a whole or half step. `None` where
/// `wide` may refuse or a product could round.
fn rated(t: f64, rate: u32, round: Round) -> Option<i64> {
    let held = t == 0.0 || (2f64.powi(-74)..2f64.powi(51)).contains(&t.abs());
    if !held || rate >= 1 << 26 {
        return None;
    }
    let r = f64::from(rate);
    let split = 134_217_729.0 * t;
    let hi = split - (split - t);
    let (x, y) = (hi * r, (t - hi) * r);
    let p = x + y;
    let back = p - x;
    let e = (x - (p - back)) + (y - back);
    if p.abs() >= 2f64.powi(51) {
        return None;
    }
    let f = p.floor();
    let k = match round {
        Round::Floor if p == f && e < 0.0 => f - 1.0,
        Round::Floor => f,
        Round::Ceil if p == p.ceil() && e > 0.0 => p + 1.0,
        Round::Ceil => p.ceil(),
        Round::Even => match (p - f).total_cmp(&0.5) {
            std::cmp::Ordering::Less => f,
            std::cmp::Ordering::Greater => f + 1.0,
            std::cmp::Ordering::Equal if e > 0.0 => f + 1.0,
            std::cmp::Ordering::Equal if e < 0.0 => f,
            std::cmp::Ordering::Equal => f + f.rem_euclid(2.0),
        },
    };
    Some(k as i64)
}

#[cfg(test)]
thread_local! {
    pub(crate) static WIDE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Round {
    Even,
    Floor,
    Ceil,
}

#[cfg(test)]
mod tests {
    use super::{Grid, Round, rated};

    /// 0.8333333333333334 s lies past 5/6 s, sample 40000's at 48 kHz, though the doubles tie.
    #[test]
    fn an_edge_in_seconds_tying_an_instant_is_decided_by_its_decimal() {
        let cd = Grid::of(44_100);
        assert_eq!(cd.first_at(0.1), 4410);
        assert!(cd.inside(4410, 0.1, 1.0));
        let grid = Grid::of(48_000);
        let edge = 60.0 / 72.0;
        assert_eq!(grid.instant(40_000), edge);
        assert_eq!(grid.first_at(edge), 40_001);
        assert!(!grid.inside(40_000, edge, 2.0));
        assert!(grid.inside(40_000, 0.0, edge));
    }

    fn instants(rate: u32) -> Vec<f64> {
        let r = f64::from(rate);
        let mut out = vec![0.0, -0.0, 1e-300, -1e-300, 1e300, f64::MIN_POSITIVE];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for k in -2_000i64..2_000 {
            for half in [0.0, 0.5] {
                let at = (k as f64 + half) / r;
                let mut near = at;
                for _ in 0..3 {
                    near = near.next_up();
                    out.push(near);
                }
                near = at;
                for _ in 0..3 {
                    near = near.next_down();
                    out.push(near);
                }
                out.push(at);
                out.push(at * 1e9);
            }
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            out.push(f64::from_bits(seed >> 2) * if seed & 1 == 0 { 1.0 } else { -1.0 });
        }
        out
    }

    #[test]
    fn a_step_rounds_alike_in_doubles_and_in_wide_integers() {
        for rate in [1, 8_000, 44_100, 48_000, 88_200, 96_000, 192_000] {
            let grid = Grid::of(rate);
            for t in instants(rate) {
                for round in [Round::Floor, Round::Ceil, Round::Even] {
                    if let Some(k) = rated(t, rate, round) {
                        assert_eq!(Some(k), grid.wide(t, round), "{t:e} at {rate} {round:?}");
                    }
                }
            }
        }
    }
}
