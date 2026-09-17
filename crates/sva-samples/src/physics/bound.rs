// Concern: the range one physics parameter must land in, and the test that it did | Non-concern: which parameter takes which range (each model's own valid()) | IO: (value, Bound) -> bool

/// Every shape the six physics models' parameters are checked against. Each implies finiteness,
/// so a table entry is the whole test for its field.
#[derive(Clone, Copy)]
pub enum Bound {
    Finite,
    Positive,
    NonNegative,
    OpenUnit,
    HalfOpenUnit,
    AtLeast(f64),
    Within(f64, f64),
}

pub fn holds(v: f64, bound: Bound) -> bool {
    v.is_finite()
        && match bound {
            Bound::Finite => true,
            Bound::Positive => v > 0.0,
            Bound::NonNegative => v >= 0.0,
            Bound::OpenUnit => v > 0.0 && v < 1.0,
            Bound::HalfOpenUnit => (0.0..1.0).contains(&v),
            Bound::AtLeast(lo) => v >= lo,
            Bound::Within(lo, hi) => (lo..=hi).contains(&v),
        }
}

pub fn all(fields: &[(f64, Bound)]) -> bool {
    fields.iter().all(|&(v, bound)| holds(v, bound))
}
