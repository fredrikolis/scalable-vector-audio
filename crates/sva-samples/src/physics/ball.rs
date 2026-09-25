// Concern: a real number held as a float and a radius it provably lies within, and arithmetic that keeps it there | Non-concern: which reals a bound needs (unison_tail.rs) | IO: (Ball, Ball) -> Ball

//! Each result lies within `u |c|` of the exact operation on the centres, so `|x - c| <= r`
//! survives every operation with the radius widened by that and its own rounding. `sin` rests
//! on the platform's being within two ulps.

const U: f64 = f64::EPSILON / 2.0;
/// Covers underflow.
const TINY: f64 = 1e-300;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ball {
    pub(crate) c: f64,
    pub(crate) r: f64,
}

fn widened(r: f64, c: f64) -> f64 {
    (r + 2.0 * U * c.abs()) * (1.0 + 16.0 * U) + TINY
}

impl Ball {
    pub(crate) fn exact(c: f64) -> Ball {
        Ball { c, r: 0.0 }
    }

    pub(crate) fn pi() -> Ball {
        Ball {
            c: std::f64::consts::PI,
            r: U * std::f64::consts::PI,
        }
    }

    pub(crate) fn lo(self) -> f64 {
        (self.c - self.r).next_down()
    }

    pub(crate) fn add(self, b: Ball) -> Ball {
        let c = self.c + b.c;
        Ball {
            c,
            r: widened(self.r + b.r, c),
        }
    }

    pub(crate) fn sub(self, b: Ball) -> Ball {
        self.add(b.neg())
    }

    pub(crate) fn neg(self) -> Ball {
        Ball {
            c: -self.c,
            r: self.r,
        }
    }

    pub(crate) fn mul(self, b: Ball) -> Ball {
        let c = self.c * b.c;
        let r = self.c.abs() * b.r + b.c.abs() * self.r + self.r * b.r;
        Ball {
            c,
            r: widened(r, c),
        }
    }

    pub(crate) fn scale(self, k: f64) -> Ball {
        self.mul(Ball::exact(k))
    }

    /// `None` where zero lies within.
    pub(crate) fn recip(self) -> Option<Ball> {
        let gap = (self.c.abs() - self.r) * (1.0 - 4.0 * U);
        (gap > 0.0).then(|| {
            let c = 1.0 / self.c;
            Ball {
                c,
                r: widened(self.r / self.c.abs() / gap, c),
            }
        })
    }

    pub(crate) fn div(self, b: Ball) -> Option<Ball> {
        b.recip().map(|inv| self.mul(inv))
    }

    pub(crate) fn square(self) -> Ball {
        let c = self.c * self.c;
        let r = 2.0 * self.c.abs() * self.r + self.r * self.r;
        Ball {
            c,
            r: widened(r, c),
        }
    }

    /// `None` where a negative lies within.
    pub(crate) fn sqrt(self) -> Option<Ball> {
        (self.lo() >= 0.0).then(|| {
            let c = self.c.sqrt();
            let r = match c > 0.0 {
                true => (self.r / c).min(self.r.sqrt()),
                false => self.r.sqrt(),
            };
            Ball {
                c,
                r: widened(r, c),
            }
        })
    }

    pub(crate) fn sin(self) -> Ball {
        let c = self.c.sin();
        Ball {
            c,
            r: widened(self.r + 4.0 * U, c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::twofold::Twofold;

    fn balls() -> Vec<Ball> {
        let mut out = Vec::new();
        for (i, c) in [0.3, -1.7e-5, 2.5e3, 1.0 / 3.0, -7.0, 1e-200]
            .iter()
            .enumerate()
        {
            for r in [0.0, 1e-16, 1e-9, 0.25] {
                out.push(Ball {
                    c: *c,
                    r: r * c.abs() * (1.0 + i as f64 / 7.0),
                });
            }
        }
        out
    }

    /// Points half the radius out where that clears the centre's ulp.
    fn corners(b: Ball) -> Vec<f64> {
        match b.r > 8.0 * f64::EPSILON * b.c.abs() {
            true => vec![b.c - b.r / 2.0, b.c, b.c + b.r / 2.0],
            false => vec![b.c],
        }
    }

    fn within(out: Ball, truth: Twofold) -> bool {
        let t = truth.value();
        t >= out.lo() && -t >= out.neg().lo()
    }

    #[test]
    fn every_operation_holds_the_exact_result_of_every_point_within() {
        let t = Twofold::of;
        for a in balls() {
            for b in balls() {
                for x in corners(a) {
                    for y in corners(b) {
                        assert!(within(a.add(b), t(x).add(t(y))), "{a:?} + {b:?}");
                        assert!(within(a.sub(b), t(x).sub(t(y))), "{a:?} - {b:?}");
                        assert!(within(a.mul(b), t(x).mul(t(y))), "{a:?} * {b:?}");
                        if let Some(q) = a.div(b) {
                            assert!(within(q, t(x).div(t(y))), "{a:?} / {b:?}");
                        }
                    }
                }
            }
            for x in corners(a) {
                assert!(within(a.square(), t(x).mul(t(x))), "{a:?}^2");
                if let (Some(root), true) = (a.sqrt(), x > 0.0) {
                    let s = x.sqrt();
                    let fixed = t(s).add(t(x).sub(t(s).mul(t(s))).div(t(2.0 * s)));
                    assert!(within(root, fixed), "sqrt {a:?}");
                }
            }
        }
    }
}
