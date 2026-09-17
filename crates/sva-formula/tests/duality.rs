// Concern: proves every atom family's dual lands back in A and the kernel is its own inverse | Non-concern: deciding a type without transforming (inference.rs) | IO: (a SpectralSum) -> its dual

mod fixtures;

use fixtures::{bank, causal, constant, cosine, decay, gaussian, line, part, sine, term, terms};
use sva_formula::{
    Body, C64, Edge, Rational, SpectralSum, Var, dual, normalize, normalize_closed_form, reflect,
};

fn law(f: &Body, var: Var) -> SpectralSum {
    normalize(f, var).expect("the fixture stays in A")
}

fn round_trip(f: &Body, var: Var) {
    reflects(&law(f, var), "");
}

/// Same atoms, in the same order, with parameters that agree: the transcendental families
/// cannot land on the same bits, and shape is what the table actually promises.
fn reflects(x: &SpectralSum, name: &str) {
    let there = dual(x).expect("the fixture has a dual in A");
    let back = dual(&there).expect("the dual has a dual in A");
    let wanted = reflect(x).unwrap();
    assert_eq!(back.var, wanted.var, "{name}");
    assert_eq!(back.lanes.len(), wanted.lanes.len(), "{name}");
    for (got, want) in back.lanes.iter().zip(wanted.lanes.iter()) {
        assert_eq!(got.series, want.series, "{name}");
        assert_eq!(got.modal, want.modal, "{name}");
        let (got, want) = (audible(&got.atoms), audible(&want.atoms));
        assert_eq!(got.len(), want.len(), "{name}: {got:?} against {want:?}");
        for (a, b) in got.iter().zip(want.iter()) {
            assert!(alike(a, b), "{name}: {a:?} against {b:?}");
        }
    }
}

/// A round trip through the pole family leaves amplitudes a thousand billion times below
/// the closed form's own scale. The spectral sum keeps them, because folding on an epsilon would make
/// equality intransitive; a structural comparison is where they are read as the residue
/// they are.
fn audible(atoms: &[sva_formula::SpectralAtom]) -> Vec<sva_formula::SpectralAtom> {
    let loudest = atoms.iter().map(|a| a.c.abs()).fold(0.0f64, f64::max);
    atoms
        .iter()
        .filter(|a| a.c.abs() > 1e-9 * loudest)
        .copied()
        .collect()
}

fn alike(a: &sva_formula::SpectralAtom, b: &sva_formula::SpectralAtom) -> bool {
    let near = |x: f64, y: f64| x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0);
    let edges = |x: Option<sva_formula::Indicator>, y: Option<sva_formula::Indicator>| match (x, y)
    {
        (None, None) => true,
        (Some(p), Some(q)) => near(p.l.value(), q.l.value()) && near(p.r.value(), q.r.value()),
        _ => false,
    };
    a.poly == b.poly
        && a.sing == b.sing
        && near(a.c.re, b.c.re)
        && near(a.c.im, b.c.im)
        && match (a.exp, b.exp) {
            (None, None) => true,
            (Some(p), Some(q)) => near(p.sigma, q.sigma) && near(p.omega, q.omega),
            _ => false,
        }
        && match (a.gauss, b.gauss) {
            (None, None) => true,
            (Some(p), Some(q)) => near(p.a, q.a) && near(p.mu, q.mu),
            _ => false,
        }
        && edges(a.ind, b.ind)
        && match (a.pole, b.pole) {
            (None, None) => true,
            (Some(p), Some(q)) => {
                p.order == q.order
                    && p.pv == q.pv
                    && near(p.at.re, q.at.re)
                    && near(p.at.im, q.at.im)
            }
            _ => false,
        }
}

#[test]
fn a_line_pair_round_trips() {
    round_trip(&sine(440.0), Var::T);
    let d = dual(&law(&sine(440.0), Var::T)).unwrap();
    assert_eq!(d.var, Var::F);
    assert_eq!(d.lanes[0].atoms.len(), 2, "one line is two deltas");
    for atom in &d.lanes[0].atoms {
        assert!(atom.is_delta());
    }
}

#[test]
fn a_delta_derivative_round_trips() {
    let free = law(&line(), Var::T);
    let d = dual(&free).unwrap();
    let [atom] = d.lanes[0].atoms[..] else {
        panic!("t duals to one delta derivative");
    };
    assert_eq!(
        atom.sing,
        sva_formula::Singular::Delta { at: 0.0, order: 1 }
    );
    assert_eq!(atom.c, C64::new(0.0, 1.0 / std::f64::consts::TAU));
    round_trip(&line(), Var::T);
}

