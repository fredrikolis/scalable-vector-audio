// Concern: bounds a fixed biquad's computed output and its ringing once its input is zero | Non-concern: a moving parameter, the input's own bound | IO: (Coeffs, input bound, end) -> a bound per sample

use sva_samples::biquad::Coeffs;

/// A step's nine roundings and five underflows err `<= GAMMA*sum|terms| + TINY`, and `y` is
/// `h*x + g*err`, so `|y| <= whole`. Past `quiet`, `y[quiet+m] = u0*g[m] + u1*g[m-1] + (g*err)[m]`.
pub(super) struct Ringing {
    whole: f64,
    quiet: i64,
    lead: f64,
    lag: f64,
    floor: f64,
    decay: Envelope,
}

const GAMMA: f64 = 16.0 * f64::EPSILON;
const TINY: f64 = 5.0 * f64::MIN_POSITIVE * f64::EPSILON;
const SLACK: f64 = 1e-9;

impl Ringing {
    /// The input is within `input`, and zero from `end` on.
    pub(super) fn of(c: &Coeffs, input: f64, end: i64) -> Option<Ringing> {
        let decay = Envelope::of(c.a1, c.a2)?;
        let gain = c.b0.abs() + c.b1.abs() + c.b2.abs();
        let feedback = c.a1.abs() + c.a2.abs();
        let carried = decay.sum();
        let kept = 1.0 - carried * GAMMA * feedback;
        if !(input.is_finite() && carried.is_finite() && kept > 0.5) {
            return None;
        }
        let whole = (gain * carried * input * (1.0 + GAMMA) + carried * TINY) / kept;
        Some(Ringing {
            whole,
            quiet: end.checked_add(2)?,
            lead: feedback * whole,
            lag: c.a2.abs() * whole,
            floor: carried * (GAMMA * feedback * whole + TINY),
            decay,
        })
    }

    pub(super) fn from(&self, n: i64) -> f64 {
        if n < self.quiet {
            return self.whole * (1.0 + SLACK);
        }
        let m = n - self.quiet;
        let rings = self.lead * self.decay.at(m) + self.lag * self.decay.at(m - 1);
        (rings + self.floor).min(self.whole) * (1.0 + SLACK)
    }
}

/// Bounds `sup_{j>=k}|g[j]|` off `(k+1)*rho^k`, a pair's `r^k/sin(theta)`, one pole's `rho^k`.
#[derive(Clone, Copy)]
struct Envelope {
    rho: f64,
    peak: i64,
    pair: Option<f64>,
    single: bool,
}

impl Envelope {
    fn of(a1: f64, a2: f64) -> Option<Envelope> {
        if !(a1.is_finite() && a2.is_finite()) {
            return None;
        }
        if a2 == 0.0 {
            let rho = a1.abs() * (1.0 + SLACK);
            let single = Envelope {
                rho,
                peak: 0,
                pair: None,
                single: true,
            };
            return (rho < 1.0).then_some(single);
        }
        let disc = a1 * a1 - 4.0 * a2;
        let disc_hi = disc + 8.0 * f64::EPSILON * (a1 * a1 + 4.0 * a2.abs());
        let (rho, pair) = match disc_hi < 0.0 {
            true => (a2.sqrt(), Some((4.0 * a2 / -disc_hi).sqrt())),
            false => ((a1.abs() + disc_hi.sqrt()) / 2.0, None),
        };
        let rho = rho.max(a2.abs().sqrt()) * (1.0 + SLACK);
        if rho.is_nan() || rho >= 1.0 {
            return None;
        }
        let peak = ((2.0 * rho - 1.0) / (1.0 - rho)).ceil().max(0.0);
        Some(Envelope {
            rho,
            peak: peak.min(i64::MAX as f64) as i64,
            pair: pair.map(|p| p * (1.0 + SLACK)),
            single: false,
        })
    }

    fn power(&self, k: i64) -> f64 {
        ((k as f64 * self.rho.ln()).exp() * (1.0 + SLACK)).max(f64::MIN_POSITIVE)
    }

    fn at(&self, k: i64) -> f64 {
        let k = k.max(0);
        if self.single {
            return self.power(k);
        }
        let from = k.max(self.peak);
        let linear = (from as f64 + 1.0) * self.power(from);
        match self.pair {
            Some(pair) => linear.min(pair * self.power(k)),
            None => linear,
        }
    }

    fn sum(&self) -> f64 {
        let open = 1.0 - self.rho;
        let held = match (self.single, self.pair) {
            (true, _) => 1.0 / open,
            (false, Some(pair)) => (1.0 / (open * open)).min(pair / open),
            (false, None) => 1.0 / (open * open),
        };
        held * (1.0 + SLACK)
    }
}
