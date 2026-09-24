// Concern: one hammer/unison-string-set call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Chaigne & Askenfelt 1994's coupled hammer/stiff-string model. A physics citation, not an
//! instrument: nothing here, or anywhere this is wired in, may name one.

use crate::error::SampleError;
use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;
use crate::physics::hammer::Hammer;

#[derive(Clone, Debug, PartialEq)]
pub struct ChaigneAskenfeltParams {
    pub f0: f64,
    pub b: f64,
    pub strike_pos: f64,
    pub vel: f64,
    pub hammer_mass: f64,
    pub hammer_k: f64,
    pub hammer_p: f64,
    pub damp_dc: f64,
    pub damp_freq: f64,
    pub unison_count: f64,
    pub detune: f64,
    pub bridge_coupling: f64,
    /// kg; `0` is massless.
    pub bridge_mass: f64,
    /// Cents off `detune`'s placing.
    pub string_cents: [f64; MAX_UNISON],
    pub string_hammer_k_ratio: [f64; MAX_UNISON],
}

impl ChaigneAskenfeltParams {
    /// Chaigne & Askenfelt's own C4 reference set, at the fundamental asked for.
    pub fn at(f0: f64) -> ChaigneAskenfeltParams {
        ChaigneAskenfeltParams {
            f0,
            b: 0.00021,
            strike_pos: 0.125,
            vel: 3.2,
            hammer_mass: 2.9e-3,
            hammer_k: 2.6646e8,
            hammer_p: 2.5,
            damp_dc: 0.6,
            damp_freq: 1.6e-4,
            unison_count: 1.0,
            detune: 2f64.powf(3.0 / 1200.0),
            bridge_coupling: 1000.0,
            bridge_mass: 0.0,
            string_cents: [0.0; MAX_UNISON],
            string_hammer_k_ratio: [1.0; MAX_UNISON],
        }
    }
}

impl ChaigneAskenfeltParams {
    pub fn valid(&self) -> bool {
        let [c1, c2, c3] = self.string_cents;
        let [k1, k2, k3] = self.string_hammer_k_ratio;
        all(&[
            (self.f0, Positive),
            (self.b, NonNegative),
            (self.strike_pos, OpenUnit),
            (self.vel, Positive),
            (self.hammer_mass, Positive),
            (self.hammer_k, Positive),
            (self.hammer_p, Positive),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
            (self.unison_count, Within(1.0, MAX_UNISON as f64)),
            (self.detune, AtLeast(1.0)),
            (self.bridge_coupling, Positive),
            (self.bridge_mass, NonNegative),
            (c1, Finite),
            (c2, Finite),
            (c3, Finite),
            (k1, Positive),
            (k2, Positive),
            (k3, Positive),
        ])
    }
}

const WIRE_DENSITY_KG_M3: f64 = 7850.0;
/// A generic wire radius, tuned against this module's bridge-force and contact-time tests.
const WIRE_RADIUS_M: f64 = 0.6e-3;
/// A generic string tension, tuned alongside [`WIRE_RADIUS_M`].
const STRING_TENSION_N: f64 = 1500.0;
/// `SharedBridge`: terminated at the site's common bridge, not a rigid pin. Set uniformly
/// across every string in a site; `step()` reads only `strings[0]`'s to dispatch.
#[derive(Clone, Copy)]
enum Termination {
    Rigid,
    SharedBridge,
}

pub(crate) struct StringGrid {
    pub(crate) y_now: Vec<f64>,
    pub(crate) y_prev: Vec<f64>,
    pub(crate) y_next: Vec<f64>,
    pub(crate) n: usize,
    pub(crate) dx: f64,
    pub(crate) rho: f64,
    pub(crate) courant_sq: f64,
    pub(crate) stiff_sq: f64,
    pub(crate) damp_a: f64,
    pub(crate) damp_b: f64,
    far_termination: Termination,
}

/// The ghost a 5-point biharmonic stencil needs past `0` and `n`, far end `pin`.
pub(crate) fn ghost_pinned(y: &[f64], n: usize, idx: isize, pin: f64) -> f64 {
    if idx < 0 {
        -y[(-idx) as usize]
    } else if idx as usize > n {
        2.0 * pin - y[(2 * n as isize - idx) as usize]
    } else {
        y[idx as usize]
    }
}

