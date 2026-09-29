// Concern: a written time as an exact rational, and a read's time as `k*t + c` | Non-concern: folding an expression into one (loops.rs) | IO: (f64 or ints) -> Q

use std::cmp::Ordering;

/// An exact rational in lowest terms, `den > 0`. A written decimal is the rational it spells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Q {
    num: i128,
    den: i128,
}

/// Past this either side of a rational, arithmetic stops being exact and folds nothing.
const LIMIT: i128 = 1 << 120;

fn gcd(a: i128, b: i128) -> i128 {
    match b {
        0 => a.abs(),
        b => gcd(b, a % b),
    }
}

impl Q {
    pub const ZERO: Q = Q { num: 0, den: 1 };
    pub const ONE: Q = Q { num: 1, den: 1 };

    pub fn new(num: i128, den: i128) -> Option<Q> {
        if den == 0 {
            return None;
        }
        let g = gcd(num, den).max(1) * den.signum();
        let (num, den) = (num / g, den / g);
        (num.abs() < LIMIT && den < LIMIT).then_some(Q { num, den })
    }

    pub fn int(n: i64) -> Q {
        Q {
            num: i128::from(n),
            den: 1,
        }
    }

    /// The shortest decimal that reads back as `x`.
    pub fn decimal(x: f64) -> Option<Q> {
        if !x.is_finite() {
            return None;
        }
        let text = format!("{x:e}");
        let (mantissa, exp) = text.split_once('e')?;
        let exp: i32 = exp.parse().ok()?;
        let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits: i128 = format!("{whole}{frac}").parse().ok()?;
        let shift = exp - frac.len() as i32;
        let ten = |k: u32| 10i128.checked_pow(k).filter(|p| *p < LIMIT);
        match shift >= 0 {
            true => Q::new(digits.checked_mul(ten(shift as u32)?)?, 1),
            false => Q::new(digits, ten(shift.unsigned_abs())?),
        }
    }

    pub fn num(self) -> i128 {
        self.num
    }

    pub fn den(self) -> i128 {
        self.den
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn is_integer(self) -> bool {
        self.den == 1
    }

    pub fn add(self, o: Q) -> Option<Q> {
        let g = gcd(self.den, o.den);
        let den = (self.den / g).checked_mul(o.den)?;
        let num = self
            .num
            .checked_mul(o.den / g)?
            .checked_add(o.num.checked_mul(self.den / g)?)?;
        Q::new(num, den)
    }

    pub fn neg(self) -> Q {
        Q {
            num: -self.num,
            den: self.den,
        }
    }

    pub fn sub(self, o: Q) -> Option<Q> {
        self.add(o.neg())
    }

    pub fn mul(self, o: Q) -> Option<Q> {
        let (g1, g2) = (gcd(self.num, o.den).max(1), gcd(o.num, self.den).max(1));
        Q::new(
            (self.num / g1).checked_mul(o.num / g2)?,
            (self.den / g2).checked_mul(o.den / g1)?,
        )
    }

    pub fn div(self, o: Q) -> Option<Q> {
        match o.num.signum() {
            0 => None,
            sign => self.mul(Q {
                num: sign * o.den,
                den: o.num.abs(),
            }),
        }
    }

    /// `a mod b` on the floor, as `%` is: its sign is the divisor's.
    pub fn rem(self, o: Q) -> Option<Q> {
        let ratio = self.div(o)?;
        let floor = ratio.num.div_euclid(ratio.den);
        self.sub(o.mul(Q::new(floor, 1)?)?)
    }

    /// Correctly rounded, half to even.
    pub fn to_f64(self) -> f64 {
        let negative = self.num < 0;
        let (n, d) = (self.num.unsigned_abs(), self.den as u128);
        if n == 0 {
            return 0.0;
        }
        let bits = |v: u128| 128 - v.leading_zeros() as i32;
        let mut e = bits(n) - bits(d);
        let at_least = |e: i32| match e >= 0 {
            true => n >= d << e,
            false => n << -e >= d,
        };
        if !at_least(e) {
            e -= 1;
        }
        let (top, unit) = match e >= 0 {
            true => (n, d << e),
            false => (n << -e, d),
        };
        let (mut q, mut r) = (top / unit, top % unit);
        for _ in 0..53 {
            r <<= 1;
            q <<= 1;
            if r >= unit {
                r -= unit;
                q |= 1;
            }
        }
        let (mantissa, half) = (q >> 1, q & 1);
        let round = half == 1 && (r > 0 || mantissa & 1 == 1);
        let value = (mantissa + u128::from(round)) as f64 * 2f64.powi(e - 52);
        if negative { -value } else { value }
    }
}

impl PartialOrd for Q {
    fn partial_cmp(&self, o: &Q) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Q {
    fn cmp(&self, o: &Q) -> Ordering {
        fraction_cmp((self.num, self.den), (o.num, o.den))
    }
}

/// By whole parts, then the reciprocals of what is left, so no product overflows.
fn fraction_cmp((a, b): (i128, i128), (c, d): (i128, i128)) -> Ordering {
    let (p, q) = (a.div_euclid(b), c.div_euclid(d));
    if p != q {
        return p.cmp(&q);
    }
    match (a.rem_euclid(b), c.rem_euclid(d)) {
        (0, 0) => Ordering::Equal,
        (0, _) => Ordering::Less,
        (_, 0) => Ordering::Greater,
        (r, s) => fraction_cmp((d, s), (b, r)),
    }
}

/// A read's time `scale * t + shift`, exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Affine {
    pub scale: Q,
    pub shift: Q,
}

impl Affine {
    pub const NOW: Affine = Affine {
        scale: Q::ONE,
        shift: Q::ZERO,
    };
}

/// Sample `m` of a node stands at `step*m + phase` samples of `rate`, `step > 0` and
/// `0 <= phase < step`: the render's own grid, or the one a reader's time asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Grid {
    pub rate: u32,
    pub step: Q,
    pub phase: Q,
}

