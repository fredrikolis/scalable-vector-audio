// Concern: the modal bank each named instrument builtin lowers to | Non-concern: any geometry's eigenfrequencies (sva-formula), the finite-difference family (calls.rs) | IO: (name, args) -> a Piece

use sva_formula::modal::bar::BarGeom;
use sva_formula::modal::bore::{BoreGeom, Ends, helmholtz};
use sva_formula::modal::membrane::MembraneGeom;
use sva_formula::modal::room::RoomGeom;
use sva_formula::modal::string::StringGeom;
use sva_formula::modal::{HammerGeom, mode, strike};
use sva_formula::{Body, Damping, Excitation, Geometry, ModalBank, Mode, modes};

use crate::error::EngineError;
use crate::lower::{Lowering, Piece};

/// The seven names of FORMAT 12.1, each a series of modes and each a pair.
pub const MODAL: [&str; 7] = [
    "string",
    "membrane",
    "bar",
    "bore",
    "room",
    "hammer_pulse",
    "helmholtz",
];

/// Enough partials to reach the ceiling for a bass fundamental; the collapse drops what the
/// profile's floor does not carry.
const MODE_COUNT: usize = 256;

/// Chaigne & Askenfelt's piano hammer, JASA 95 (1994) 1112, as the reference contact.
const REFERENCE_VELOCITY: f64 = 3.2;
const REFERENCE_CONTACT: f64 = 0.0015;
const REFERENCE_FORCE: f64 = 500.0;

struct Args<'a> {
    positional: &'a [f64],
    named: &'a [(&'a str, f64)],
}

impl Args<'_> {
    fn at(&self, slot: usize, key: &str, fallback: f64) -> f64 {
        match self.positional.get(slot) {
            Some(v) => *v,
            None => super::calls::named_or(self.named, key, fallback),
        }
    }

    fn named(&self, key: &str, fallback: f64) -> f64 {
        super::calls::named_or(self.named, key, fallback)
    }

    fn damping(&self) -> Damping {
        Damping {
            dc: self.named("damp_dc", 0.6),
            per_square_hz: self.named("damp_freq", 1.6e-4),
        }
    }

    fn count(&self) -> usize {
        (self.named("modes", MODE_COUNT as f64) as usize).clamp(1, 4096)
    }

    /// A velocity names a hammer; without one the bank rings from an impulse.
    fn excitation(&self) -> Excitation {
        let at = self.named("at", 0.0);
        match self.named.iter().find(|(k, _)| *k == "vel") {
            Some((_, vel)) => strike(
                *vel,
                &HammerGeom {
                    reference_velocity: REFERENCE_VELOCITY,
                    reference_contact: self.named("contact", REFERENCE_CONTACT),
                    stiffness_exponent: self.named("p", 2.5),
                    force: self.named("force", REFERENCE_FORCE),
                },
                at,
            ),
            None => Excitation::Impulse { t0: at },
        }
    }
}

impl Lowering<'_, '_> {
    /// Every geometry answers with its own modes; the excitation multiplies them.
    pub(super) fn modal(
        &mut self,
        name: &str,
        positional: &[f64],
        named: &[(&str, f64)],
    ) -> Result<Piece, EngineError> {
        let args = Args { positional, named };
        let bad = || EngineError::BadArity(name.to_string());
        let geometry = match name {
            "string" => Geometry::String(StringGeom {
                f0: *positional.first().ok_or_else(bad)?,
                inharmonicity: args.named("inharmonicity", 0.00021),
                strike: args.named("strike", 0.125),
                damping: args.damping(),
            }),
            "membrane" => Geometry::Membrane(MembraneGeom {
                lx: *positional.first().ok_or_else(bad)?,
                ly: args.at(1, "ly", 0.36),
                tension: args.named("tension", 3000.0),
                density: args.named("density", 0.26),
                strike: (args.named("strike_x", 0.3), args.named("strike_y", 0.5)),
                damping: args.damping(),
            }),
            "bar" => Geometry::Bar(BarGeom {
                length: *positional.first().ok_or_else(bad)?,
                thickness: args.named("thickness", 0.009),
                young: args.named("young", 6.9e10),
                density: args.named("density", 2700.0),
                damping: args.damping(),
            }),
            "bore" => Geometry::Bore(BoreGeom {
                length: *positional.first().ok_or_else(bad)?,
                radius: args.named("radius", 0.0075),
                speed: args.named("speed", 343.0),
                ends: match args.named("closed", 0.0) == 0.0 {
                    true => Ends::OpenOpen,
                    false => Ends::ClosedOpen,
                },
                damping: args.damping(),
            }),
            "room" => Geometry::Room(RoomGeom {
                lx: *positional.first().ok_or_else(bad)?,
                ly: args.at(1, "ly", 4.0),
                lz: args.at(2, "lz", 2.7),
                speed: args.named("speed", 343.0),
                damping: args.damping(),
            }),
            "helmholtz" => return Ok(self.cavity(&args, positional.first().ok_or_else(bad)?)),
            "hammer_pulse" => return Ok(self.contact(&args, *positional.first().ok_or_else(bad)?)),
            other => unreachable!("{other} is not one of the modal names"),
        };
        Ok(self.bank(modes(&geometry, args.count()), args.excitation()))
    }

    /// One mode at the cavity's own resonance, damped by the same closed form every geometry uses.
    fn cavity(&mut self, args: &Args, volume: &f64) -> Piece {
        let hz = helmholtz(
            *volume,
            args.named("neck_area", 0.0005),
            args.named("neck_length", 0.02),
            args.named("radius", 0.0126),
            args.named("speed", 343.0),
        );
        self.bank(vec![mode(hz, 1.0, args.damping())], args.excitation())
    }

    /// A velocity closed form on its own: one undamped, unturning mode the contact shapes.
    fn contact(&mut self, args: &Args, vel: f64) -> Piece {
        let excite = strike(
            vel,
            &HammerGeom {
                reference_velocity: REFERENCE_VELOCITY,
                reference_contact: args.named("contact", REFERENCE_CONTACT),
                stiffness_exponent: args.named("p", 2.5),
                force: args.named("force", REFERENCE_FORCE),
            },
            args.named("at", 0.0),
        );
        let carrier = Mode {
            omega: 0.0,
            tau: f64::INFINITY,
            amp: 1.0,
            phase: 0.0,
        };
        self.bank(vec![carrier], excite)
    }

    fn bank(&mut self, modes: Vec<Mode>, excite: Excitation) -> Piece {
        Piece::ClosedForm(Body::Modal(ModalBank { modes, excite }))
    }
}
