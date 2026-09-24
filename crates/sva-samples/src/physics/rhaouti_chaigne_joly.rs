// Concern: one hammer/membrane call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Rhaouti, Chaigne & Joly, JASA 105(6) (1999) 3545-3562, doi 10.1121/1.424679. eq. 3
//! `F = K[(delta - u + W)+]^alpha` is `hammer.rs`; eq. 4-6's `W` windows a contact patch.

use crate::error::SampleError;
use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;
use crate::physics::hammer::Hammer;

#[derive(Clone, Debug, PartialEq)]
pub struct RhaoutiChaigneJolyParams {
    pub f0: f64,
    pub aspect_ratio: f64,
    pub strike_x: f64,
    pub strike_y: f64,
    pub vel: f64,
    pub hammer_mass: f64,
    pub hammer_k: f64,
    pub hammer_p: f64,
    pub damp_dc: f64,
    pub damp_freq: f64,
}

impl RhaoutiChaigneJolyParams {
    /// The strike sits off-centre so no low-order mode is silenced.
    pub fn at(f0: f64) -> RhaoutiChaigneJolyParams {
        RhaoutiChaigneJolyParams {
            f0,
            aspect_ratio: 1.0,
            strike_x: 0.3,
            strike_y: 0.4,
            vel: 3.2,
            hammer_mass: 2.9e-3,
            hammer_k: 2.6646e8,
            hammer_p: 2.5,
            damp_dc: 0.6,
            damp_freq: 1.6e-4,
        }
    }
}

impl RhaoutiChaigneJolyParams {
    pub fn valid(&self) -> bool {
        all(&[
            (self.f0, Positive),
            (self.aspect_ratio, Positive),
            (self.strike_x, OpenUnit),
            (self.strike_y, OpenUnit),
            (self.vel, Positive),
            (self.hammer_mass, Positive),
            (self.hammer_k, Positive),
            (self.hammer_p, Positive),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
        ])
    }
}

/// A small tense drumhead's tension, tuned alongside [`MEMBRANE_AREAL_DENSITY_KG_M2`].
const MEMBRANE_TENSION_N_M: f64 = 3000.0;
/// Gives `c = sqrt(T/sigma) = 100 m/s`; order of magnitude only.
const MEMBRANE_AREAL_DENSITY_KG_M2: f64 = 0.3;
/// Row-major `iy * (nx + 1) + ix`; the edges are the clamped boundary.
struct MembraneGrid {
    u_now: Vec<f64>,
    u_prev: Vec<f64>,
    u_next: Vec<f64>,
    nx: usize,
    ny: usize,
    h: f64,
    sigma: f64,
    /// `lambda = c*dt/h`; Bilbao eq. 11.13 bounds 2D at `1/sqrt(2)`, not 1D's `1`.
    courant_sq: f64,
    damp_a: f64,
    damp_b: f64,
}

#[inline]
fn node_index(nx: usize, ix: usize, iy: usize) -> usize {
    iy * (nx + 1) + ix
}

/// RCJ Table I's window, `g(x,y) = exp[-C((x-x0)^4+(y-y0)^4)]`, `C` in `1/m^4`.
const MALLET_WINDOW_QUARTIC_COEFF_M4: f64 = 1.0e7;
/// Below this fraction of the window's peak, a node leaves the patch.
const MALLET_WINDOW_WEIGHT_FLOOR: f64 = 1e-8;

/// Weights (sum to 1, RCJ eq. 4) shared by the read average and force spread.
struct ContactPatch {
    nodes: Vec<usize>,
    weights: Vec<f64>,
}

