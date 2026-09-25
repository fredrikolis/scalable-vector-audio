// Concern: one hammer/unison-string-set call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Chaigne & Askenfelt 1994's coupled hammer/stiff-string model. A physics citation, not an
//! instrument: nothing here, or anywhere this is wired in, may name one.

use crate::error::SampleError;
use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;
use crate::physics::hammer::Hammer;
use crate::physics::stiff_string::{
    StringGrid, Wire, dispersive_grid, grid_tension, point_weights, read_at, spread, stencil_update,
};

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
        let strings = freqs
            .iter()
            .map(|&f0| build_grid(params, f0, sr))
            .collect::<Option<Vec<StringGrid>>>()
            .ok_or(SampleError::StringPastRate {
                model: "chaigne_askenfelt",
            })?;
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

impl ChaigneAskenfeltSite {
    fn advance(&mut self) -> f64 {
        // One string ends on a rigid pin; a unison ends on its shared bridge.
        match self.strings.len() {
            1 => self.step_single(),
            _ => self.step_unison(),
        }
    }
}

impl Solver for ChaigneAskenfeltSite {
    fn step(&mut self) -> Result<f64, SampleError> {
        Ok(self.advance())
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