/// `pin` is the far-end ghost: `0.0` rigid, or a shared bridge's own state.
pub(crate) fn stencil_update(grid: &StringGrid, j: usize, pin_now: f64, pin_prev: f64) -> f64 {
    let n = grid.n;
    let jj = j as isize;
    let lap_now = ghost_pinned(&grid.y_now, n, jj + 1, pin_now) - 2.0 * grid.y_now[j]
        + ghost_pinned(&grid.y_now, n, jj - 1, pin_now);
    let biharm = ghost_pinned(&grid.y_now, n, jj + 2, pin_now)
        - 4.0 * ghost_pinned(&grid.y_now, n, jj + 1, pin_now)
        + 6.0 * grid.y_now[j]
        - 4.0 * ghost_pinned(&grid.y_now, n, jj - 1, pin_now)
        + ghost_pinned(&grid.y_now, n, jj - 2, pin_now);
    let lap_prev = ghost_pinned(&grid.y_prev, n, jj + 1, pin_prev) - 2.0 * grid.y_prev[j]
        + ghost_pinned(&grid.y_prev, n, jj - 1, pin_prev);
    2.0 * grid.y_now[j] - grid.y_prev[j] + grid.courant_sq * lap_now
        - grid.stiff_sq * biharm
        - grid.damp_a * (grid.y_now[j] - grid.y_prev[j])
        + grid.damp_b * (lap_now - lap_prev)
}

/// The most intervals the lossless scheme keeps stable.
fn finest_stable_points(c: f64, length: f64, kappa: f64, dt: f64) -> usize {
    let (c2, dt2, k2) = (c * c, dt * dt, kappa * kappa);
    let dx_bound = ((c2 * dt2 + (c2 * c2 * dt2 * dt2 + 16.0 * k2 * dt2).sqrt()) / 2.0).sqrt();
    (length / dx_bound).floor() as usize
}

/// At rest, losses set; the caller sets the restoring terms.
fn lossy_grid(
    n: usize,
    rho: f64,
    length: f64,
    damp_dc: f64,
    damp_freq: f64,
    dt: f64,
) -> StringGrid {
    let dx = length / n as f64;
    StringGrid {
        y_now: vec![0.0; n + 1],
        y_prev: vec![0.0; n + 1],
        y_next: vec![0.0; n + 1],
        n,
        dx,
        rho,
        courant_sq: 0.0,
        stiff_sq: 0.0,
        damp_a: 2.0 * damp_dc * dt,
        damp_b: 2.0 * damp_freq * dt / (dx * dx),
        far_termination: Termination::Rigid,
    }
}

/// Pinned mode `m` is exactly `sin(m pi j/n)`.
fn mode_s(m: usize, n: usize) -> f64 {
    (m as f64 * std::f64::consts::PI / (2.0 * n as f64))
        .sin()
        .powi(2)
}

fn mode_sigma(grid: &StringGrid, s: f64) -> f64 {
    grid.damp_a + 4.0 * grid.damp_b * s
}

/// `2 - sigma - 2 sqrt(1 - sigma) cos(theta)`, without cancellation.
fn stiffness_for(theta: f64, sigma: f64) -> f64 {
    let r = (1.0 - sigma).sqrt();
    (sigma / (1.0 + r)).powi(2) + 4.0 * r * (theta / 2.0).sin().powi(2)
}

/// Jury's test on `z^2 + (D + sigma - 2) z + 1 - sigma`, every mode.
fn is_stable(grid: &StringGrid) -> bool {
    (1..grid.n).all(|m| {
        let s = mode_s(m, grid.n);
        let sigma = mode_sigma(grid, s);
        let d = 4.0 * grid.courant_sq * s + 16.0 * grid.stiff_sq * s * s;
        (0.0..2.0).contains(&sigma) && d > 0.0 && d < 4.0 - 2.0 * sigma
    })
}

/// `L = c/(2 f0)` on a generic wire.
fn build_grid(params: &ChaigneAskenfeltParams, f0: f64, sr: f64) -> Option<StringGrid> {
    let rho = std::f64::consts::PI * WIRE_RADIUS_M * WIRE_RADIUS_M * WIRE_DENSITY_KG_M3;
    let c = (STRING_TENSION_N / rho).sqrt();
    let length = c / (2.0 * f0);
    dispersive_grid(
        Wire { rho, c, length },
        f0,
        params.b,
        params.damp_dc,
        params.damp_freq,
        sr,
    )
}

pub(crate) struct Wire {
    pub(crate) rho: f64,
    pub(crate) c: f64,
    pub(crate) length: f64,
}