fn build_contact_patch(
    nx: usize,
    ny: usize,
    contact_ix: usize,
    contact_iy: usize,
    h: f64,
) -> ContactPatch {
    let cutoff_m =
        (MALLET_WINDOW_WEIGHT_FLOOR.ln().abs() / MALLET_WINDOW_QUARTIC_COEFF_M4).powf(0.25);
    let radius_cells = (cutoff_m / h).ceil() as isize;

    let mut nodes = Vec::new();
    let mut weights = Vec::new();
    let mut total = 0.0;
    for dj in -radius_cells..=radius_cells {
        for di in -radius_cells..=radius_cells {
            let (ix, iy) = (contact_ix as isize + di, contact_iy as isize + dj);
            if ix < 1 || ix > nx as isize - 1 || iy < 1 || iy > ny as isize - 1 {
                continue;
            }
            let (dx, dy) = (di as f64 * h, dj as f64 * h);
            let w = (-MALLET_WINDOW_QUARTIC_COEFF_M4 * (dx.powi(4) + dy.powi(4))).exp();
            if w < MALLET_WINDOW_WEIGHT_FLOOR {
                continue;
            }
            nodes.push(node_index(nx, ix as usize, iy as usize));
            weights.push(w);
            total += w;
        }
    }
    for w in &mut weights {
        *w /= total;
    }
    ContactPatch { nodes, weights }
}

/// The CFL bound at a 90% margin.
const LAMBDA_TARGET: f64 = 0.9 * std::f64::consts::FRAC_1_SQRT_2;

/// Nodes, not hertz: the rate decides as much as `f0`. Three `f64` planes, so ~100 MB.
pub const NODE_COUNT_CEILING: usize = 4_000_000;

/// Grows as `1/f0^2`.
pub fn node_count(params: &RhaoutiChaigneJolyParams, sr: f64) -> f64 {
    let (lx, ly) = sides(params);
    let h = step(MEMBRANE_TENSION_N_M, MEMBRANE_AREAL_DENSITY_KG_M2, sr);
    (axis_nodes(lx, h) + 1.0) * (axis_nodes(ly, h) + 1.0)
}

fn step(tension: f64, sigma: f64, sr: f64) -> f64 {
    (tension / sigma).sqrt() * (1.0 / sr) / LAMBDA_TARGET
}

fn axis_nodes(span: f64, h: f64) -> f64 {
    (span / h).round().max(4.0)
}

fn membrane_grid(
    sigma: f64,
    tension: f64,
    lx: f64,
    ly: f64,
    damp_dc: f64,
    damp_freq: f64,
    sr: f64,
) -> MembraneGrid {
    let dt = 1.0 / sr;
    let h = step(tension, sigma, sr);
    let nx = axis_nodes(lx, h) as usize;
    let ny = axis_nodes(ly, h) as usize;
    let courant_sq = LAMBDA_TARGET * LAMBDA_TARGET;

    let count = (nx + 1) * (ny + 1);
    MembraneGrid {
        u_now: vec![0.0; count],
        u_prev: vec![0.0; count],
        u_next: vec![0.0; count],
        nx,
        ny,
        h,
        sigma,
        courant_sq,
        damp_a: 2.0 * damp_dc * dt,
        damp_b: 2.0 * damp_freq * dt / (h * h),
    }
}

/// Bilbao eq. 11.8: `L0 = c/(f0*sqrt(2))`, split as `L0*sqrt(ar)` by `L0/sqrt(ar)` so the
/// area holds at any `ar` (his Sec. 10.1 convention).
fn sides(params: &RhaoutiChaigneJolyParams) -> (f64, f64) {
    let c = (MEMBRANE_TENSION_N_M / MEMBRANE_AREAL_DENSITY_KG_M2).sqrt();
    let l0 = c / (params.f0 * std::f64::consts::SQRT_2);
    (
        l0 * params.aspect_ratio.sqrt(),
        l0 / params.aspect_ratio.sqrt(),
    )
}

fn build_grid(params: &RhaoutiChaigneJolyParams, sr: f64) -> MembraneGrid {
    let (lx, ly) = sides(params);
    membrane_grid(
        MEMBRANE_AREAL_DENSITY_KG_M2,
        MEMBRANE_TENSION_N_M,
        lx,
        ly,
        params.damp_dc,
        params.damp_freq,
        sr,
    )
}

pub struct RhaoutiChaigneJolySite {
    grid: MembraneGrid,
    hammer: Hammer,
    detached: bool,
    contact_patch: ContactPatch,
    pickup_ix: usize,
    pickup_iy: usize,
    dt: f64,
}