#[test]
fn a_gaussian_round_trips() {
    let a = std::f64::consts::PI;
    let self_dual = law(&gaussian(a, 0.0), Var::T);
    let d = dual(&self_dual).unwrap();
    let [atom] = d.lanes[0].atoms[..] else {
        panic!("a Gaussian duals to one Gaussian");
    };
    let width = atom.gauss.expect("still a Gaussian").a;
    assert!((width - a).abs() < 1e-12, "exp(-pi*t^2) is self-dual");
    assert!((atom.c.re - 1.0).abs() < 1e-12, "{:?}", atom.c);
    round_trip(&gaussian(a, 0.0), Var::T);
    round_trip(&gaussian(2.0, 0.25), Var::T);
}

/// A carrier shifts the image Gaussian. Written as a real exponential beside a centred one
/// instead, its constant `e^{-pi^2*f0^2/a}` underflows while the shift overflows, leaving the
/// atom non-finite past 54 hertz at `a = 40`; these sit either side of that edge.
#[test]
fn a_gaussian_under_a_carrier_duals_to_a_shifted_gaussian() {
    let a = 40.0;
    let peak = 0.5 * (std::f64::consts::PI / a).sqrt();
    for hz in [1.0, 53.0, 54.0, 261.625_565_300_598_6, 8000.0] {
        let f = Body::Mul(vec![part(gaussian(a, 0.0)), part(cosine(hz))]);
        let d = dual(&law(&f, Var::T)).unwrap();
        let mut found: Vec<(f64, f64)> = d.lanes[0]
            .atoms
            .iter()
            .map(|at| (at.gauss.expect("still a Gaussian").mu, at.c.abs()))
            .collect();
        found.sort_by(|x, y| x.0.total_cmp(&y.0));
        let [(low, left), (high, right)] = found[..] else {
            panic!("{hz} hertz: a carrier duals to one conjugate pair, not {found:?}");
        };
        assert!((low + hz).abs() <= 1e-9 * hz, "{hz} hertz: {low}");
        assert!((high - hz).abs() <= 1e-9 * hz, "{hz} hertz: {high}");
        assert!((left - peak).abs() <= 1e-9 * peak, "{hz} hertz: {left}");
        assert!((right - peak).abs() <= 1e-9 * peak, "{hz} hertz: {right}");
        round_trip(&f, Var::T);
        // The Hermite lift is written against the same centre, so it moves with it.
        round_trip(
            &Body::Mul(vec![part(line()), part(gaussian(a, 0.0)), part(cosine(hz))]),
            Var::T,
        );
    }
}

#[test]
fn a_finite_indicator_round_trips() {
    let window = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(0.0),
        r: Edge::at(2.0),
        rise: 0.0,
        fall: 0.0,
    };
    let d = dual(&law(&window, Var::T)).unwrap();
    assert!(
        d.lanes[0].atoms.iter().all(|a| a.pole.is_some()),
        "a finite window duals to poles"
    );
    round_trip(&window, Var::T);
}

#[test]
fn a_causal_exponential_round_trips() {
    let ringing = causal(Body::Mul(vec![part(sine(440.0)), part(decay(-2.0))]));
    let d = dual(&law(&ringing, Var::T)).unwrap();
    assert_eq!(d.lanes[0].atoms.len(), 2, "a decaying line is a pole pair");
    for atom in &d.lanes[0].atoms {
        assert_eq!(atom.pole.expect("a pole").order, 1);
    }
    round_trip(&ringing, Var::T);
}

#[test]
fn a_heaviside_round_trips_through_pv() {
    let step = causal(constant(1.0));
    let d = dual(&law(&step, Var::T)).unwrap();
    assert_eq!(d.lanes[0].atoms.len(), 2);
    assert!(
        d.lanes[0]
            .atoms
            .iter()
            .any(sva_formula::SpectralAtom::is_delta)
    );
    assert!(
        d.lanes[0]
            .atoms
            .iter()
            .any(|a| a.pole.is_some_and(|p| p.pv)),
        "the step's dual needs a principal value"
    );
    round_trip(&step, Var::T);
}

#[test]
fn a_pv_round_trips_through_sgn() {
    let principal = Body::Pv(part(line()));
    let d = dual(&law(&principal, Var::T)).unwrap();
    assert_eq!(d.lanes[0].atoms.len(), 2, "sgn is two indicator atoms");
    for atom in &d.lanes[0].atoms {
        assert!(atom.ind.is_some());
        assert!((atom.c.re).abs() < 1e-15 && atom.c.im.abs() > 1.0);
    }
    round_trip(&principal, Var::T);
}

