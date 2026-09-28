// Concern: a loop's proven error through one kernel, and the shortest kernel within precision | Non-concern: a body's gains (loops.rs), refusing (render/bounds.rs) | IO: (Gain, reach, Bound) -> error

use sva_samples::{Bound, Kernel, PSYCHOACOUSTIC_V1, kernel, shortest};

use crate::loops::{self, Expanded};
use crate::time::Q;
use crate::typing::Gain;

type Taps = Vec<(Q, f64)>;

/// The expansion of a fixed linear loop's taps that the shortest kernel within precision
/// needs, `None` where the loop as written already meets it or no expansion does.
pub(crate) fn expansion(taps: &[(f64, Q)], gain: Gain) -> Option<(Taps, Taps)> {
    let (family, precision) = (PSYCHOACOUSTIC_V1.kernel, PSYCHOACOUSTIC_V1.half_lsb());
    let written: Taps = taps.iter().map(|(g, d)| (*d, *g)).collect();
    for half_width in family.lengths(family.most) {
        let (held, expanded) = match loops::expanded(taps, half_width) {
            Expanded::Needless => (Loop::fixed(gain, &written), None),
            Expanded::Taps { rest, own } => {
                (Loop::fixed(loops::own_gain(&own), &own), Some((rest, own)))
            }
            Expanded::Unreachable => continue,
        };
        let error = held.bound(kernel(half_width)).and_then(|b| held.error(&b));
        if half_width <= held.most() && error.is_some_and(|e| e <= precision) {
            return expanded;
        }
    }
    None
}

/// A reading errs `e`, amplifies sample errors `L`-fold; taps gain `W` whole, `K` by kernel.
/// Per generation `x -> e K + (W + L K) x`, summed over `n`. LTI: `X/(1 - O)` against
/// `X/(1 - O - D)`, `|O| <= W + K`, `|D| <= e K`, so `e K / (1 - W - K - e K)` per component.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Loop {
    pub gain: Gain,
    pub least_delay: i64,
    pub least_whole: Option<i64>,
    pub len: Option<i64>,
    pub position: f64,
}

impl Loop {
    pub fn over(gain: Gain, len: Option<i64>) -> Loop {
        Loop {
            gain,
            least_delay: i64::MAX,
            least_whole: None,
            len,
            position: 0.0,
        }
    }

    /// Over no end, each `(delay, coefficient)`.
    pub fn fixed(gain: Gain, own: &[(Q, f64)]) -> Loop {
        let mut held = Loop::over(gain, None);
        for (d, _) in own {
            held.back(*d);
        }
        held
    }

    /// One whose samples overflow is taken as one back.
    pub fn back(&mut self, d: Q) {
        let lattice = Q::int(i64::from(PSYCHOACOUSTIC_V1.lattice_hz));
        match d.mul(lattice) {
            Some(s) if s.is_integer() => {
                let samples = s.num() as i64;
                self.least_whole = Some(self.least_whole.map_or(samples, |w| w.min(samples)));
            }
            Some(s) => self.moving(s.num().div_euclid(s.den()) as i64 + 1, 0.0),
            None => self.moving(1, 0.0),
        }
    }

    pub fn moving(&mut self, least: i64, position: f64) {
        self.least_delay = self.least_delay.min(least);
        self.position = self.position.max(position);
    }

    pub fn most(&self) -> usize {
        usize::try_from(self.least_delay - 1).unwrap_or(0)
    }

    fn gap(&self, half_width: usize) -> i64 {
        let kernel = self.least_delay - half_width as i64;
        self.least_whole.map_or(kernel, |w| w.min(kernel)).max(1)
    }

    fn generations(&self, half_width: usize) -> Option<f64> {
        let gap = self.gap(half_width);
        self.len.map(|len| (len as f64 / gap as f64).ceil())
    }

    pub fn amplified(&self, b: &Bound) -> f64 {
        self.gain.whole + b.lebesgue * self.gain.kernel
    }

    fn injected(&self, b: &Bound) -> f64 {
        b.in_band * self.gain.kernel
    }

    /// Through the kernel `b` is the bound of, its position's rounding counted.
    pub fn error(&self, b: &Bound) -> Option<f64> {
        let a = self.amplified(b);
        let horizon = self
            .generations(b.taps / 2)
            .map(|n| self.injected(b) * geometric(a, n));
        let lasting = (a < 1.0).then(|| self.injected(b) / (1.0 - a));
        let (g, drift) = (self.gain.total(), self.injected(b));
        let spectral = (self.gain.lti && g + drift < 1.0).then(|| drift / (1.0 - g - drift));
        [spectral, horizon, lasting]
            .into_iter()
            .flatten()
            .min_by(f64::total_cmp)
    }

    pub fn bound(&self, k: &Kernel) -> Option<Bound> {
        let p = PSYCHOACOUSTIC_V1;
        k.bound(p.ceiling_hz, p.lattice_hz)
            .map(|b| b.moved(self.position, p.lattice_hz))
    }

    pub fn kernel(&self) -> Option<(&'static Kernel, f64)> {
        let (p, precision) = (PSYCHOACOUSTIC_V1, PSYCHOACOUSTIC_V1.half_lsb());
        let moved = |b: &Bound| b.moved(self.position, p.lattice_hz);
        let meets = |b: &Bound| self.error(&moved(b)).is_some_and(|e| e <= precision);
        let k = shortest(self.most(), meets)?;
        Some((k, self.error(&self.bound(k)?)?))
    }

    pub fn fits(&self, k: &Kernel) -> Option<f64> {
        let b = self.bound(k)?;
        let (a, room) = (
            self.amplified(&b),
            PSYCHOACOUSTIC_V1.half_lsb() / self.injected(&b),
        );
        let mut n = match a == 1.0 {
            true => room.floor(),
            false => ((1.0 + room * (a - 1.0)).ln() / a.ln()).floor().max(0.0),
        };
        while n > 0.0 && geometric(a, n) > room {
            n -= 1.0;
        }
        Some(n * self.gap(k.half_width()) as f64)
    }
}

fn geometric(a: f64, k: f64) -> f64 {
    match a == 1.0 {
        true => k,
        false => (a.powf(k) - 1.0) / (a - 1.0),
    }
}
