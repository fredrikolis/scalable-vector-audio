// Concern: the eight shapes' H(s) as a Rational in s | Non-concern: the biquad recurrence (sva-samples) | IO: (Shape, cutoff, q, gain_db) -> Rational

use std::f64::consts::TAU;

use crate::closed_form::Rational;
use crate::complex::C64;

/// `OnePole` is the 6 dB/oct filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    OnePole,
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
    Peaking,
    Lowshelf,
    Highshelf,
}

/// In the order sva-ast's `FILTERS` names them.
pub const ALL_SHAPES: [Shape; 8] = [
    Shape::OnePole,
    Shape::Lowpass,
    Shape::Highpass,
    Shape::Bandpass,
    Shape::Notch,
    Shape::Peaking,
    Shape::Lowshelf,
    Shape::Highshelf,
];

impl Shape {
    pub fn default_q(self) -> f64 {
        match self {
            Shape::Bandpass | Shape::Notch | Shape::Peaking => 1.0,
            _ => std::f64::consts::FRAC_1_SQRT_2,
        }
    }

    pub fn takes_gain(self) -> bool {
        matches!(self, Shape::Peaking | Shape::Lowshelf | Shape::Highshelf)
    }
}

/// The RBJ Audio EQ Cookbook's analog prototypes, before any bilinear transform.
pub fn design(shape: Shape, cutoff: f64, q: f64, gain_db: f64) -> Rational {
    let w0 = TAU * cutoff;
    let a = 10f64.powf(gain_db / 40.0);
    let root_a = a.sqrt();
    match shape {
        Shape::OnePole => Rational {
            zeros: Vec::new(),
            poles: vec![C64::real(-w0)],
            gain: C64::real(w0),
        },
        Shape::Lowpass => Rational {
            zeros: Vec::new(),
            poles: quadratic(w0 / q, w0 * w0),
            gain: C64::real(w0 * w0),
        },
        Shape::Highpass => Rational {
            zeros: vec![C64::ZERO, C64::ZERO],
            poles: quadratic(w0 / q, w0 * w0),
            gain: C64::ONE,
        },
        Shape::Bandpass => Rational {
            zeros: vec![C64::ZERO],
            poles: quadratic(w0 / q, w0 * w0),
            gain: C64::real(w0 / q),
        },
        Shape::Notch => Rational {
            zeros: vec![C64::new(0.0, w0), C64::new(0.0, -w0)],
            poles: quadratic(w0 / q, w0 * w0),
            gain: C64::ONE,
        },
        Shape::Peaking => Rational {
            zeros: quadratic(w0 * a / q, w0 * w0),
            poles: quadratic(w0 / (q * a), w0 * w0),
            gain: C64::ONE,
        },
        Shape::Lowshelf => Rational {
            zeros: quadratic(root_a * w0 / q, a * w0 * w0),
            poles: quadratic(root_a * w0 / (q * a), w0 * w0 / a),
            gain: C64::ONE,
        },
        Shape::Highshelf => Rational {
            zeros: quadratic(root_a * w0 / (q * a), w0 * w0 / a),
            poles: quadratic(root_a * w0 / q, a * w0 * w0),
            gain: C64::real(a * a),
        },
    }
}

/// The two roots of `s^2 + b*s + c`, real or conjugate.
fn quadratic(b: f64, c: f64) -> Vec<C64> {
    let discriminant = b * b - 4.0 * c;
    let half = C64::real(-b / 2.0);
    let spread = if discriminant >= 0.0 {
        C64::real(discriminant.sqrt() / 2.0)
    } else {
        C64::new(0.0, (-discriminant).sqrt() / 2.0)
    };
    vec![half + spread, half - spread]
}
