// Concern: one rigid-box room-acoustics call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Botteldooren, JASA 95(5), 2313-2319 (1994), doi 10.1121/1.409866 -- paper unobtainable, so
//! this is a first-principles rigid-Cartesian re-derivation, sized for a test enclosure.

use crate::error::SampleError;
use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;

const SOUND_SPEED_M_S: f64 = 343.0;
const AIR_DENSITY_KG_M3: f64 = 1.225;

/// `(nx+1)*(ny+1)*(nz+1)`: above a test enclosure, below anything slow.
pub const NODE_COUNT_CEILING: usize = 250_000;

#[derive(Clone, Debug, PartialEq)]
pub struct BotteldoorenParams {
    pub f0: f64,
    pub aspect_y: f64,
    pub aspect_z: f64,
    pub listener_x: f64,
    pub listener_y: f64,
    pub listener_z: f64,
    pub pulse_amp: f64,
    pub pulse_width: f64,
    pub damp_dc: f64,
    pub damp_freq: f64,
}

impl BotteldoorenParams {
    /// A test enclosure whose lowest axial mode is the frequency asked for.
    pub fn at(f0: f64) -> BotteldoorenParams {
        BotteldoorenParams {
            f0,
            aspect_y: 0.75,
            aspect_z: 0.6,
            listener_x: 0.7,
            listener_y: 0.3,
            listener_z: 0.6,
            pulse_amp: 1.0,
            pulse_width: 0.001,
            damp_dc: 0.5,
            damp_freq: 1.0e-4,
        }
    }
}

impl BotteldoorenParams {
    pub fn valid(&self) -> bool {
        all(&[
            (self.f0, Positive),
            (self.aspect_y, Positive),
            (self.aspect_z, Positive),
            (self.listener_x, OpenUnit),
            (self.listener_y, OpenUnit),
            (self.listener_z, OpenUnit),
            (self.pulse_amp, Positive),
            (self.pulse_width, Positive),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
        ])
    }
}

#[inline]
fn node_index(nx: usize, ny: usize, ix: usize, iy: usize, iz: usize) -> usize {
    (iz * (ny + 1) + iy) * (nx + 1) + ix
}

/// Neumann ghost mirror: `p_{-1}=p_{1}`.
#[inline]
fn mirror(i: isize, n: usize) -> usize {
    if i < 0 {
        1
    } else if i as usize > n {
        n - 1
    } else {
        i as usize
    }
}

/// Unscaled by `h^2`: three ghost-mirrored 1D second differences.
#[inline]
fn laplacian(u: &[f64], nx: usize, ny: usize, nz: usize, ix: usize, iy: usize, iz: usize) -> f64 {
    let c = node_index(nx, ny, ix, iy, iz);
    let xm = node_index(nx, ny, mirror(ix as isize - 1, nx), iy, iz);
    let xp = node_index(nx, ny, mirror(ix as isize + 1, nx), iy, iz);
    let ym = node_index(nx, ny, ix, mirror(iy as isize - 1, ny), iz);
    let yp = node_index(nx, ny, ix, mirror(iy as isize + 1, ny), iz);
    let zm = node_index(nx, ny, ix, iy, mirror(iz as isize - 1, nz));
    let zp = node_index(nx, ny, ix, iy, mirror(iz as isize + 1, nz));
    (u[xp] - 2.0 * u[c] + u[xm]) + (u[yp] - 2.0 * u[c] + u[ym]) + (u[zp] - 2.0 * u[c] + u[zm])
}

/// Node count is `~1/f0^3`: `f64` saturates where `usize` panics.
fn room_sizing_f64(f0: f64, aspect_y: f64, aspect_z: f64, sr: f64) -> (f64, f64, f64, f64) {
    let (nx_u, ny_u, nz_u, h) = axis_unrounded_nodes(f0, aspect_y, aspect_z, sr);
    (
        nx_u.round().max(4.0),
        ny_u.round().max(4.0),
        nz_u.round().max(4.0),
        h,
    )
}

