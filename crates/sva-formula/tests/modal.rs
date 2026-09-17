// Concern: proves each geometry's eigenfrequencies and the hammer's contact closed form | Non-concern: the transform over the bank they build (duality.rs) | IO: (a geometry) -> its modes

mod fixtures;

use std::f64::consts::{PI, TAU};

use fixtures::{part, sine, term};
use sva_formula::modal::bar::{BarGeom, beta};
use sva_formula::modal::bore::{BoreGeom, Ends, correction, helmholtz};
use sva_formula::modal::membrane::MembraneGeom;
use sva_formula::modal::room::RoomGeom;
use sva_formula::modal::string::{StringGeom, inharmonicity};
use sva_formula::modal::{HammerGeom, contact_time, strike};
use sva_formula::{
    Body, Damping, Edge, Excitation, Geometry, ModalBank, Mode, Var, dual, infer, modes,
    normalize_closed_form,
};

const LOSSLESS: Damping = Damping {
    dc: 1.0,
    per_square_hz: 0.0,
};

fn cents(a: f64, b: f64) -> f64 {
    (1200.0 * (a / b).log2()).abs()
}

fn heard(found: &[Mode]) -> Vec<f64> {
    found.iter().map(|m| m.omega / TAU).collect()
}

/// Each geometry is checked against the literature formula written out again here, so a
/// transcription error in the crate cannot agree with a transcription error in the test.
#[test]
fn string_membrane_bar_bore_room_first_ten_modes_within_five_cents() {
    let (young, diameter, tension, length): (f64, f64, f64, f64) = (2.0e11, 1.0e-3, 700.0, 0.62);
    let b = PI.powi(3) * young * diameter.powi(4) / (64.0 * tension * length * length);
    assert_eq!(b, inharmonicity(young, diameter, tension, length));
    let string = StringGeom {
        f0: 262.0,
        inharmonicity: b,
        strike: 1.0 / 8.0,
        damping: LOSSLESS,
    };
    for (k, hz) in heard(&modes(&Geometry::String(string), 10))
        .iter()
        .enumerate()
    {
        let n = k as f64 + 1.0;
        assert!(cents(*hz, n * 262.0 * (1.0 + b * n * n).sqrt()) < 5.0);
    }

    let membrane = MembraneGeom {
        lx: 0.4,
        ly: 0.3,
        tension: 3000.0,
        density: 0.26,
        strike: (0.3, 0.7),
        damping: LOSSLESS,
    };
    let speed = (3000.0f64 / 0.26).sqrt();
    let lowest = heard(&modes(&Geometry::Membrane(membrane), 10));
    let mut wanted: Vec<f64> = (1..=12)
        .flat_map(|m| (1..=12).map(move |n| (m, n)))
        .map(|(m, n)| {
            0.5 * speed * ((f64::from(m) / 0.4).powi(2) + (f64::from(n) / 0.3).powi(2)).sqrt()
        })
        .collect();
    wanted.sort_by(f64::total_cmp);
    for (got, want) in lowest.iter().zip(wanted.iter()) {
        assert!(cents(*got, *want) < 5.0, "{got} against {want}");
    }

    let bar = BarGeom {
        length: 0.35,
        thickness: 0.02,
        young: 1.4e10,
        density: 800.0,
        damping: LOSSLESS,
    };
    let scale = PI * (0.02 / 12f64.sqrt()) * (1.4e10f64 / 800.0).sqrt() / (8.0 * 0.35 * 0.35);
    for (k, hz) in heard(&modes(&Geometry::Bar(bar), 10)).iter().enumerate() {
        assert!(cents(*hz, scale * beta(k).powi(2)) < 5.0);
    }

    let bore = BoreGeom {
        length: 0.6,
        radius: 0.008,
        speed: 343.0,
        ends: Ends::ClosedOpen,
        damping: LOSSLESS,
    };
    let effective = 0.6 + correction(0.008);
    for (k, hz) in heard(&modes(&Geometry::Bore(bore), 10)).iter().enumerate() {
        let n = k as f64 + 1.0;
        assert!(cents(*hz, (2.0 * n - 1.0) * 343.0 / (4.0 * effective)) < 5.0);
    }

    let room = RoomGeom {
        lx: 5.0,
        ly: 4.0,
        lz: 2.6,
        speed: 343.0,
        damping: LOSSLESS,
    };
    let got = heard(&modes(&Geometry::Room(room), 10));
    let mut wanted: Vec<f64> = (0..=6)
        .flat_map(|l| (0..=6).flat_map(move |m| (0..=6).map(move |n| (l, m, n))))
        .filter(|(l, m, n)| *l + *m + *n > 0)
        .map(|(l, m, n)| {
            0.5 * 343.0
                * ((f64::from(l) / 5.0).powi(2)
                    + (f64::from(m) / 4.0).powi(2)
                    + (f64::from(n) / 2.6).powi(2))
                .sqrt()
        })
        .collect();
    wanted.sort_by(f64::total_cmp);
    for (a, b) in got.iter().zip(wanted.iter()) {
        assert!(cents(*a, *b) < 5.0, "{a} against {b}");
    }

    let (volume, area, neck, radius) = (0.002, 1.5e-4, 0.02, 0.007);
    let effective = neck + 1.7 * radius;
    assert!(
        cents(
            helmholtz(volume, area, neck, radius, 343.0),
            343.0 / TAU * (area / (volume * effective)).sqrt()
        ) < 5.0
    );
}

