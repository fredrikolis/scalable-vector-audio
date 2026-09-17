// Concern: declares Ty, Held, Codomain, width and the dual property, and the meet | Non-concern: deciding a term's type (infer.rs) | IO: (Ty, Ty) -> Ty

use crate::closed_form::Var;

pub const MAX_WIDTH: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Form(Var),
    Sampled,
    Frames,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codomain {
    Real,
    Complex,
}

/// `dual`: the closed form on the other axis is in A too. Never set on samples or frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ty {
    pub held: Held,
    pub dual: bool,
    pub width: u8,
    pub codomain: Codomain,
    pub rate: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    Domain,
    SamplesInClosedForm,
    Rate,
    Frames,
    Width,
}

impl Codomain {
    pub fn join(self, other: Codomain) -> Codomain {
        match (self, other) {
            (Codomain::Real, Codomain::Real) => Codomain::Real,
            _ => Codomain::Complex,
        }
    }
}

impl Held {
    pub fn is_closed_form(self) -> bool {
        matches!(self, Held::Form(_))
    }
}

impl Ty {
    pub const fn form(var: Var, dual: bool, codomain: Codomain) -> Ty {
        Ty {
            held: Held::Form(var),
            dual,
            width: 1,
            codomain,
            rate: None,
        }
    }

    pub const fn discrete(held: Held, codomain: Codomain) -> Ty {
        Ty {
            held,
            dual: false,
            width: 1,
            codomain,
            rate: None,
        }
    }

    pub fn is_closed_form(self) -> bool {
        self.held.is_closed_form()
    }

    pub fn has_dual(self) -> bool {
        self.dual
    }

    /// One value written two ways, so an expression on `var` reads a dual there.
    pub fn read_on(self, var: Var) -> Ty {
        match self.dual && self.is_closed_form() {
            true => Ty {
                held: Held::Form(var),
                ..self
            },
            false => self,
        }
    }

    /// One axis, a dual only where both have one, the wider width, `Complex` if either is.
    pub fn meet(self, other: Ty) -> Result<Ty, Mismatch> {
        let (held, dual) = match (self.held, other.held) {
            (Held::Form(a), Held::Form(b)) if a == b => (Held::Form(a), self.dual && other.dual),
            (Held::Form(_), Held::Form(_)) => return Err(Mismatch::Domain),
            (a, b) if a == b => (a, false),
            _ => return Err(Mismatch::SamplesInClosedForm),
        };
        let width = match (self.width, other.width) {
            (a, b) if a == b => a,
            (1, b) => b,
            (a, 1) => a,
            _ => return Err(Mismatch::Width),
        };
        let rate = match (self.rate, other.rate) {
            (Some(a), Some(b)) if a != b => return Err(Mismatch::Rate),
            (a, b) => a.or(b),
        };
        Ok(Ty {
            held,
            dual,
            width,
            codomain: self.codomain.join(other.codomain),
            rate,
        })
    }
}