/// The residue experiment behind the order-12 cap measured 7.6e-9 dB; the round trip cannot
/// beat the cancellation that measurement found, so this is the bound it is held to.
const ROUND_TRIP_DB: f64 = 1e-8;

/// Checked against the response, not the atoms: the cap exists because of conditioning.
#[test]
fn a_twelfth_order_rational_round_trips_within_the_measured_residue_bound() {
    let poles: Vec<C64> = (0..6)
        .flat_map(|k| {
            let angle = std::f64::consts::PI * (0.5 + (2.0 * f64::from(k) + 1.0) / 24.0);
            let p = C64::new(angle.cos(), angle.sin()).scale(300.0);
            [p, p.conj()]
        })
        .collect();
    let written = Rational {
        zeros: Vec::new(),
        poles: poles.clone(),
        gain: C64::real(300.0f64.powi(12)),
    };
    let spectrum = law(&Body::Rational(written), Var::F);
    let impulse = dual(&spectrum).unwrap();
    let back = reflect(&dual(&impulse).unwrap()).unwrap();

    let mut worst = 0.0f64;
    for step in 0..200 {
        let f = 1.0 + 40.0 * f64::from(step);
        let direct = evaluate(&spectrum, f);
        let looped = evaluate(&back, f);
        let db = 20.0 * direct.abs().log10();
        if db < -100.0 {
            continue;
        }
        worst = worst.max((20.0 * looped.abs().log10() - db).abs());
    }
    assert!(worst < ROUND_TRIP_DB, "worst round-trip error {worst} dB");
}

#[test]
fn a_line_series_round_trips_termwise() {
    let saw = term(Var::T, fixtures::saw_series(220.0));
    let x = normalize_closed_form(&saw).unwrap();
    assert_eq!(x.lanes[0].series.len(), 1, "a series stays unexpanded");
    let there = dual(&x).unwrap();
    assert_eq!(there.lanes[0].series.len(), 1);
    reflects(&x, "a band-limited saw");
}

#[test]
fn a_modal_bank_round_trips() {
    let x = normalize_closed_form(&term(Var::T, bank())).unwrap();
    assert_eq!(x.lanes[0].modal.len(), 1, "a bank stays a bank until asked");
    let there = dual(&x).unwrap();
    assert!(there.lanes[0].modal.is_empty(), "the transform expands it");
    assert_eq!(there.lanes[0].atoms.len(), 2, "one mode is one pole pair");
    reflects(&x, "a modal bank");
}

#[test]
fn the_dual_of_the_dual_is_a_reflection() {
    for (name, subject) in terms() {
        let Ok(x) = normalize_closed_form(&subject) else {
            continue;
        };
        if dual(&x).is_err() {
            continue;
        }
        reflects(&x, name);
    }
}

/// The atom sum's value at one point, which is all the numeric row needs; sampling a closed form is
/// otherwise no concern of this crate.
fn evaluate(n: &SpectralSum, at: f64) -> C64 {
    n.lanes[0]
        .atoms
        .iter()
        .filter_map(|a| a.smooth_at(at))
        .fold(C64::ZERO, |acc, v| acc + v)
}

/// A line a second away from the origin normalizes the pole row's exponential at its own
/// window edge, where the factor is bounded, rather than as a prefactor that overflows and
/// an exponential that underflows against it.
#[test]
fn a_lowpass_on_a_negative_frequency_line_is_finite() {
    use sva_formula::{Exp, Factors, Origin, Pole, Singular, SpectralAtom};
    let modulated = SpectralAtom::new(
        C64::ONE,
        Factors {
            exp: Some(Exp::at(0.0, -std::f64::consts::TAU)),
            pole: Some(Pole {
                at: C64::new(0.0, 200.0),
                order: 1,
                pv: false,
            }),
            ..Factors::NONE
        },
        Singular::Regular,
        Origin::UNKNOWN,
    );
    let held = sva_formula::table::dual_atom(&modulated).expect("a pole row with a reference");
    assert!(!held.is_empty());
    for a in &held {
        for at in [-1.0, -1.001, -1.5, -2.0, 0.0] {
            let v = a.smooth_at(at).expect("a value off the pole");
            assert!(
                v.abs().is_finite() && v.abs() <= 10.0,
                "theta {at}: {} is not the bounded value the window carries",
                v.abs()
            );
        }
    }
}