/// `Tc(v) = Tref*(v/vref)^((1-p)/(1+p))`: Hertzian felt gives -1/5, piano felt about -3/7.
#[test]
fn hammer_pulse_contact_time_follows_the_velocity_exponent() {
    for (stiffness, wanted) in [(1.5, -1.0 / 5.0), (2.5, -3.0 / 7.0)] {
        let felt = HammerGeom {
            reference_velocity: 1.0,
            reference_contact: 2.0e-3,
            stiffness_exponent: stiffness,
            force: 40.0,
        };
        let (slow, fast) = (contact_time(1.0, &felt), contact_time(4.0, &felt));
        assert!((slow - 2.0e-3).abs() < 1e-15, "the reference is its own");
        let measured = (fast / slow).log2() / 4.0f64.log2();
        assert!(
            (measured - wanted).abs() < 1e-12,
            "{measured} against {wanted}"
        );

        let (Excitation::HammerPulse { f0: soft, .. }, Excitation::HammerPulse { f0: hard, .. }) =
            (strike(1.0, &felt, 0.0), strike(4.0, &felt, 0.0))
        else {
            panic!("a strike is a hammer pulse");
        };
        let force = (hard / soft).log2() / 4.0f64.log2();
        let momentum = 2.0 * stiffness / (stiffness + 1.0);
        assert!(
            (force - momentum).abs() < 1e-12,
            "peak force must carry the momentum the contact time does not: {force}"
        );
    }
}

#[test]
fn a_modal_bank_is_a_pair_and_its_excitation_keeps_it_one() {
    let geometry = Geometry::String(StringGeom {
        f0: 220.0,
        inharmonicity: 4.0e-4,
        strike: 1.0 / 7.0,
        damping: Damping {
            dc: 1.5,
            per_square_hz: 2.0e-8,
        },
    });
    let felt = HammerGeom {
        reference_velocity: 1.0,
        reference_contact: 2.0e-3,
        stiffness_exponent: 2.5,
        force: 40.0,
    };
    {
        let bank = Body::Modal(ModalBank {
            modes: modes(&geometry, 8),
            excite: strike(3.0, &felt, 0.0),
        });
        let typed = term(Var::T, bank);
        assert!(
            infer(&typed, &fixtures::Fixed::holding(fixtures::DUAL, false))
                .unwrap()
                .has_dual()
        );
        let sum = normalize_closed_form(&typed).unwrap();
        let spectrum = dual(&sum).expect("a bank of causal modes has a dual in A");
        assert!(
            spectrum.lanes[0].atoms.iter().all(|a| a.pole.is_some()),
            "every mode duals to a pole"
        );
    }
}

/// A hammer's contact and a crop's shoulder are one raised cosine, at a whole turn and a
/// half: a level of `dc` beside two lines of `side`, each phased to the window's own start.
/// Counting the atoms is not enough — swapping `dc` for `side` keeps the count.
#[test]
fn a_hammer_contact_and_a_crop_shoulder_lower_to_one_raised_cosine() {
    let (f0, t0, contact) = (40.0, 0.0, 2.0e-3);
    let pulse = sva_formula::modal::pulse(f0, t0, contact, sva_formula::Origin::UNKNOWN);
    assert_eq!(levels(&pulse), vec![f0 * 0.5], "the contact's own level");
    assert_eq!(
        sides(&pulse),
        vec![-f0 * 0.25, -f0 * 0.25],
        "and one line either side of it, turned upside down"
    );
    assert!(
        pulse.iter().filter_map(|a| a.exp).all(|e| e.sigma == 0.0),
        "a window shape neither grows nor decays"
    );
    let turn: Vec<f64> = pulse
        .iter()
        .filter_map(|a| a.exp)
        .map(|e| e.omega)
        .collect();
    assert_eq!(turn, vec![TAU / contact, -TAU / contact], "a whole turn");

    let windowed = Body::Crop {
        of: part(sine(440.0)),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.01,
        fall: 0.05,
    };
    let shouldered = normalize_closed_form(&term(Var::T, windowed)).unwrap();
    let rise: Vec<sva_formula::SpectralAtom> = shouldered.lanes[0]
        .atoms
        .iter()
        .filter(|a| a.ind.map(|i| i.l) == Some(Edge::at(0.0)))
        .copied()
        .collect();
    assert_eq!(rise.len(), 6, "two lines of a sinusoid, three pieces each");
    let carried: Vec<f64> = rise
        .iter()
        .filter_map(|a| a.exp)
        .map(|e| e.omega.abs() - TAU * 440.0)
        .collect();
    assert_eq!(
        carried.iter().filter(|o| o.abs() < 1e-6).count(),
        2,
        "one level per line of the sinusoid: {carried:?}"
    );
    let half = PI / 0.01;
    assert_eq!(
        carried
            .iter()
            .filter(|o| (o.abs() - half).abs() < 1e-6)
            .count(),
        4,
        "and a shoulder takes half a turn over its own span: {carried:?}"
    );
}

/// The amplitude of every piece a raised cosine's level carries, and of its side lines.
fn levels(atoms: &[sva_formula::SpectralAtom]) -> Vec<f64> {
    atoms
        .iter()
        .filter(|a| a.exp.is_none())
        .map(|a| a.c.re)
        .collect()
}

fn sides(atoms: &[sva_formula::SpectralAtom]) -> Vec<f64> {
    atoms
        .iter()
        .filter(|a| a.exp.is_some())
        .map(|a| a.c.re)
        .collect()
}