impl RhaoutiChaigneJolySite {
    pub fn new(params: &RhaoutiChaigneJolyParams, sr: f64) -> RhaoutiChaigneJolySite {
        let grid = build_grid(params, sr);
        let contact_ix = (params.strike_x * grid.nx as f64)
            .round()
            .clamp(1.0, (grid.nx - 1) as f64) as usize;
        let contact_iy = (params.strike_y * grid.ny as f64)
            .round()
            .clamp(1.0, (grid.ny - 1) as f64) as usize;
        // (0.8, 0.2): off the even-(p,q)-silencing x=Lx/2, y=Ly/2 nodal lines.
        let pickup_ix = (0.8 * grid.nx as f64)
            .round()
            .clamp(1.0, (grid.nx - 1) as f64) as usize;
        let pickup_iy = (0.2 * grid.ny as f64)
            .round()
            .clamp(1.0, (grid.ny - 1) as f64) as usize;
        let contact_patch = build_contact_patch(grid.nx, grid.ny, contact_ix, contact_iy, grid.h);
        let dt = 1.0 / sr;
        RhaoutiChaigneJolySite {
            grid,
            hammer: Hammer::new(
                params.hammer_mass,
                params.hammer_k,
                params.hammer_p,
                params.vel,
                dt,
            ),
            detached: false,
            contact_patch,
            pickup_ix,
            pickup_iy,
            dt,
        }
    }
}

impl RhaoutiChaigneJolySite {
    fn advance(&mut self) -> f64 {
        let grid = &mut self.grid;
        let (nx, ny) = (grid.nx, grid.ny);
        let u_h = if self.detached {
            0.0
        } else {
            self.contact_patch
                .nodes
                .iter()
                .zip(&self.contact_patch.weights)
                .map(|(&n, &w)| w * grid.u_now[n])
                .sum::<f64>()
        };

        let mut forces = [0.0f64; 1];
        self.hammer.substeps(
            self.dt,
            &[u_h],
            std::slice::from_mut(&mut self.detached),
            &mut forces,
        );
        let force = forces[0];

        for iy in 1..ny {
            for ix in 1..nx {
                let c = node_index(nx, ix, iy);
                let lap_now = grid.u_now[node_index(nx, ix + 1, iy)]
                    + grid.u_now[node_index(nx, ix - 1, iy)]
                    + grid.u_now[node_index(nx, ix, iy + 1)]
                    + grid.u_now[node_index(nx, ix, iy - 1)]
                    - 4.0 * grid.u_now[c];
                let lap_prev = grid.u_prev[node_index(nx, ix + 1, iy)]
                    + grid.u_prev[node_index(nx, ix - 1, iy)]
                    + grid.u_prev[node_index(nx, ix, iy + 1)]
                    + grid.u_prev[node_index(nx, ix, iy - 1)]
                    - 4.0 * grid.u_prev[c];
                grid.u_next[c] = 2.0 * grid.u_now[c] - grid.u_prev[c] + grid.courant_sq * lap_now
                    - grid.damp_a * (grid.u_now[c] - grid.u_prev[c])
                    + grid.damp_b * (lap_now - lap_prev);
            }
        }
        if force != 0.0 {
            let injection = (self.dt * self.dt / (grid.sigma * grid.h * grid.h)) * force;
            for (&node, &w) in self
                .contact_patch
                .nodes
                .iter()
                .zip(&self.contact_patch.weights)
            {
                grid.u_next[node] += injection * w;
            }
        }
        for ix in 0..=nx {
            grid.u_next[node_index(nx, ix, 0)] = 0.0;
            grid.u_next[node_index(nx, ix, ny)] = 0.0;
        }
        for iy in 0..=ny {
            grid.u_next[node_index(nx, 0, iy)] = 0.0;
            grid.u_next[node_index(nx, nx, iy)] = 0.0;
        }

        let sample = grid.u_now[node_index(nx, self.pickup_ix, self.pickup_iy)];

        std::mem::swap(&mut grid.u_prev, &mut grid.u_now);
        std::mem::swap(&mut grid.u_now, &mut grid.u_next);

        sample
    }
}

impl Solver for RhaoutiChaigneJolySite {
    fn step(&mut self) -> Result<f64, SampleError> {
        Ok(self.advance())
    }
}
