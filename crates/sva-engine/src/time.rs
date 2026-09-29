// Concern: a written time as an exact rational, a read's time as `k*t + c`, and the whole sample it lands on | Non-concern: folding an expression into one (loops.rs) | IO: (f64 or ints) -> Q, Map

use std::cmp::Ordering;

use sva_samples::Map;

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

pub use sva_samples::Grid;

/// A grid's `a*m/d` as `step*m` samples of its rate, exact, starting at t = 0.
pub trait Lattice: Sized {
    fn step(self) -> Q;
    fn stepping(rate: u32, step: Q) -> Grid;
    fn steps(self, n: Q) -> Option<Q>;
    fn steps_f64(self, n: f64) -> f64;
    /// The grid a read at `time` steps its source on.
    fn speed(self, time: Affine) -> Option<Grid>;
    /// Sample `n` reads sample `(a*n + b)/d` of `on`.
    fn landing(self, time: Affine, on: Grid) -> Option<(i128, i128, i128)>;
    /// Where each sample reads on the `speed` grid, the shift rounded half to even: at most
    /// half a sample of that grid off the written instant, as `moved` states exactly.
    fn snapped(self, time: Affine) -> Option<Map>;
    /// Seconds between `time` and the sample `snapped` reads.
    fn moved(self, time: Affine) -> Option<Q>;
    fn edge(self, edge: f64) -> i64;
}

fn rate_q(g: Grid) -> Q {
    Q::int(i64::from(g.rate))
}

impl Lattice for Grid {
    fn step(self) -> Q {
        Q::new(self.a, self.d).expect("a grid's step is a rational")
    }

    fn stepping(rate: u32, step: Q) -> Grid {
        Grid {
            rate,
            a: step.num,
            d: step.den,
        }
    }

    fn steps(self, n: Q) -> Option<Q> {
        n.mul(self.step())?.div(rate_q(self))
    }

    fn steps_f64(self, n: f64) -> f64 {
        n * self.step().to_f64() / f64::from(self.rate)
    }

    fn speed(self, time: Affine) -> Option<Grid> {
        let k = time.scale;
        let step = match k.is_zero() {
            true => self.step(),
            false => Q::new(k.num.abs(), k.den)?.mul(self.step())?,
        };
        Some(Grid::stepping(self.rate, step))
    }

    fn landing(self, time: Affine, on: Grid) -> Option<(i128, i128, i128)> {
        let a = time.scale.mul(self.step())?.div(on.step())?;
        let b = time.shift.mul(rate_q(self))?.div(on.step())?;
        let d = a.den.checked_mul(b.den)? / gcd(a.den, b.den);
        Some((
            a.num.checked_mul(d / a.den)?,
            b.num.checked_mul(d / b.den)?,
            d,
        ))
    }

    fn snapped(self, time: Affine) -> Option<Map> {
        let on = self.speed(time)?;
        let a = time.scale.num().signum();
        let b = nearest(time.shift.mul(rate_q(self))?.div(on.step())?)?;
        Some(Map::whole(a, b))
    }

    fn moved(self, time: Affine) -> Option<Q> {
        let on = self.speed(time)?;
        let exact = time.shift.mul(rate_q(self))?.div(on.step())?;
        let off = exact.sub(Q::int(nearest(exact)?))?;
        let off = Q::new(off.num().abs(), off.den())?;
        off.mul(on.step())?.div(rate_q(self))
    }

    fn edge(self, edge: f64) -> i64 {
        first_at(self, edge)
    }
}

/// `q` rounded to the nearest integer, ties to even; `None` past `i64`.
pub fn nearest(q: Q) -> Option<i64> {
    let (num, den) = (q.num(), q.den());
    let (floor, rem) = (num.div_euclid(den), num.rem_euclid(den));
    let k = match (2 * rem).cmp(&den) {
        Ordering::Less => floor,
        Ordering::Greater => floor + 1,
        Ordering::Equal => floor + floor.rem_euclid(2),
    };
    i64::try_from(k).ok()
}

/// The first sample whose exact instant `a*n/(d*rate)` is at or past `edge`; an edge no
/// 120-bit decimal spells is read at its binary value.
fn first_at(grid: Grid, edge: f64) -> i64 {
    let beyond = if edge < 0.0 { i64::MIN } else { i64::MAX };
    if edge.is_nan() {
        return beyond;
    }
    if edge.is_infinite() {
        return beyond;
    }
    let decimal = || {
        let steps = Q::decimal(edge)?
            .mul(Q::new(grid.d.checked_mul(i128::from(grid.rate))?, 1)?)?
            .div(Q::new(grid.a, 1)?)?;
        let (num, den) = (steps.num(), steps.den());
        Some(num.div_euclid(den) + i128::from(num.rem_euclid(den) != 0))
    };
    if let Some(n) = decimal() {
        return i64::try_from(n).unwrap_or(beyond);
    }
    grid.step_at(edge, sva_samples::Round::Ceil)
        .unwrap_or(beyond)
}