impl Grid {
    pub fn of(rate: u32) -> Grid {
        Grid {
            rate,
            step: Q::ONE,
            phase: Q::ZERO,
        }
    }

    pub fn is_rate(self) -> bool {
        self == Grid::of(self.rate)
    }

    fn rate_q(self) -> Q {
        Q::int(i64::from(self.rate))
    }

    pub fn steps(self, n: Q) -> Option<Q> {
        n.mul(self.step)?.div(self.rate_q())
    }

    pub fn steps_f64(self, n: f64) -> f64 {
        n * self.step.to_f64() / f64::from(self.rate)
    }

    /// The grid a read at `time` steps its source on: the step scaled by `time`'s, and the
    /// phase its instants land at.
    pub fn read(self, time: Affine) -> Option<Grid> {
        let k = time.scale;
        let step = match k.is_zero() {
            true => self.step,
            false => Q::new(k.num.abs(), k.den)?.mul(self.step)?,
        };
        let at = k.mul(self.phase)?.add(time.shift.mul(self.rate_q())?)?;
        Some(Grid {
            step,
            phase: at.rem(step)?,
            ..self
        })
    }

    /// Sample `n` of this grid reads `time` at sample `(a*n + b)/d` of `on`, exactly.
    pub fn position(self, time: Affine, on: Grid) -> Option<(i128, i128, i128)> {
        let a = time.scale.mul(self.step)?.div(on.step)?;
        let b = time
            .scale
            .mul(self.phase)?
            .add(time.shift.mul(self.rate_q())?)?
            .sub(on.phase)?
            .div(on.step)?;
        let d = a.den.checked_mul(b.den)? / gcd(a.den, b.den);
        Some((
            a.num.checked_mul(d / a.den)?,
            b.num.checked_mul(d / b.den)?,
            d,
        ))
    }

    pub fn map(self, time: Affine, on: Grid) -> Option<sva_samples::Map> {
        let (a, b, d) = self.position(time, on)?;
        sva_samples::Map::new(a, b, d)
    }

    pub fn samples(self) -> sva_samples::Grid {
        let (step, phase) = (self.step, self.phase);
        let d = step.den / gcd(step.den, phase.den) * phase.den;
        sva_samples::Grid {
            rate: self.rate,
            a: step.num * (d / step.den),
            b: phase.num * (d / phase.den),
            d,
        }
    }
}
