// Concern: a pinned stiff string's discrete energy and the bound its modes give every later sample | Non-concern: stepping the grid (stiff_string.rs), the strike | IO: (&StringGrid) -> energy, bound

//! Mode `m` of a free pinned grid steps as `q+ = (2 - D - sigma) q - (1 - sigma) q-`, with
//! `D = 4 lambda^2 s + 16 mu^2 s^2`, `sigma = a + 4 b s`, `s = sin^2(m pi/2n)`.

use crate::physics::stiff_string::{StringGrid, mode_s};

const U: f64 = f64::EPSILON / 2.0;

pub(crate) fn gamma(n: f64) -> f64 {
    n * U / (1.0 - n * U)
}

struct Mode {
    d: f64,
    sigma: f64,
}

fn modes(grid: &StringGrid) -> Vec<Mode> {
    (1..grid.n)
        .map(|m| {
            let s = mode_s(m, grid.n);
            Mode {
                d: 4.0 * grid.courant_sq * s + 16.0 * grid.stiff_sq * s * s,
                sigma: grid.damp_a + 4.0 * grid.damp_b * s,
            }
        })
        .collect()
}

/// Each within `24 u`.
fn sines(n: usize) -> Vec<f64> {
    (0..2 * n)
        .map(|r| (std::f64::consts::PI * r as f64 / n as f64).sin())
        .collect()
}

/// `q_m = (2/n) sum_j y_j sin(m pi j/n)`, and its error.
fn modal(y: &[f64], n: usize, table: &[f64]) -> (Vec<f64>, f64) {
    let scale = 2.0 / n as f64;
    let q = (1..n)
        .map(|m| scale * (1..n).map(|j| y[j] * table[(m * j) % (2 * n)]).sum::<f64>())
        .collect();
    let mass: f64 = y[1..n].iter().map(|v| v.abs()).sum();
    (q, scale * mass * gamma(n as f64 + 32.0))
}

pub(crate) fn stencil_mass(grid: &StringGrid) -> f64 {
    3.0 + 4.0 * grid.courant_sq + 16.0 * grid.stiff_sq + 2.0 * grid.damp_a + 8.0 * grid.damp_b
}

fn slopes(y: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (0..n).map(move |j| pinned(y, n, j + 1) - pinned(y, n, j))
}

fn curvatures(y: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (1..n).map(move |j| pinned(y, n, j + 1) - 2.0 * y[j] + pinned(y, n, j - 1))
}

fn pinned(y: &[f64], n: usize, j: usize) -> f64 {
    match j {
        0 => 0.0,
        j if j == n => 0.0,
        j => y[j],
    }
}

/// `E = w (v'(I - A/2)v + y'K y-)/2` joules, `v = y - y-`, `w = rho dx/dt^2`.
pub(crate) fn energy(grid: &StringGrid, dt: f64) -> f64 {
    let (n, y, yp) = (grid.n, &grid.y_now, &grid.y_prev);
    let v: Vec<f64> = y.iter().zip(yp).map(|(a, b)| a - b).collect();
    let kinetic = (1.0 - grid.damp_a / 2.0) * v[1..n].iter().map(|x| x * x).sum::<f64>()
        - grid.damp_b / 2.0 * slopes(&v, n).map(|x| x * x).sum::<f64>();
    let tension: f64 = slopes(y, n).zip(slopes(yp, n)).map(|(a, b)| a * b).sum();
    let bending: f64 = curvatures(y, n)
        .zip(curvatures(yp, n))
        .map(|(a, b)| a * b)
        .sum();
    let potential = grid.courant_sq * tension + grid.stiff_sq * bending;
    grid.rho * grid.dx / (dt * dt) * (kinetic + potential) / 2.0
}

/// `c` with `gain |y_{n-1}| <= c sqrt(E)`: `|q_m| <= sqrt(H_m (1/alpha + 1/beta)/2)`,
/// `alpha = 1 - sigma/2 - D/4`, `beta = D/4`, and `sum H_m = 2E/(w n)`.
pub(crate) fn energy_gain(grid: &StringGrid, gain: f64, dt: f64) -> f64 {
    let n = grid.n;
    let table = sines(n);
    let w = grid.rho * grid.dx / (dt * dt);
    let weight: f64 = modes(grid)
        .iter()
        .enumerate()
        .map(|(i, mode)| {
            let g = table[i + 1].abs() + 32.0 * U;
            let alpha =
                1.0 - mode.sigma / 2.0 - mode.d / 4.0 - gamma(8.0) * (1.0 + mode.sigma + mode.d);
            let beta = mode.d / 4.0 * (1.0 - gamma(8.0));
            g * g * (1.0 / alpha + 1.0 / beta) / 2.0
        })
        .sum();
    gain * (2.0 * weight / (w * n as f64)).sqrt() * (1.0 + gamma(4.0 * n as f64 + 64.0))
}

