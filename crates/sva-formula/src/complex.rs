// Concern: the complex amplitude every atom factor is measured in | Non-concern: what an amplitude multiplies (spectral_sum/atom.rs) | IO: (C64, C64) -> C64

use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

/// Bitwise after canonicalizing `-0.0`, so equality is transitive.
impl PartialEq for C64 {
    fn eq(&self, other: &C64) -> bool {
        self.bits() == other.bits()
    }
}
impl Eq for C64 {}

impl C64 {
    pub const ZERO: C64 = C64 { re: 0.0, im: 0.0 };
    pub const ONE: C64 = C64 { re: 1.0, im: 0.0 };
    pub const I: C64 = C64 { re: 0.0, im: 1.0 };

    pub fn new(re: f64, im: f64) -> C64 {
        C64 { re, im }
    }

    pub fn real(re: f64) -> C64 {
        C64 { re, im: 0.0 }
    }

    pub fn bits(self) -> (u64, u64) {
        (canonical(self.re), canonical(self.im))
    }

    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }

    pub fn is_zero(self) -> bool {
        self.bits() == C64::ZERO.bits()
    }

    pub fn is_real(self) -> bool {
        canonical(self.im) == canonical(0.0)
    }

    pub fn conj(self) -> C64 {
        C64::new(self.re, -self.im)
    }

    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn inv(self) -> C64 {
        let d = self.norm_sqr();
        C64::new(self.re / d, -self.im / d)
    }

    pub fn exp(self) -> C64 {
        let r = self.re.exp();
        C64::new(r * self.im.cos(), r * self.im.sin())
    }

    pub fn powi(self, n: u32) -> C64 {
        let mut out = C64::ONE;
        for _ in 0..n {
            out = out * self;
        }
        out
    }

    pub fn scale(self, k: f64) -> C64 {
        C64::new(self.re * k, self.im * k)
    }

    pub fn over(self, k: f64) -> C64 {
        C64::new(self.re / k, self.im / k)
    }
}

pub fn canonical(x: f64) -> u64 {
    if x == 0.0 {
        0f64.to_bits()
    } else {
        x.to_bits()
    }
}

impl Add for C64 {
    type Output = C64;
    fn add(self, o: C64) -> C64 {
        C64::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for C64 {
    type Output = C64;
    fn sub(self, o: C64) -> C64 {
        C64::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for C64 {
    type Output = C64;
    fn mul(self, o: C64) -> C64 {
        C64::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}
impl Div for C64 {
    type Output = C64;
    fn div(self, o: C64) -> C64 {
        let d = o.norm_sqr();
        C64::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
}
impl Neg for C64 {
    type Output = C64;
    fn neg(self) -> C64 {
        C64::new(-self.re, -self.im)
    }
}