fn axis_unrounded_nodes(f0: f64, aspect_y: f64, aspect_z: f64, sr: f64) -> (f64, f64, f64, f64) {
    let dt = 1.0 / sr;
    // 7-point stencil's von Neumann bound: the 1D<=1 / 2D<=1/sqrt(2) bounds' 3-axis generalization.
    let lambda_max = 1.0 / 3.0f64.sqrt();
    let lambda_target = 0.9 * lambda_max;
    let h = SOUND_SPEED_M_S * dt / lambda_target;
    // f0 = c/(2Lx): same fundamental-axial-mode sizing as every string/bar primitive here.
    let lx = SOUND_SPEED_M_S / (2.0 * f0);
    let ly = lx * aspect_y;
    let lz = lx * aspect_z;
    (lx / h, ly / h, lz / h, h)
}

pub fn node_count(f0: f64, aspect_y: f64, aspect_z: f64, sr: f64) -> f64 {
    let (nx, ny, nz, _) = room_sizing_f64(f0, aspect_y, aspect_z, sr);
    (nx + 1.0) * (ny + 1.0) * (nz + 1.0)
}

/// Above 4.5, where an axis pins to the `.max(4.0)` floor.
pub const MIN_AXIS_UNROUNDED_NODES: f64 = 6.0;

pub fn max_axis_unrounded_nodes(f0: f64, aspect_y: f64, aspect_z: f64, sr: f64) -> f64 {
    let (nx_u, ny_u, nz_u, _) = axis_unrounded_nodes(f0, aspect_y, aspect_z, sr);
    nx_u.max(ny_u).max(nz_u)
}

struct RoomGrid {
    p_now: Vec<f64>,
    p_prev: Vec<f64>,
    p_next: Vec<f64>,
    nx: usize,
    ny: usize,
    nz: usize,
    h: f64,
    rho0: f64,
    /// `lambda^2`, 90% under the `1/sqrt(3)` bound.
    courant_sq: f64,
    damp_a: f64,
    damp_b: f64,
}

/// Safe only under a caller's [`node_count`] check.
fn build_grid(params: &BotteldoorenParams, sr: f64) -> RoomGrid {
    let dt = 1.0 / sr;
    let (nx, ny, nz, h) = room_sizing_f64(params.f0, params.aspect_y, params.aspect_z, sr);
    let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
    let count = (nx + 1) * (ny + 1) * (nz + 1);
    let lambda_target = 0.9 / 3.0f64.sqrt();
    RoomGrid {
        p_now: vec![0.0; count],
        p_prev: vec![0.0; count],
        p_next: vec![0.0; count],
        nx,
        ny,
        nz,
        h,
        rho0: AIR_DENSITY_KG_M3,
        courant_sq: lambda_target * lambda_target,
        damp_a: 2.0 * params.damp_dc * dt,
        damp_b: 2.0 * params.damp_freq * dt / (h * h),
    }
}

/// Fixed: off the low-order nodal planes.
const SOURCE_X: f64 = 0.21;
const SOURCE_Y: f64 = 0.57;
const SOURCE_Z: f64 = 0.34;

/// Radius in grid CELLS: `h` moves with `sr` alone, so this stays sane as `f0` rises.
const SOURCE_WINDOW_RADIUS_CELLS: f64 = 2.0;
const SOURCE_WINDOW_WEIGHT_FLOOR: f64 = 1e-3;

struct SourcePatch {
    nodes: Vec<usize>,
    weights: Vec<f64>,
}