/// Each mode's `A_m r_m^k`, and the rounding every step adds, decaying at `rate`.
pub(crate) struct Ringdown {
    terms: Vec<(f64, f64)>,
    slack: f64,
    rate: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unringing {
    Lossless,
    Critical,
    Rounding,
}

impl Ringdown {
    /// `|q_k| <= A r^k`, `A^2 = r^2 ((q - c q-)^2/Delta + q-^2)`, `c = 1 - (D + sigma)/2`,
    /// `Delta = D - (D + sigma)^2/4`. A step's rounding reaches a mode as at most `2 eps Y`,
    /// `Y` the largest node, and rings on by `kappa = r/sqrt(Delta)`, so `Y_k <= Z rho'^k`.
    pub(crate) fn of(grid: &StringGrid, gain: f64) -> Result<Ringdown, Unringing> {
        let n = grid.n;
        let table = sines(n);
        let (q, err_q) = modal(&grid.y_now, n, &table);
        let (q_prev, err_prev) = modal(&grid.y_prev, n, &table);
        let gain = gain * (1.0 + gamma(4.0));
        let mut rings = Vec::with_capacity(n - 1);
        for (i, mode) in modes(grid).iter().enumerate() {
            let (d, sigma) = (mode.d, mode.sigma);
            if sigma <= 0.0 {
                return Err(Unringing::Lossless);
            }
            let spread = d + (d + sigma) * (d + sigma);
            let delta = d - (d + sigma) * (d + sigma) / 4.0 - gamma(32.0) * spread;
            if delta <= 1e-6 * d {
                return Err(Unringing::Critical);
            }
            let r = (1.0 - sigma * (1.0 - gamma(8.0))).sqrt() * (1.0 + gamma(2.0));
            let c = 1.0 - (d + sigma) / 2.0;
            let c_err = gamma(8.0) * (1.0 + d + sigma);
            let lead = (q[i] - c * q_prev[i]).abs()
                + err_q
                + (c.abs() + c_err) * err_prev
                + c_err * q_prev[i].abs();
            let lag = q_prev[i].abs() + err_prev;
            let amplitude = r * (lead * lead / delta + lag * lag).sqrt() * (1.0 + gamma(16.0));
            let g = table[i + 1].abs() + 32.0 * U;
            rings.push((amplitude, r, r / delta.sqrt(), g, lag));
        }
        let r_max = rings.iter().map(|m| m.1).fold(0.0f64, f64::max);
        if r_max >= 1.0 {
            return Err(Unringing::Lossless);
        }
        let rate = (1.0 + r_max) / 2.0;
        let reach = |(_, r, kappa, _, _): &(f64, f64, f64, f64, f64)| kappa / (rate * (rate - r));
        let carried: f64 = rings.iter().map(reach).sum::<f64>() * (1.0 + gamma(n as f64 + 8.0));
        let eps = 2.0 * gamma(64.0) * stencil_mass(grid);
        if eps * carried >= 0.5 {
            return Err(Unringing::Rounding);
        }
        let start = rings.iter().map(|m| m.0).sum::<f64>();
        let before = rate * rings.iter().map(|m| m.4).sum::<f64>();
        let z = start.max(before) / (1.0 - eps * carried) * (1.0 + gamma(n as f64 + 8.0));
        let slack = gain
            * eps
            * z
            * rings.iter().map(|m| m.3 * reach(m)).sum::<f64>()
            * (1.0 + gamma(n as f64 + 8.0));
        Ok(Ringdown {
            terms: rings.iter().map(|m| (gain * m.3 * m.0, m.1)).collect(),
            slack,
            rate,
        })
    }

    pub(crate) fn along(&self, first: usize, step: usize, count: usize) -> Vec<f64> {
        let mut out = vec![0.0f64; count];
        let widen = 1.0 + 4.0 * U;
        let mut add = |scale: f64, r: f64| {
            let per = r.powi(step as i32) * (1.0 + gamma(4.0 * step as f64));
            let mut held = scale * r.powf(first as f64) * (1.0 + gamma(4.0));
            for slot in out.iter_mut() {
                *slot += held;
                held = held * per * widen;
            }
        };
        for &(scale, r) in &self.terms {
            add(scale, r);
        }
        add(self.slack, self.rate);
        let summed = 1.0 + gamma(self.terms.len() as f64 + 2.0);
        out.iter().map(|v| v * summed).collect()
    }
}
