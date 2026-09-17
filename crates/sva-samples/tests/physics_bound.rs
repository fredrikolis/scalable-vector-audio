// Concern: states which interval each parameter bound admits | Non-concern: which parameter takes which bound (each model) | IO: (value, Bound) -> asserted verdict

use sva_samples::physics::bound::Bound::*;
use sva_samples::physics::bound::{all, holds};

#[test]
fn every_bound_refuses_a_non_finite_value_whatever_else_it_admits() {
    for bound in [
        Finite,
        Positive,
        NonNegative,
        OpenUnit,
        HalfOpenUnit,
        AtLeast(0.0),
        Within(0.0, f64::MAX),
    ] {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!holds(v, bound));
        }
    }
}

#[test]
fn each_bound_admits_its_own_interval_and_nothing_past_either_edge() {
    assert!(holds(-1e300, Finite) && holds(0.0, Finite));
    assert!(holds(1e-300, Positive) && !holds(0.0, Positive));
    assert!(holds(0.0, NonNegative) && !holds(-1e-300, NonNegative));
    assert!(holds(0.5, OpenUnit) && !holds(0.0, OpenUnit) && !holds(1.0, OpenUnit));
    assert!(holds(0.0, HalfOpenUnit) && !holds(1.0, HalfOpenUnit));
    assert!(holds(1.0, AtLeast(1.0)) && !holds(0.999, AtLeast(1.0)));
    assert!(holds(3.0, Within(1.0, 3.0)) && !holds(3.001, Within(1.0, 3.0)));
}

#[test]
fn a_table_holds_only_when_every_row_does() {
    assert!(all(&[(1.0, Positive), (0.0, NonNegative)]));
    assert!(!all(&[(1.0, Positive), (-1.0, NonNegative)]));
    assert!(all(&[]));
}