fn build_source_patch(
    nx: usize,
    ny: usize,
    nz: usize,
    cx: usize,
    cy: usize,
    cz: usize,
) -> SourcePatch {
    let coeff = SOURCE_WINDOW_WEIGHT_FLOOR.ln().abs() / SOURCE_WINDOW_RADIUS_CELLS.powi(4);
    let radius = SOURCE_WINDOW_RADIUS_CELLS.ceil() as isize;
    let mut nodes = Vec::new();
    let mut weights = Vec::new();
    let mut total = 0.0;
    for dk in -radius..=radius {
        for dj in -radius..=radius {
            for di in -radius..=radius {
                let (ix, iy, iz) = (cx as isize + di, cy as isize + dj, cz as isize + dk);
                if ix < 0
                    || ix > nx as isize
                    || iy < 0
                    || iy > ny as isize
                    || iz < 0
                    || iz > nz as isize
                {
                    continue;
                }
                let quartic = |d: isize| (d * d * d * d) as f64;
                let w = (-coeff * (quartic(di) + quartic(dj) + quartic(dk))).exp();
                if w < SOURCE_WINDOW_WEIGHT_FLOOR {
                    continue;
                }
                nodes.push(node_index(nx, ny, ix as usize, iy as usize, iz as usize));
                weights.push(w);
                total += w;
            }
        }
    }
    for w in &mut weights {
        *w /= total;
    }
    SourcePatch { nodes, weights }
}

pub struct BotteldoorenSite {
    grid: RoomGrid,
    source_patch: SourcePatch,
    listener_index: usize,
    pulse_amp: f64,
    pulse_width: f64,
    dt: f64,
    sample_index: u64,
}

fn axis_index(ratio: f64, n: usize) -> usize {
    (ratio * n as f64).round().clamp(0.0, n as f64) as usize
}

impl BotteldoorenSite {
    pub fn new(params: &BotteldoorenParams, sr: f64) -> BotteldoorenSite {
        let grid = build_grid(params, sr);
        let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
        let source_patch = build_source_patch(
            nx,
            ny,
            nz,
            axis_index(SOURCE_X, nx),
            axis_index(SOURCE_Y, ny),
            axis_index(SOURCE_Z, nz),
        );
        let listener_index = node_index(
            nx,
            ny,
            axis_index(params.listener_x, nx),
            axis_index(params.listener_y, ny),
            axis_index(params.listener_z, nz),
        );
        BotteldoorenSite {
            grid,
            source_patch,
            listener_index,
            pulse_amp: params.pulse_amp,
            pulse_width: params.pulse_width,
            dt: 1.0 / sr,
            sample_index: 0,
        }
    }
}

impl BotteldoorenSite {
    /// Rigid walls fall out of [`laplacian`]'s own mirroring.
    fn advance(&mut self) -> f64 {
        let t = self.sample_index as f64 * self.dt;
        let source = crate::physics::raised_cosine_pulse(t, self.pulse_amp, self.pulse_width);

        let grid = &mut self.grid;
        let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);

        for iz in 0..=nz {
            for iy in 0..=ny {
                for ix in 0..=nx {
                    let c = node_index(nx, ny, ix, iy, iz);
                    let lap_now = laplacian(&grid.p_now, nx, ny, nz, ix, iy, iz);
                    let lap_prev = laplacian(&grid.p_prev, nx, ny, nz, ix, iy, iz);
                    grid.p_next[c] = 2.0 * grid.p_now[c] - grid.p_prev[c]
                        + grid.courant_sq * lap_now
                        - grid.damp_a * (grid.p_now[c] - grid.p_prev[c])
                        + grid.damp_b * (lap_now - lap_prev);
                }
            }
        }

        if source != 0.0 {
            // 3D analog of the membrane's dt^2/(sigma*h^2): h^3 this grid's per-node volume.
            let injection = (self.dt * self.dt / (grid.rho0 * grid.h.powi(3))) * source;
            for (&node, &w) in self
                .source_patch
                .nodes
                .iter()
                .zip(&self.source_patch.weights)
            {
                grid.p_next[node] += injection * w;
            }
        }

        let sample = grid.p_now[self.listener_index];
        self.sample_index += 1;

        std::mem::swap(&mut grid.p_prev, &mut grid.p_now);
        std::mem::swap(&mut grid.p_now, &mut grid.p_next);

        sample
    }
}

impl Solver for BotteldoorenSite {
    fn step(&mut self) -> Result<f64, SampleError> {
        Ok(self.advance())
    }
}