/// Partials 1 and 2 at `k f0 sqrt(1 + b k^2)`, on the finest grid that can.
pub(crate) fn dispersive_grid(
    wire: Wire,
    f0: f64,
    b: f64,
    damp_dc: f64,
    damp_freq: f64,
    sr: f64,
) -> Option<StringGrid> {
    let dt = 1.0 / sr;
    let Wire { rho, c, length } = wire;
    let kappa = c * length * b.sqrt() / std::f64::consts::PI;
    let theta = |k: f64| std::f64::consts::TAU * f0 * k * (1.0 + b * k * k).sqrt() * dt;
    let (theta_1, theta_2) = (theta(1.0), theta(2.0));
    if theta_2 >= std::f64::consts::PI {
        return None;
    }
    (3..=finest_stable_points(c, length, kappa, dt))
        .rev()
        .find_map(|n| {
            let mut grid = lossy_grid(n, rho, length, damp_dc, damp_freq, dt);
            let (s1, s2) = (mode_s(1, n), mode_s(2, n));
            let (sigma_1, sigma_2) = (mode_sigma(&grid, s1), mode_sigma(&grid, s2));
            if sigma_1 >= 1.0 || sigma_2 >= 1.0 {
                return None;
            }
            let (d1, d2) = (
                stiffness_for(theta_1, sigma_1),
                stiffness_for(theta_2, sigma_2),
            );
            // `D_m = 4 lambda^2 s_m + 16 mu^2 s_m^2`, m = 1, 2.
            let det = s1 * s2 * (s2 - s1);
            grid.courant_sq = (d1 * s2 * s2 - d2 * s1 * s1) / (4.0 * det);
            grid.stiff_sq = (s1 * d2 - s2 * d1) / (16.0 * det);
            (grid.courant_sq > 0.0 && is_stable(&grid)).then_some(grid)
        })
}

/// `rho (lambda dx/dt)^2`.
pub(crate) fn grid_tension(grid: &StringGrid, dt: f64) -> f64 {
    grid.rho * grid.courant_sq * grid.dx * grid.dx / (dt * dt)
}

/// Modal content `sin(m pi x)` for every grid mode; `1` alone on a node. Read and spread alike.
pub(crate) fn point_weights(n: usize, x: f64) -> Vec<f64> {
    let nf = n as f64;
    // sum_{m=1}^{n-1} cos(m theta)
    let cos_sum = |theta: f64| {
        let half = theta / 2.0;
        let sh = half.sin();
        if sh == 0.0 {
            nf - 1.0
        } else {
            (nf * half).sin() * ((nf - 1.0) * half).cos() / sh - 1.0
        }
    };
    (0..=n)
        .map(|j| match j {
            0 => 0.0,
            j if j == n => 0.0,
            j => {
                let xj = j as f64 / nf;
                let pi = std::f64::consts::PI;
                (cos_sum(pi * (x - xj)) - cos_sum(pi * (x + xj))) / nf
            }
        })
        .collect()
}

/// Strings placed symmetrically in log frequency around `f0` at `+-sqrt(detune)`, then each
/// moved by its own `cents`.
fn unison_frequencies(f0: f64, detune: f64, unison_count: usize, cents: &[f64]) -> Vec<f64> {
    let spread = detune.sqrt();
    let placed = match unison_count {
        1 => vec![f0],
        2 => vec![f0 / spread, f0 * spread],
        _ => vec![f0 / spread, f0, f0 * spread],
    };
    placed
        .iter()
        .zip(cents)
        .map(|(&f, &c)| f * 2f64.powf(c / 1200.0))
        .collect()
}

/// Bounds `valid()`, so the per-string force buffer can be a stack array.
const MAX_UNISON: usize = 3;

pub struct ChaigneAskenfeltSite {
    strings: Vec<StringGrid>,
    /// Per string, `rho (lambda dx/dt)^2`.
    tensions: Vec<f64>,
    strike: Vec<Vec<f64>>,
    hammer: Hammer,
    detached: Vec<bool>,
    bridge_now: f64,
    bridge_prev: f64,
    bridge_coupling: f64,
    bridge_mass: f64,
    dt: f64,
}

impl ChaigneAskenfeltSite {
    pub fn new(params: &ChaigneAskenfeltParams, sr: f64) -> Result<Self, SampleError> {
        let unison_count = params.unison_count.round().clamp(1.0, MAX_UNISON as f64) as usize;
        let freqs =
            unison_frequencies(params.f0, params.detune, unison_count, &params.string_cents);
        let mut strings = freqs
            .iter()
            .map(|&f0| build_grid(params, f0, sr))
            .collect::<Option<Vec<StringGrid>>>()
            .ok_or(SampleError::StringPastRate {
                model: "chaigne_askenfelt",
            })?;
        if strings.len() > 1 {
            for grid in &mut strings {
                grid.far_termination = Termination::SharedBridge;
            }
        }
        let dt = 1.0 / sr;
        let tensions = strings.iter().map(|g| grid_tension(g, dt)).collect();
        let strike = strings
            .iter()
            .map(|g| point_weights(g.n, params.strike_pos))
            .collect();
        Ok(ChaigneAskenfeltSite {
            detached: vec![false; strings.len()],
            strings,
            tensions,
            strike,
            hammer: Hammer::new(
                params.hammer_mass,
                params.hammer_k,
                params.hammer_p,
                params.vel,
                dt,
            )
            .with_anvil_ratios(&params.string_hammer_k_ratio[..unison_count]),
            bridge_now: 0.0,
            bridge_prev: 0.0,
            bridge_coupling: params.bridge_coupling,
            bridge_mass: params.bridge_mass,
            dt,
        })
    }
}

