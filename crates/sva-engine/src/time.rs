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

    /// Reader sample `n` on a lattice of `reader` a second lands on source sample
    /// `(a*n + b)/d` of a lattice of `source` a second.
    pub fn map(self, reader: u32, source: u32) -> Option<sva_samples::Map> {
        let scale = self
            .scale
            .mul(Q::int(i64::from(source)))?
            .div(Q::int(i64::from(reader)))?;
        let shift = self.shift.mul(Q::int(i64::from(source)))?;
        let d = scale.den.checked_mul(shift.den)? / gcd(scale.den, shift.den);
        let a = scale.num.checked_mul(d / scale.den)?;
        let b = shift.num.checked_mul(d / shift.den)?;
        sva_samples::Map::new(a, b, d)
    }
}
