// Concern: where a ref reads its source, and the index it names at a rate | Non-concern: the reading node's type (typing.rs) | IO: (Offset, rate) -> an index or the count needed

/// A duration is an index offset only once an observation names a rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Offset {
    Steps(i64),
    Secs(f64),
}

/// A duration carries the rounding of its own decimal, so a sample count is whole to
/// floating precision rather than exactly.
const GRID_EPSILON: f64 = 1e-9;

impl Offset {
    pub fn steps_at(self, rate: u32) -> Result<i64, f64> {
        match self {
            Offset::Steps(steps) => Ok(steps),
            Offset::Secs(secs) => {
                let count = secs * f64::from(rate);
                let whole = count.round();
                match (count - whole).abs() <= GRID_EPSILON * count.abs().max(1.0) {
                    true => Ok(whole as i64),
                    false => Err(count),
                }
            }
        }
    }
}