impl Solver for ChaigneAskenfeltSite {
    fn step(&mut self) -> f64 {
        match self.strings[0].far_termination {
            Termination::Rigid => self.step_single(),
            Termination::SharedBridge => self.step_unison(),
        }
    }
}

impl ChaigneAskenfeltSite {
    /// A released anvil is not read.
    fn hammer_forces(&mut self, forces: &mut [f64]) {
        let mut y_h = [0.0f64; MAX_UNISON];
        for (i, grid) in self.strings.iter().enumerate() {
            if !self.detached[i] {
                y_h[i] = read_at(&self.strike[i], &grid.y_now);
            }
        }
        self.hammer.substeps(
            self.dt,
            &y_h[..self.strings.len()],
            &mut self.detached,
            forces,
        );
    }

    fn step_single(&mut self) -> f64 {
        let mut forces = [0.0f64; 1];
        self.hammer_forces(&mut forces);
        let grid = &mut self.strings[0];
        let n = grid.n;
        for i in 1..n {
            grid.y_next[i] = stencil_update(grid, i, 0.0, 0.0);
        }
        spread(grid, &self.strike[0], forces[0], self.dt);
        grid.y_next[0] = 0.0;
        grid.y_next[n] = 0.0;

        let sample = self.tensions[0] * (grid.y_now[n] - grid.y_now[n - 1]) / grid.dx;

        std::mem::swap(&mut grid.y_prev, &mut grid.y_now);
        std::mem::swap(&mut grid.y_now, &mut grid.y_next);

        sample
    }

    /// The hammer's reaction is summed over the strings, not divided.
    fn step_unison(&mut self) -> f64 {
        let mut forces = [0.0f64; MAX_UNISON];
        let forces = &mut forces[..self.strings.len()];
        self.hammer_forces(forces);

        let (bridge_now, bridge_prev) = (self.bridge_now, self.bridge_prev);
        // Bridge `M a + R_B v = net string force`, implicit in its next position like the strings.
        let mut k_eff = 0.0;
        let mut rhs_sum = 0.0;
        let mut sample = 0.0;
        for (i, &force) in forces.iter().enumerate() {
            let grid = &mut self.strings[i];
            let tension = self.tensions[i];
            let n = grid.n;
            for j in 1..n {
                grid.y_next[j] = stencil_update(grid, j, bridge_now, bridge_prev);
            }
            spread(grid, &self.strike[i], force, self.dt);
            grid.y_next[0] = 0.0;
            k_eff += tension / grid.dx;
            rhs_sum += tension * grid.y_now[n - 1] / grid.dx;
            sample += tension * (grid.y_now[n] - grid.y_now[n - 1]) / grid.dx;
        }

        let z_string = (self.tensions[0] * self.strings[0].rho).sqrt();
        let r_bridge = self.bridge_coupling * z_string;
        let r_over_dt = r_bridge / self.dt;
        let m_over_dt2 = self.bridge_mass / (self.dt * self.dt);
        let bridge_next =
            (rhs_sum + r_over_dt * bridge_now + m_over_dt2 * (2.0 * bridge_now - bridge_prev))
                / (k_eff + r_over_dt + m_over_dt2);
        for grid in &mut self.strings {
            let n = grid.n;
            grid.y_next[n] = bridge_next;
        }

        self.bridge_prev = bridge_now;
        self.bridge_now = bridge_next;

        for grid in &mut self.strings {
            std::mem::swap(&mut grid.y_prev, &mut grid.y_now);
            std::mem::swap(&mut grid.y_now, &mut grid.y_next);
        }

        sample
    }
}

/// `spread`'s adjoint.
pub(crate) fn read_at(weights: &[f64], y: &[f64]) -> f64 {
    weights.iter().zip(y).map(|(w, y)| w * y).sum()
}

/// The readout's adjoint.
pub(crate) fn spread(grid: &mut StringGrid, weights: &[f64], force: f64, dt: f64) {
    if force == 0.0 {
        return;
    }
    let scale = (dt * dt / (grid.rho * grid.dx)) * force;
    for (y, w) in grid.y_next.iter_mut().zip(weights) {
        *y += scale * w;
    }
}
