// Concern: the geometries and the hammer a bank is built from, and the atoms it expands to | Non-concern: any geometry's own eigenfrequencies (the siblings) | IO: (&ModalBank) -> Vec<SpectralAtom>

pub mod bar;
pub mod bore;
pub mod damping;
mod grid;
pub mod membrane;
pub mod room;
pub mod string;

use std::f64::consts::TAU;

use crate::closed_form::{Edge, Excitation, ModalBank, Mode};
use crate::complex::C64;
use crate::modal::damping::Damping;
use crate::origin::Origin;
use crate::spectral_sum::atom::{Exp, Factors, Indicator, Singular, SpectralAtom};
use crate::spectral_sum::image::raised_cosine;

pub fn atoms(bank: &ModalBank, origin: Origin) -> Vec<SpectralAtom> {
    let start = match bank.excite {
        Excitation::HammerPulse { t0, .. } | Excitation::Impulse { t0 } => t0,
    };
    let rings: Vec<SpectralAtom> = bank
        .modes
        .iter()
        .flat_map(|m| {
            [1.0, -1.0].map(|sign| {
                let alpha = C64::new(-1.0 / m.tau, sign * m.omega);
                SpectralAtom::new(
                    C64::new(0.0, sign * m.phase).exp().scale(m.amp * 0.5)
                        * (-alpha.scale(start)).exp(),
                    Factors {
                        exp: Some(Exp::at(alpha.re, alpha.im)),
                        ind: Some(Indicator {
                            l: Edge::at(start),
                            r: Edge::PosInf,
                        }),
                        ..Factors::NONE
                    },
                    Singular::Regular,
                    origin,
                )
            })
        })
        .collect();

    let Excitation::HammerPulse { f0, t0, contact } = bank.excite else {
        return rings;
    };
    let mut out = Vec::new();
    for ring in &rings {
        for push in &pulse(f0, t0, contact, origin) {
            out.push(driven(ring, push));
        }
    }
    out
}

/// Both are a window times one exponential, so their product is one atom that cannot refuse.
fn driven(ring: &SpectralAtom, push: &SpectralAtom) -> SpectralAtom {
    let (r, p) = (ring.factors(), push.factors());
    SpectralAtom::new(
        ring.c * push.c,
        Factors {
            exp: Some(Exp::at(
                r.exp.map_or(0.0, |e| e.sigma) + p.exp.map_or(0.0, |e| e.sigma),
                r.exp.map_or(0.0, |e| e.omega) + p.exp.map_or(0.0, |e| e.omega),
            )),
            ind: Some(
                r.ind
                    .unwrap_or(Indicator::ALL)
                    .meet(p.ind.unwrap_or(Indicator::ALL)),
            ),
            ..Factors::NONE
        },
        Singular::Regular,
        ring.origin,
    )
}

/// Which geometry a modal builtin names.
pub enum Geometry {
    String(string::StringGeom),
    Membrane(membrane::MembraneGeom),
    Bar(bar::BarGeom),
    Bore(bore::BoreGeom),
    Room(room::RoomGeom),
}

pub fn mode(hz: f64, amp: f64, damping: Damping) -> Mode {
    Mode {
        omega: TAU * hz,
        tau: damping.tau(hz),
        amp,
        phase: 0.0,
    }
}

pub fn modes(g: &Geometry, count: usize) -> Vec<Mode> {
    match g {
        Geometry::String(s) => string::modes(s, count),
        Geometry::Membrane(m) => membrane::modes(m, count),
        Geometry::Bar(b) => bar::modes(b, count),
        Geometry::Bore(b) => bore::modes(b, count),
        Geometry::Room(r) => room::modes(r, count),
    }
}

/// Hertzian felt is `p = 1.5`, piano felt about 2.5; Chaigne & Askenfelt, JASA 95 (1994) 1112.
pub struct HammerGeom {
    pub reference_velocity: f64,
    pub reference_contact: f64,
    pub stiffness_exponent: f64,
    pub force: f64,
}

pub fn contact_time(velocity: f64, h: &HammerGeom) -> f64 {
    let p = h.stiffness_exponent;
    h.reference_contact * (velocity / h.reference_velocity).powf((1.0 - p) / (1.0 + p))
}

/// `F0 * Tc` is the hammer's momentum, so the peak force carries what `Tc` does not.
pub fn strike(velocity: f64, h: &HammerGeom, at: f64) -> Excitation {
    let p = h.stiffness_exponent;
    let ratio = velocity / h.reference_velocity;
    Excitation::HammerPulse {
        f0: h.force * ratio.powf(2.0 * p / (p + 1.0)),
        t0: at,
        contact: contact_time(velocity, h),
    }
}

/// A whole turn of a raised cosine, upside down, over the contact; none over no contact.
pub fn pulse(f0: f64, t0: f64, contact: f64, origin: Origin) -> Vec<SpectralAtom> {
    if contact <= 0.0 || !contact.is_finite() {
        return Vec::new();
    }
    raised_cosine(
        t0,
        t0 + contact,
        TAU / contact,
        f0 * 0.5,
        -f0 * 0.25,
        origin,
    )
}
