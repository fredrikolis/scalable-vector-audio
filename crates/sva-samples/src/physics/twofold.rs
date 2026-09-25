// Concern: sums and products carried to twice f64's precision, the reference a rounding bound is checked against | Non-concern: any bound itself (string_tail.rs) | IO: (f64s) -> a value to about 2^-104

/// `hi + lo`, by Dekker's and Knuth's error-free transforms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Twofold {
    hi: f64,
    lo: f64,
}

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let back = s - a;
    (s, (a - (s - back)) + (b - back))
}

impl Twofold {
    pub(crate) fn of(x: f64) -> Twofold {
        Twofold { hi: x, lo: 0.0 }
    }

    pub(crate) fn value(self) -> f64 {
        self.hi + self.lo
    }

    fn normal(hi: f64, lo: f64) -> Twofold {
        let (hi, lo) = two_sum(hi, lo);
        Twofold { hi, lo }
    }

    pub(crate) fn add(self, b: Twofold) -> Twofold {
        let (s, e) = two_sum(self.hi, b.hi);
        let (t, f) = two_sum(self.lo, b.lo);
        let (s, e) = two_sum(s, e + t);
        Twofold::normal(s, e + f)
    }

    pub(crate) fn sub(self, b: Twofold) -> Twofold {
        self.add(Twofold {
            hi: -b.hi,
            lo: -b.lo,
        })
    }

    pub(crate) fn mul(self, b: Twofold) -> Twofold {
        let p = self.hi * b.hi;
        let e = self.hi.mul_add(b.hi, -p);
        Twofold::normal(p, e + (self.hi * b.lo + self.lo * b.hi))
    }

    pub(crate) fn div(self, b: Twofold) -> Twofold {
        let q = self.hi / b.hi;
        let r = self.sub(b.mul(Twofold::of(q)));
        Twofold::normal(q, r.value() / b.hi)
    }
}
