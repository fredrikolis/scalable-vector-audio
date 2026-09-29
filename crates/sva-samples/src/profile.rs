// Concern: the named tolerance set every label cites, and the ceiling and floor it puts on a rate | Non-concern: what a collapse does with either (collapse/) | IO: (name) -> Profile

use crate::reconstruct::KernelFamily;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub name: &'static str,
    pub level_db: f64,
    pub floor_db: f64,
    pub floor_db_above_5k: f64,
    pub ceiling_hz: f64,
    pub band_db: f64,
    /// The operation count a render pays without the caller saying so.
    pub flop_budget: u128,
    pub precision_bits: i32,
    /// The step every stateful node runs at, and `1sp`.
    pub lattice_hz: u32,
    pub kernel: KernelFamily,
}

pub const PSYCHOACOUSTIC_V1: Profile = Profile {
    name: "psychoacoustic-v1",
    level_db: 0.5,
    floor_db: -20.0,
    floor_db_above_5k: -25.0,
    ceiling_hz: 20_000.0,
    band_db: 1.0,
    flop_budget: 10_000_000_000,
    precision_bits: 24,
    lattice_hz: 44_100,
    kernel: KernelFamily {
        name: "kaiser-sinc",
        step: 4,
        most: 256,
        oversample: 512,
    },
};

pub const LATTICE_8K: Profile = PSYCHOACOUSTIC_V1.on_lattice("lattice-8k", 8_000);

// One kernel family (reconstruct.rs) serves every profile.
const _: () = assert!(LATTICE_8K.fold_margin() == PSYCHOACOUSTIC_V1.fold_margin());

pub fn named(name: &str) -> Option<Profile> {
    [PSYCHOACOUSTIC_V1, LATTICE_8K]
        .into_iter()
        .find(|p| p.name == name)
}

impl Profile {
    pub const fn on_lattice(self, name: &'static str, lattice_hz: u32) -> Profile {
        Profile {
            name,
            ceiling_hz: self.ceiling_hz / self.lattice_hz as f64 * lattice_hz as f64,
            lattice_hz,
            ..self
        }
    }

    pub fn ceiling(&self, rate: u32) -> f64 {
        self.ceiling_hz.min(f64::from(rate) / 2.0)
    }

    pub const fn fold_margin(&self) -> f64 {
        0.5 - self.ceiling_hz / self.lattice_hz as f64
    }

    pub fn half_lsb(&self) -> f64 {
        2f64.powi(-self.precision_bits)
    }

    pub fn floor(&self, hz: f64) -> f64 {
        if hz > 5_000.0 {
            self.floor_db_above_5k
        } else {
            self.floor_db
        }
    }
}
