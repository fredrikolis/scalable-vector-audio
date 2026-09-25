// Concern: designs RBJ biquad coefficients and their magnitude response | Non-concern: the shapes and their H(s) (sva-formula), dispatch (filters.rs) | IO: (Shape, cutoff, q, gain_db, sr) -> Coeffs

use sva_formula::filter::Shape;

/// Normalized to `a0 = 1`: `y = b0*x + b1*x1 + b2*x2 - a1*y1 - a2*y2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl State {
    pub fn held(&self) -> [f64; 4] {
        [self.x1, self.x2, self.y1, self.y2]
    }

    #[inline]
    pub fn step(&mut self, c: &Coeffs, x: f64) -> f64 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Outside these it is a wire or a ringing sine, not a filter.
pub const MIN_Q: f64 = 1e-3;
pub const MAX_Q: f64 = 100.0;

/// Short of the top tenth, where `tan` runs away.
const HIGHEST_FRACTION_OF_RATE: f64 = 0.45;

/// A sweep past 0 Hz or Nyquist blows the bilinear design up; the clamp is reported.
pub fn clamp_cutoff(hz: f64, sr: f64) -> (f64, bool) {
    let hi = HIGHEST_FRACTION_OF_RATE * sr;
    let lo = hi.min(1.0);
    if hz < lo {
        (lo, true)
    } else if hz > hi {
        (hi, true)
    } else {
        (hz, false)
    }
}

pub fn clamp_q(q: f64) -> (f64, bool) {
    if q < MIN_Q {
        (MIN_Q, true)
    } else if q > MAX_Q {
        (MAX_Q, true)
    } else {
        (q, false)
    }
}

/// The RBJ Audio EQ Cookbook's bilinear-transform designs, normalized by `a0`.
pub fn design(shape: Shape, cutoff: f64, q: f64, gain_db: f64, sr: f64) -> Coeffs {
    if shape == Shape::OnePole {
        let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff);
        let dt = 1.0 / sr;
        let alpha = dt / (rc + dt);
        return Coeffs {
            b0: alpha,
            b1: 0.0,
            b2: 0.0,
            a1: alpha - 1.0,
            a2: 0.0,
        };
    }

    let w0 = 2.0 * std::f64::consts::PI * cutoff / sr;
    let cs = w0.cos();
    let sn = w0.sin();
    let alpha = sn / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let sqrt_a = a.sqrt();

    let (b0, b1, b2, a0, a1, a2) = match shape {
        Shape::OnePole => unreachable!("handled above"),
        Shape::Lowpass => (
            (1.0 - cs) / 2.0,
            1.0 - cs,
            (1.0 - cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        Shape::Highpass => (
            (1.0 + cs) / 2.0,
            -(1.0 + cs),
            (1.0 + cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        Shape::Bandpass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        Shape::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        Shape::Peaking => (
            1.0 + alpha * a,
            -2.0 * cs,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cs,
            1.0 - alpha / a,
        ),
        Shape::Lowshelf => (
            a * ((a + 1.0) - (a - 1.0) * cs + 2.0 * sqrt_a * alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
            a * ((a + 1.0) - (a - 1.0) * cs - 2.0 * sqrt_a * alpha),
            (a + 1.0) + (a - 1.0) * cs + 2.0 * sqrt_a * alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cs),
            (a + 1.0) + (a - 1.0) * cs - 2.0 * sqrt_a * alpha,
        ),
        Shape::Highshelf => (
            a * ((a + 1.0) + (a - 1.0) * cs + 2.0 * sqrt_a * alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
            a * ((a + 1.0) + (a - 1.0) * cs - 2.0 * sqrt_a * alpha),
            (a + 1.0) - (a - 1.0) * cs + 2.0 * sqrt_a * alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cs),
            (a + 1.0) - (a - 1.0) * cs - 2.0 * sqrt_a * alpha,
        ),
    };

    Coeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

pub fn magnitude_db(c: &Coeffs, hz: f64, sr: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / sr;
    let (c1, s1) = (w.cos(), -w.sin());
    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
    let num_re = c.b0 + c.b1 * c1 + c.b2 * c2;
    let num_im = c.b1 * s1 + c.b2 * s2;
    let den_re = 1.0 + c.a1 * c1 + c.a2 * c2;
    let den_im = c.a1 * s1 + c.a2 * s2;
    let num = (num_re * num_re + num_im * num_im).sqrt();
    let den = (den_re * den_re + den_im * den_im).sqrt();
    if den == 0.0 || num == 0.0 {
        return f64::NEG_INFINITY;
    }
    20.0 * (num / den).log10()
}

pub const RESPONSE_RATIOS: [f64; 9] = [0.125, 0.25, 0.5, 0.707, 1.0, 1.414, 2.0, 4.0, 8.0];

/// A point at or above Nyquist is dropped, not clamped: clamping reports the transform's own
/// zero there as if the filter had put it there.
pub fn response(c: &Coeffs, cutoff: f64, sr: f64) -> Vec<(f64, f64)> {
    RESPONSE_RATIOS
        .iter()
        .map(|r| cutoff * r)
        .filter(|hz| *hz < 0.5 * sr)
        .map(|hz| (hz, magnitude_db(c, hz, sr)))
        .collect()
}
