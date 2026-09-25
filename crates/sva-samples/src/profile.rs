// Concern: the named tolerance set every label cites, and the ceiling and floor it puts on a rate | Non-concern: what a collapse does with either (collapse/) | IO: (name) -> Profile

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
};

pub fn named(name: &str) -> Option<Profile> {
    (name == PSYCHOACOUSTIC_V1.name).then_some(PSYCHOACOUSTIC_V1)
}

impl Profile {
    pub fn ceiling(&self, rate: u32) -> f64 {
        self.ceiling_hz.min(f64::from(rate) / 2.0)
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
