// Concern: declares SpectralSum and Lane, the canonical form of a closed form | Non-concern: producing one (build.rs) | IO: none

pub mod atom;
pub mod build;
pub mod image;
pub mod merge;
pub mod product;
pub mod spread;
pub mod sup;

use crate::closed_form::{ModalBank, Series, Var};
use atom::SpectralAtom;

/// The infinite two sit beside the atoms because they carry their own truncation.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Lane {
    pub atoms: Vec<SpectralAtom>,
    pub series: Vec<Series>,
    pub modal: Vec<ModalBank>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectralSum {
    pub var: Var,
    pub lanes: Vec<Lane>,
}

impl Lane {
    pub fn of(atoms: Vec<SpectralAtom>) -> Lane {
        Lane {
            atoms,
            ..Lane::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty() && self.series.is_empty() && self.modal.is_empty()
    }

    /// Finitely many atoms is what every product rule is written over.
    pub fn is_finite_sum(&self) -> bool {
        self.series.is_empty() && self.modal.is_empty()
    }

    pub fn expanded(mut self) -> Lane {
        for bank in std::mem::take(&mut self.modal) {
            self.atoms
                .extend(crate::modal::atoms(&bank, crate::origin::Origin::UNKNOWN));
        }
        self
    }
}

impl SpectralSum {
    pub fn of(var: Var, lanes: Vec<Lane>) -> SpectralSum {
        SpectralSum { var, lanes }
    }

    pub fn mono(var: Var, atoms: Vec<SpectralAtom>) -> SpectralSum {
        SpectralSum {
            var,
            lanes: vec![Lane::of(atoms)],
        }
    }

    pub fn width(&self) -> usize {
        self.lanes.len()
    }

    pub fn atoms(&self) -> impl Iterator<Item = &SpectralAtom> {
        self.lanes.iter().flat_map(|l| l.atoms.iter())
    }
}
