// Concern: the named tolerance set every label cites, and the ceiling and floor it puts on a rate | Non-concern: what a collapse does with either (collapse/), choosing the rate | IO: none -> Profile

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub name: &'static str,
    pub floor_db: f64,
    pub floor_db_above_5k: f64,
    pub ceiling_hz: f64,
    /// The operation count a render pays without the caller saying so.
    pub flop_budget: u128,
    pub precision_bits: i32,
    pub prune_db: f64,
}

pub const PSYCHOACOUSTIC_V1: Profile = Profile {
    name: "psychoacoustic-v1",
    floor_db: -20.0,
    floor_db_above_5k: -25.0,
    ceiling_hz: 20_000.0,
    flop_budget: 10_000_000_000,
    precision_bits: 24,
    prune_db: -120.0,
};

impl Profile {
    pub fn ceiling(&self, rate: u32) -> f64 {
        self.ceiling_hz.min(f64::from(rate) / 2.0)
    }

    pub fn half_lsb(&self) -> f64 {
        2f64.powi(-self.precision_bits)
    }

    pub fn prune_level(&self) -> f64 {
        10f64.powf(self.prune_db / 20.0)
    }

    /// Every setting a value or its label depends on.
    pub fn deciding(&self) -> [u64; 6] {
        let Profile {
            name,
            floor_db,
            floor_db_above_5k,
            ceiling_hz,
            flop_budget: _,
            precision_bits,
            prune_db,
        } = *self;
        let named = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |held, b| {
            (held ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        [
            named,
            floor_db.to_bits(),
            floor_db_above_5k.to_bits(),
            ceiling_hz.to_bits(),
            precision_bits as u64,
            prune_db.to_bits(),
        ]
    }

    pub fn floor(&self, hz: f64) -> f64 {
        if hz > 5_000.0 {
            self.floor_db_above_5k
        } else {
            self.floor_db
        }
    }
}
