// Concern: declares SpectralAtom, its six factors, its derivative and its sort key | Non-concern: multiplying two (product.rs), building one from a Body (build.rs) | IO: (an atom) -> its key

use crate::closed_form::Edge;
use crate::complex::{C64, canonical};
use crate::origin::Origin;
use crate::refusal::Factor;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exp {
    pub sigma: f64,
    pub omega: f64,
    /// `e^{sigma*(t - mu)}`: the growing half is read from a reference point, so a factor
    /// bounded on its own window is not held as an overflow times an underflow.
    pub mu: f64,
}

impl Exp {
    pub fn at(sigma: f64, omega: f64) -> Exp {
        Exp {
            sigma,
            omega,
            mu: 0.0,
        }
    }

    /// The same factor written from the origin, or nothing where that overflows.
    pub fn flattened(self) -> Option<(Exp, C64)> {
        if self.mu == 0.0 || self.sigma == 0.0 {
            return Some((Exp::at(self.sigma, self.omega), C64::ONE));
        }
        let carried = C64::real(-self.sigma * self.mu).exp();
        carried
            .is_finite()
            .then_some((Exp::at(self.sigma, self.omega), carried))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gauss {
    pub a: f64,
    pub mu: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indicator {
    pub l: Edge,
    pub r: Edge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pole {
    pub at: C64,
    pub order: u16,
    pub pv: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Singular {
    Regular,
    Delta { at: f64, order: u16 },
}

/// The five smooth factors beside the amplitude, absent where `None`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Factors {
    pub poly: u16,
    pub exp: Option<Exp>,
    pub gauss: Option<Gauss>,
    pub ind: Option<Indicator>,
    pub pole: Option<Pole>,
}

#[derive(Clone, Copy, Debug)]
pub struct SpectralAtom {
    pub c: C64,
    pub poly: u16,
    pub exp: Option<Exp>,
    pub gauss: Option<Gauss>,
    pub ind: Option<Indicator>,
    pub pole: Option<Pole>,
    pub sing: Singular,
    pub origin: Origin,
}

/// Every field but `c` and `origin`. Equal keys are like atoms, and the hash agrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpectralAtomKey {
    sing: (u8, u64, u16),
    pole: (u8, u64, u64, u16, bool),
    ind: (u8, Edge, Edge),
    gauss: (u8, u64, u64),
    poly: u16,
    exp: (u8, u64, u64, u64),
}

/// A real exponential beside a Gaussian is that Gaussian recentred: one spelling survives.
fn absorb_shift(atom: SpectralAtom) -> SpectralAtom {
    let (Some(g), Some(e)) = (atom.gauss, atom.exp) else {
        return atom;
    };
    if e.sigma == 0.0 || e.mu != 0.0 {
        return atom;
    }
    SpectralAtom {
        c: atom
            .c
            .scale((e.sigma * g.mu + e.sigma * e.sigma / (4.0 * g.a)).exp()),
        gauss: Some(Gauss {
            mu: g.mu + e.sigma / (2.0 * g.a),
            ..g
        }),
        exp: (e.omega != 0.0).then_some(Exp::at(0.0, e.omega)),
        ..atom
    }
}

/// `origin` is excluded, so two spellings written in different files are one atom.
impl PartialEq for SpectralAtom {
    fn eq(&self, other: &SpectralAtom) -> bool {
        self.c == other.c && self.key() == other.key()
    }
}

impl Indicator {
    pub const ALL: Indicator = Indicator {
        l: Edge::NegInf,
        r: Edge::PosInf,
    };

    /// Half-open, per FORMAT 3.1: two windows that abut share no instant, so a signal cut
    /// into abutting segments sums to itself rather than to a doubled seam.
    pub fn contains(self, x: f64) -> bool {
        self.l.value() <= x && x < self.r.value()
    }

    pub fn is_empty(self) -> bool {
        self.l.value() >= self.r.value()
    }

    /// Two windows meet in one window, which is why a crop of a crop is one atom.
    pub fn meet(self, other: Indicator) -> Indicator {
        Indicator {
            l: self.l.max(other.l),
            r: self.r.min(other.r),
        }
    }
}

impl Factors {
    pub const NONE: Factors = Factors {
        poly: 0,
        exp: None,
        gauss: None,
        ind: None,
        pole: None,
    };

    pub fn poly(n: u16) -> Factors {
        Factors {
            poly: n,
            ..Factors::NONE
        }
    }
}

impl SpectralAtom {
    /// Folds every absent factor to `None`, and reduces a zeroth-order delta at its point.
    pub fn new(c: C64, factors: Factors, sing: Singular, origin: Origin) -> SpectralAtom {
        assert!(c.is_finite(), "an atom's amplitude must be finite");
        let atom = SpectralAtom {
            c,
            poly: factors.poly,
            exp: factors
                .exp
                .filter(|e| e.sigma != 0.0 || e.omega != 0.0)
                .map(|e| match e.sigma == 0.0 {
                    true => Exp::at(0.0, e.omega),
                    false => e,
                }),
            gauss: factors.gauss,
            ind: factors.ind.filter(|i| *i != Indicator::ALL),
            pole: factors.pole.filter(|p| p.order > 0),
            sing: Singular::Regular,
            origin,
        };
        if let Some(e) = atom.exp {
            assert!(e.sigma.is_finite() && e.omega.is_finite() && e.mu.is_finite());
        }
        if let Some(g) = atom.gauss {
            assert!(g.a.is_finite() && g.a > 0.0 && g.mu.is_finite());
        }
        if let Some(p) = atom.pole {
            assert!(p.at.is_finite());
        }
        let atom = absorb_shift(atom);
        match sing {
            Singular::Regular => atom,
            Singular::Delta { at, order } => {
                assert!(at.is_finite());
                let weight = match order {
                    0 => atom.smooth_at(at).unwrap_or(C64::ZERO),
                    _ => {
                        assert!(
                            atom.is_bare(),
                            "a delta derivative reduces by Leibniz before it reaches an atom"
                        );
                        c
                    }
                };
                SpectralAtom {
                    c: weight,
                    poly: 0,
                    exp: None,
                    gauss: None,
                    ind: None,
                    pole: None,
                    sing: Singular::Delta {
                        at: f64::from_bits(canonical(at)),
                        order,
                    },
                    origin,
                }
            }
        }
    }

    pub fn constant(c: C64, origin: Origin) -> SpectralAtom {
        SpectralAtom::new(c, Factors::NONE, Singular::Regular, origin)
    }

    pub fn is_bare(&self) -> bool {
        self.poly == 0
            && self.exp.is_none()
            && self.gauss.is_none()
            && self.ind.is_none()
            && self.pole.is_none()
    }

    pub fn is_delta(&self) -> bool {
        !matches!(self.sing, Singular::Regular)
    }

    pub fn factors(&self) -> Factors {
        Factors {
            poly: self.poly,
            exp: self.exp,
            gauss: self.gauss,
            ind: self.ind,
            pole: self.pole,
        }
    }

    pub fn with(&self, c: C64, factors: Factors) -> SpectralAtom {
        SpectralAtom::new(c, factors, self.sing, self.origin)
    }

    pub fn smooth_at(&self, x: f64) -> Option<C64> {
        if self.ind.is_some_and(|i| !i.contains(x)) {
            return Some(C64::ZERO);
        }
        let mut v = self.c;
        if self.poly > 0 {
            v = v * C64::real(x).powi(u32::from(self.poly));
        }
        if let Some(e) = self.exp {
            v = v * C64::new(e.sigma * (x - e.mu), e.omega * x).exp();
        }
        if let Some(g) = self.gauss {
            v = v.scale((-g.a * (x - g.mu) * (x - g.mu)).exp());
        }
        if let Some(p) = self.pole {
            let d = C64::real(x) - p.at;
            if d.is_zero() {
                return None;
            }
            v = v / d.powi(u32::from(p.order));
        }
        Some(v)
    }

    /// The product rule over the six factors; the indicator's two terms are edge deltas.
    pub fn derivative(&self) -> Vec<SpectralAtom> {
        if let Singular::Delta { at, order } = self.sing {
            return vec![SpectralAtom::new(
                self.c,
                Factors::NONE,
                Singular::Delta {
                    at,
                    order: order + 1,
                },
                self.origin,
            )];
        }
        let f = self.factors();
        let mut out = Vec::new();
        if self.poly > 0 {
            out.push(self.with(
                self.c.scale(f64::from(self.poly)),
                Factors {
                    poly: self.poly - 1,
                    ..f
                },
            ));
        }
        if let Some(e) = self.exp {
            out.push(self.with(self.c * C64::new(e.sigma, e.omega), f));
        }
        if let Some(g) = self.gauss {
            out.push(self.with(
                self.c.scale(-2.0 * g.a),
                Factors {
                    poly: self.poly + 1,
                    ..f
                },
            ));
            out.push(self.with(self.c.scale(2.0 * g.a * g.mu), f));
        }
        if let Some(p) = self.pole {
            out.push(self.with(
                self.c.scale(-f64::from(p.order)),
                Factors {
                    pole: Some(Pole {
                        order: p.order + 1,
                        ..p
                    }),
                    ..f
                },
            ));
        }
        if let Some(i) = self.ind {
            let open = SpectralAtom::new(
                self.c,
                Factors { ind: None, ..f },
                Singular::Regular,
                self.origin,
            );
            for (edge, sign) in [(i.l, 1.0), (i.r, -1.0)] {
                let Edge::At(bits) = edge else { continue };
                let at = f64::from_bits(bits);
                let Some(w) = open.smooth_at(at) else {
                    continue;
                };
                out.push(SpectralAtom::new(
                    w.scale(sign),
                    Factors::NONE,
                    Singular::Delta { at, order: 0 },
                    self.origin,
                ));
            }
        }
        out
    }

    /// Every field but `c`, `origin` and the line frequency: atoms alike under this key are
    /// one line spectrum times one common factor. Meaningful only where `sigma` is zero.
    pub fn key_without_line(&self) -> SpectralAtomKey {
        SpectralAtomKey {
            exp: (0, 0, 0, 0),
            ..self.key()
        }
    }

    /// Groups the atoms a piecewise window fold may combine.
    pub fn key_without_window(&self) -> SpectralAtomKey {
        SpectralAtomKey {
            ind: (0, Edge::NegInf, Edge::NegInf),
            ..self.key()
        }
    }

    pub fn key(&self) -> SpectralAtomKey {
        let (sing_tag, sing_at, sing_order) = match self.sing {
            Singular::Regular => (0u8, 0u64, 0u16),
            Singular::Delta { at, order } => (1, canonical(at), order),
        };
        let pole = self.pole.map_or((0u8, 0u64, 0u64, 0u16, false), |p| {
            let (re, im) = p.at.bits();
            (1, re, im, p.order, p.pv)
        });
        let ind = self
            .ind
            .map_or((0u8, Edge::NegInf, Edge::NegInf), |i| (1, i.l, i.r));
        let gauss = self
            .gauss
            .map_or((0u8, 0u64, 0u64), |g| (1, canonical(g.a), canonical(g.mu)));
        let exp = self.exp.map_or((0u8, 0u64, 0u64, 0u64), |e| {
            (1, canonical(e.sigma), canonical(e.omega), canonical(e.mu))
        });
        SpectralAtomKey {
            sing: (sing_tag, sing_at, sing_order),
            pole,
            ind,
            gauss,
            poly: self.poly,
            exp,
        }
    }

    pub fn present(&self) -> Vec<Factor> {
        let mut out = Vec::new();
        if self.poly > 0 {
            out.push(Factor::Polynomial);
        }
        if self.exp.is_some() {
            out.push(Factor::Exponential);
        }
        if self.gauss.is_some() {
            out.push(Factor::Gaussian);
        }
        if self.ind.is_some() {
            out.push(Factor::Indicator);
        }
        match self.pole {
            Some(p) if p.pv => out.push(Factor::PrincipalValue),
            Some(_) => out.push(Factor::Pole),
            None => {}
        }
        if self.is_delta() {
            out.push(Factor::Delta);
        }
        if out.is_empty() {
            out.push(Factor::Amplitude);
        }
        out
    }
}
