// Concern: resolves each frame into the resonances an all-pole model puts under it | Non-concern: the discrete spectrum (spectrum.rs), naming a peak (pitch.rs) | IO: (&[f64], sample rate) -> frames

use crate::measure::spectrum::db;

pub const MAX_ORDER: usize = 64;

/// Nearer than this to DC or Nyquist a pole is the overall tilt, not a resonance.
const EDGE_HZ: f64 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Formant {
    pub hz: f64,
    pub bandwidth_hz: f64,
    pub db: f64,
}

/// `residual` and `energy` are mean squares, and neither derives from the other.
#[derive(Clone, Debug, PartialEq)]
pub struct FormantFrame {
    pub t_secs: f64,
    pub order: usize,
    pub energy: f64,
    pub residual: f64,
    pub formants: Vec<Formant>,
}

/// Two poles per kilohertz of bandwidth plus two for the tilt — 46 at 44.1 kHz, far more than a
/// voice wants and about right for a full-band instrument.
pub fn default_order(sample_rate: f64) -> usize {
    (2.0 + sample_rate / 1000.0).clamp(2.0, MAX_ORDER as f64) as usize
}

pub fn track(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
    order: usize,
    max_formants: usize,
) -> Vec<FormantFrame> {
    let stride = ((frame_secs * sample_rate).round() as usize).max(1);
    samples
        .chunks(stride)
        .enumerate()
        .map(|(n, chunk)| FormantFrame {
            t_secs: start_secs + (n * stride) as f64 / sample_rate,
            ..analyze(chunk, sample_rate, order, max_formants)
        })
        .collect()
}

/// Hamming, not Hann: a taper reaching zero throws the frame's edges away. Never
/// pre-emphasised; `x - 0.97*x(t - 1sp)` is a composition's own to write.
pub fn analyze(frame: &[f64], sample_rate: f64, order: usize, max_formants: usize) -> FormantFrame {
    let order = order.clamp(2, MAX_ORDER).min(frame.len().saturating_sub(1));
    let windowed: Vec<f64> = frame
        .iter()
        .enumerate()
        .map(|(n, &x)| {
            let turn = 2.0 * std::f64::consts::PI * n as f64 / frame.len() as f64;
            let w = 0.54 - 0.46 * turn.cos();
            x * w
        })
        .collect();
    let energy = match frame.is_empty() {
        true => 0.0,
        false => windowed.iter().map(|x| x * x).sum::<f64>() / frame.len() as f64,
    };
    if order < 2 || energy <= 0.0 {
        return FormantFrame {
            t_secs: 0.0,
            order,
            energy,
            residual: energy,
            formants: Vec::new(),
        };
    }

    let r = autocorrelation(&windowed, order);
    let (a, error) = levinson(&r, order);
    let residual = error / frame.len() as f64;
    let gain = residual.max(0.0).sqrt();

    let mut formants: Vec<Formant> = roots(&a)
        .into_iter()
        .filter(|z| z.im > 0.0)
        .filter_map(|z| {
            let hz = z.im.atan2(z.re) * sample_rate / (2.0 * std::f64::consts::PI);
            let radius = z.abs();
            if !(EDGE_HZ..sample_rate / 2.0 - EDGE_HZ).contains(&hz) || radius >= 1.0 {
                return None;
            }
            Some(Formant {
                hz,
                bandwidth_hz: -radius.ln() * sample_rate / std::f64::consts::PI,
                db: db(gain / response(&a, hz, sample_rate)),
            })
        })
        .collect();
    formants.sort_by(|x, y| x.hz.total_cmp(&y.hz));
    formants.truncate(max_formants);

    FormantFrame {
        t_secs: 0.0,
        order,
        energy,
        residual,
        formants,
    }
}

fn autocorrelation(x: &[f64], lags: usize) -> Vec<f64> {
    (0..=lags)
        .map(|k| x.iter().skip(k).zip(x).map(|(a, b)| a * b).sum())
        .collect()
}

/// Levinson-Durbin: the order-`p` predictor from `p` autocorrelations, no FFT and no matrix.
fn levinson(r: &[f64], order: usize) -> (Vec<f64>, f64) {
    let mut a = vec![0.0; order + 1];
    a[0] = 1.0;
    let mut error = r[0];

    for i in 1..=order {
        if error <= 0.0 {
            return (a, error.max(0.0));
        }
        let acc: f64 = r[i] + (1..i).map(|j| a[j] * r[i - j]).sum::<f64>();
        let k = -acc / error;
        let held: Vec<f64> = a[1..i].to_vec();
        for (j, prior) in held.iter().enumerate() {
            a[j + 1] = prior + k * held[i - 2 - j];
        }
        a[i] = k;
        error *= 1.0 - k * k;
    }
    (a, error.max(0.0))
}

fn response(a: &[f64], hz: f64, sample_rate: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / sample_rate;
    let (mut re, mut im) = (0.0, 0.0);
    for (k, coefficient) in a.iter().enumerate() {
        re += coefficient * (w * k as f64).cos();
        im -= coefficient * (w * k as f64).sin();
    }
    (re * re + im * im).sqrt()
}

#[derive(Clone, Copy, Debug)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn mul(self, o: C) -> C {
        C {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    fn sub(self, o: C) -> C {
        C {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }

    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        match d == 0.0 {
            true => C { re: 0.0, im: 0.0 },
            false => C {
                re: (self.re * o.re + self.im * o.im) / d,
                im: (self.im * o.re - self.re * o.im) / d,
            },
        }
    }

    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
}

/// Durand-Kerner: every root moves at once, so no deflation loses the closely spaced pairs an
/// envelope is made of.
fn roots(a: &[f64]) -> Vec<C> {
    const ROUNDS: usize = 500;
    const SETTLED: f64 = 1e-14;

    let degree = a.len() - 1;
    let seed = C { re: 0.4, im: 0.9 };
    let mut z: Vec<C> = Vec::with_capacity(degree);
    let mut power = C { re: 1.0, im: 0.0 };
    for _ in 0..degree {
        z.push(power);
        power = power.mul(seed);
    }

    for _ in 0..ROUNDS {
        let mut moved: f64 = 0.0;
        for i in 0..degree {
            let mut denominator = C { re: 1.0, im: 0.0 };
            for j in 0..degree {
                if j != i {
                    denominator = denominator.mul(z[i].sub(z[j]));
                }
            }
            let step = evaluate(a, z[i]).div(denominator);
            z[i] = z[i].sub(step);
            moved = moved.max(step.abs());
        }
        if moved < SETTLED {
            break;
        }
    }
    z
}

fn evaluate(a: &[f64], at: C) -> C {
    let mut acc = C { re: 0.0, im: 0.0 };
    for coefficient in a {
        acc = acc.mul(at);
        acc.re += coefficient;
    }
    acc
}
