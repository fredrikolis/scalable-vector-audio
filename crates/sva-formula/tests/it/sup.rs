// Concern: proves an atom's magnitude bound from an instant holds at every later instant, or is none | Non-concern: summing atoms into a node's bound (sva-engine) | IO: (an atom, t) -> a bound or none

use sva_formula::spectral_sum::atom::{
    Exp, Factors, Gauss, Indicator, Pole, Poly, Singular, SpectralAtom,
};
use sva_formula::spectral_sum::sup::magnitude_upper_bound_from_instant;
use sva_formula::{C64, Edge, Origin};

fn atom(factors: Factors) -> SpectralAtom {
    SpectralAtom::new(C64::real(0.7), factors, Singular::Regular, Origin::UNKNOWN)
}

/// Every pairing of a power, an exponential, a Gaussian, a pole and a window.
fn atoms() -> Vec<SpectralAtom> {
    let poly = Poly { degree: 3, at: 0.2 };
    let rising = Exp {
        sigma: 4.0,
        omega: 30.0,
        mu: 0.0,
    };
    let falling = Exp {
        sigma: -6.0,
        omega: 0.0,
        mu: 0.5,
    };
    let gauss = Gauss { a: 50.0, mu: 0.5 };
    let pole = Pole {
        at: C64::new(1.0, 0.3),
        order: 2,
        pv: false,
    };
    let window = Indicator {
        l: Edge::at(0.1),
        r: Edge::at(2.0),
    };
    let mut out = Vec::new();
    for p in [Poly::ONE, poly] {
        for e in [None, Some(rising), Some(falling)] {
            for g in [None, Some(gauss)] {
                for q in [None, Some(pole)] {
                    for w in [None, Some(window)] {
                        out.push(atom(Factors {
                            poly: p,
                            exp: e,
                            gauss: g,
                            ind: w,
                            pole: q,
                        }));
                    }
                }
            }
        }
    }
    out
}

/// A rising factor beside a Gaussian gone to zero is `inf * 0`: never a NaN bound.
#[test]
fn an_atom_s_bound_from_an_instant_holds_at_every_later_instant() {
    let starts = [-3.0, 0.0, 0.3, 1.0, 5.0, 40.0, 900.0];
    for a in atoms() {
        for t in starts {
            let Some(bound) = magnitude_upper_bound_from_instant(&a, t) else {
                continue;
            };
            assert!(!bound.is_nan(), "{a:?} from {t}: NaN");
            for k in 0..400 {
                let s = t + (k as f64) * 0.01 + (k as f64 / 40.0).exp2() - 1.0;
                let v = match a.smooth_at(s).map_or(0.0, |v| v.abs()) {
                    v if v.is_nan() => f64::INFINITY,
                    v => v,
                };
                assert!(
                    v <= bound * (1.0 + 1e-9),
                    "{a:?} from {t}: {v} at {s} over {bound}"
                );
            }
        }
    }
}

/// `e^-760` underflows beside a pole's `1e300`, though their product is about `1e-30`.
#[test]
fn a_factor_past_a_double_s_range_still_bounds_the_product() {
    let a = atom(Factors {
        exp: Some(Exp {
            sigma: -1.0,
            omega: 0.0,
            mu: 0.0,
        }),
        pole: Some(Pole {
            at: C64::new(760.0, 1e-150),
            order: 2,
            pv: false,
        }),
        ..Factors::NONE
    });
    let truth = (0.7f64.ln() - 760.0 + 300.0 * 10f64.ln()).exp();
    let bound = magnitude_upper_bound_from_instant(&a, 760.0).expect("a bound");
    assert!(truth > 1e-31 && bound >= truth, "{bound} under {truth}");
}
