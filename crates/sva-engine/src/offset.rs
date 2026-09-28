// Concern: where a ref reads its source, and the index it names at a rate | Non-concern: the reading node's type (typing.rs) | IO: (Offset, rate) -> an index map or the count needed

use sva_samples::Remap;

/// A read at `scale*t` moved by a duration and a count of grid steps: a duration is an index
/// offset only once an observation names a rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offset {
    pub scale: i64,
    pub secs: f64,
    pub steps: i64,
}

/// A duration carries the rounding of its own decimal, so a sample count is whole to
/// floating precision rather than exactly.
const GRID_EPSILON: f64 = 1e-9;

impl Offset {
    pub const NOW: Offset = Offset::steps(0);

    pub const fn steps(steps: i64) -> Offset {
        Offset {
            scale: 1,
            secs: 0.0,
            steps,
        }
    }

    /// The index each sample reads, or the count of samples the offset names where that is
    /// not whole.
    pub fn steps_at(self, rate: u32) -> Result<Remap, f64> {
        let count = self.secs * f64::from(rate);
        let whole = count.round();
        match (count - whole).abs() <= GRID_EPSILON * count.abs().max(1.0) {
            true => Ok(Remap {
                scale: self.scale,
                shift: (whole as i64).saturating_add(self.steps),
            }),
            false => Err(count + self.steps as f64),
        }
    }
}
