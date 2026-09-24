// Concern: one hammer/unison-string-set call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Chaigne & Askenfelt 1994's coupled hammer/stiff-string model. A physics citation, not an
//! instrument: nothing here, or anywhere this is wired in, may name one.

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

pub(crate) fn stiff_string_grid(
    rho: f64,
    c: f64,
    length: f64,
    kappa: f64,
    damp_dc: f64,
    damp_freq: f64,
    sr: f64,
) -> StringGrid {
    let dt = 1.0 / sr;
    let (c2, dt2, k2) = (c * c, dt * dt, kappa * kappa);
    let dx_bound = ((c2 * dt2 + (c2 * c2 * dt2 * dt2 + 16.0 * k2 * dt2).sqrt()) / 2.0).sqrt();
    let n = ((length / dx_bound).floor() as usize).max(4);
    let dx = length / n as f64;

    StringGrid {
        y_now: vec![0.0; n + 1],
        y_prev: vec![0.0; n + 1],
        y_next: vec![0.0; n + 1],
        n,
        dx,
        rho,
        courant_sq: c2 * dt2 / (dx * dx),
        stiff_sq: k2 * dt2 / dx.powi(4),
        damp_a: 2.0 * damp_dc * dt,
        damp_b: 2.0 * damp_freq * dt / (dx * dx),
        far_termination: Termination::Rigid,
    }
}

/// Derives a concrete grid from `(f0, b)`: fixes a generic wire's `c`, scales this note's own
/// length `L = c/(2 f0)`, its stiffness `kappa = c L sqrt(b)/pi`, and a CFL-stable `dx`.
fn build_grid(params: &ChaigneAskenfeltParams, f0: f64, sr: f64) -> StringGrid {
    let rho = std::f64::consts::PI * WIRE_RADIUS_M * WIRE_RADIUS_M * WIRE_DENSITY_KG_M3;
    let c = (STRING_TENSION_N / rho).sqrt();
    let length = c / (2.0 * f0);
    let kappa = c * length * params.b.sqrt() / std::f64::consts::PI;
    stiff_string_grid(rho, c, length, kappa, params.damp_dc, params.damp_freq, sr)
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
    hammer: Hammer,
    detached: bool,
    contact_index: usize,
    contact_indices: Vec<usize>,
    strings_detached: Vec<bool>,
    bridge_now: f64,
    bridge_prev: f64,
    bridge_coupling: f64,
    bridge_mass: f64,
    tension: f64,
    dt: f64,
}

impl ChaigneAskenfeltSite {
    pub fn new(params: &ChaigneAskenfeltParams, sr: f64) -> ChaigneAskenfeltSite {
        let unison_count = params.unison_count.round().clamp(1.0, MAX_UNISON as f64) as usize;
        let freqs =
            unison_frequencies(params.f0, params.detune, unison_count, &params.string_cents);
        let mut strings: Vec<StringGrid> =
            freqs.iter().map(|&f0| build_grid(params, f0, sr)).collect();
        if strings.len() > 1 {
            for grid in &mut strings {
                grid.far_termination = Termination::SharedBridge;
            }
        }
        let contact_indices: Vec<usize> = strings
            .iter()
            .map(|g| {
                (params.strike_pos * g.n as f64)
                    .round()
                    .clamp(1.0, (g.n - 1) as f64) as usize
            })
            .collect();
        let contact_index = contact_indices[0];
        let strings_detached = vec![false; strings.len()];
        let dt = 1.0 / sr;
        ChaigneAskenfeltSite {
            strings,
            hammer: Hammer::new(
                params.hammer_mass,
                params.hammer_k,
                params.hammer_p,
                params.vel,
                dt,
            )
            .with_anvil_ratios(&params.string_hammer_k_ratio[..unison_count]),
            detached: false,
            contact_index,
            contact_indices,
            strings_detached,
            bridge_now: 0.0,
            bridge_prev: 0.0,
            bridge_coupling: params.bridge_coupling,
            bridge_mass: params.bridge_mass,
            tension: STRING_TENSION_N,
            dt,
        }
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
    /// The contact point is frozen at this sample's start.
    fn step_single(&mut self) -> f64 {
        let grid = &mut self.strings[0];
        let n = grid.n;
        let y_h = grid.y_now[self.contact_index];

        let mut forces = [0.0f64; 1];
        self.hammer.substeps(
            self.dt,
            &[y_h],
            std::slice::from_mut(&mut self.detached),
            &mut forces,
        );
        let force = forces[0];

        for i in 1..n {
            let mut next = stencil_update(grid, i, 0.0, 0.0);
            if i == self.contact_index {
                next += (self.dt * self.dt / (grid.rho * grid.dx)) * force;
            }
            grid.y_next[i] = next;
        }
        grid.y_next[0] = 0.0;
        grid.y_next[n] = 0.0;

        let sample = self.tension * (grid.y_now[n] - grid.y_now[n - 1]) / grid.dx;

        std::mem::swap(&mut grid.y_prev, &mut grid.y_now);
        std::mem::swap(&mut grid.y_now, &mut grid.y_next);

        sample
    }

    /// One hammer against every contact point: the reaction is summed, not divided.
    fn step_unison(&mut self) -> f64 {
        let count = self.strings.len();

        let y_h: Vec<f64> = (0..count)
            .map(|i| self.strings[i].y_now[self.contact_indices[i]])
            .collect();

        let mut forces = [0.0f64; MAX_UNISON];
        let forces = &mut forces[..count];
        self.hammer
            .substeps(self.dt, &y_h, &mut self.strings_detached, forces);

        let (bridge_now, bridge_prev) = (self.bridge_now, self.bridge_prev);
        // Bridge `M a + R_B v = net string force`, implicit in its next position like the strings.
        let mut k_eff = 0.0;
        let mut rhs_sum = 0.0;
        let mut sample = 0.0;
        for (i, &force) in forces.iter().enumerate() {
            let grid = &mut self.strings[i];
            let n = grid.n;
            for j in 1..n {
                let mut next = stencil_update(grid, j, bridge_now, bridge_prev);
                if j == self.contact_indices[i] {
                    next += (self.dt * self.dt / (grid.rho * grid.dx)) * force;
                }
                grid.y_next[j] = next;
            }
            grid.y_next[0] = 0.0;
            k_eff += self.tension / grid.dx;
            rhs_sum += self.tension * grid.y_now[n - 1] / grid.dx;
            sample += self.tension * (grid.y_now[n] - grid.y_now[n - 1]) / grid.dx;
        }

        let z_string = (self.tension * self.strings[0].rho).sqrt();
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
