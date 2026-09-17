// Concern: the atom classes a written closed form reaches, and whether each has a dual in A | Non-concern: assembling a Ty around them (infer.rs) | IO: (two Algs) -> one Alg

use crate::affine::{Axis, Coeff};
use crate::closed_form::Unary;
use crate::table::class::{AtomClass, Factors, Growth, dual_class};

/// The atom classes `normalize` would reach, abstracted from their parameters.
#[derive(Clone, Debug)]
pub struct Alg {
    pub classes: Vec<AtomClass>,
    pub constant: bool,
    pub escaped: bool,
}

impl Alg {
    pub fn atom(class: AtomClass) -> Alg {
        Alg {
            classes: vec![class],
            constant: false,
            escaped: false,
        }
    }

    pub fn scalar() -> Alg {
        Alg {
            classes: vec![AtomClass::regular()],
            constant: true,
            escaped: false,
        }
    }

    pub fn gone() -> Alg {
        Alg {
            classes: Vec::new(),
            constant: false,
            escaped: true,
        }
    }

    pub fn in_a(&self) -> bool {
        !self.escaped && self.classes.iter().all(|c| dual_class(*c).is_ok())
    }

    pub fn union(self, other: Alg) -> Alg {
        Alg {
            classes: [self.classes, other.classes].concat(),
            constant: self.constant && other.constant,
            escaped: self.escaped || other.escaped,
        }
    }

    /// `SpectralAtom::times` read on classes: factor sets unite, orders add, indicators bound.
    pub fn product(self, other: Alg) -> Alg {
        let mut classes = Vec::new();
        for a in &self.classes {
            for b in &other.classes {
                classes.push(AtomClass {
                    factors: Factors {
                        poly: a.factors.poly || b.factors.poly,
                        exp: a.factors.exp || b.factors.exp,
                        gauss: a.factors.gauss || b.factors.gauss,
                        ind: a.factors.ind || b.factors.ind,
                        pole: a.factors.pole || b.factors.pole,
                        pv: a.factors.pv || b.factors.pv,
                        delta: a.factors.delta || b.factors.delta,
                    },
                    growth: a.growth.join(b.growth),
                    bounded: (a.bounded.0 || b.bounded.0, a.bounded.1 || b.bounded.1),
                    pole_order: a.pole_order + b.pole_order,
                });
            }
        }
        Alg {
            classes,
            constant: self.constant && other.constant,
            escaped: self.escaped || other.escaped,
        }
    }

    pub fn bound(mut self, left: bool, right: bool) -> Alg {
        for c in &mut self.classes {
            c.factors.ind = true;
            c.bounded = (c.bounded.0 || left, c.bounded.1 || right);
        }
        self
    }
}

/// `exp` runs away along the slope's real part, `sin` and `cos` along its imaginary one.
pub fn exponent_growth(op: Unary, slope: Coeff) -> Growth {
    let along = match op {
        Unary::Exp => Axis::Real,
        _ => Axis::Imaginary,
    };
    match slope {
        Coeff::Exact(a) if op == Unary::Exp => Growth::of_sigma(a.re),
        Coeff::Exact(a) => Growth::of_sigma(-a.im),
        Coeff::Unknown(axis) if axis != along && axis != Axis::Any => Growth::Tempered,
        Coeff::Unknown(_) => Growth::GrowsBoth,
    }
}
