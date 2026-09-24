// Concern: one bowed-string call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Elasto-plastic friction: Dupont/Hayward/Armstrong/Altpeter, IEEE TAC 47(5) (2002). First
//! bowed use: Serafin/Avanzini/Rocchesso, SMAC-03. Corrected eqs. (7)-(9)/FD coupling:
//! Willemsen/Bilbao/Serafin, DAFx-19 pp. 40-46. Reuses `chaigne_askenfelt`'s FD string.

use crate::error::SampleError;
use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;
use crate::physics::chaigne_askenfelt::{
    StringGrid, Wire, dispersive_grid, grid_tension, point_weights, read_at, spread, stencil_update,
};

#[derive(Clone, Debug, PartialEq)]
pub struct WillemsenBilbaoSerafinParams {
    pub f0: f64,
    pub b: f64,
    pub bow_pos: f64,
    pub bow_vel: f64,
    pub bow_force: f64,
    pub mu_s: f64,
    pub mu_c: f64,
    pub stribeck_vel: f64,
    pub bristle_stiffness: f64,
    pub bristle_damping: f64,
    pub viscous_friction: f64,
    pub damp_dc: f64,
    pub damp_freq: f64,
}

impl WillemsenBilbaoSerafinParams {
    /// DAFx-19 Table 1's reference values, at the fundamental asked for.
    pub fn at(f0: f64) -> WillemsenBilbaoSerafinParams {
        WillemsenBilbaoSerafinParams {
            f0,
            b: 1e-4,
            bow_pos: 0.25,
            bow_vel: 0.1,
            bow_force: 10.0,
            mu_s: 0.8,
            mu_c: 0.3,
            stribeck_vel: 0.1,
            bristle_stiffness: 1e4,
            bristle_damping: 0.1,
            viscous_friction: 0.4,
            damp_dc: 1.0,
            damp_freq: 5e-3,
        }
    }
}

impl WillemsenBilbaoSerafinParams {
    pub fn valid(&self) -> bool {
        all(&[
            (self.f0, Positive),
            (self.b, NonNegative),
            (self.bow_pos, OpenUnit),
            (self.bow_vel, Finite),
            (self.bow_force, Positive),
            (self.mu_s, Positive),
            (self.mu_c, Positive),
            (self.stribeck_vel, Positive),
            (self.bristle_stiffness, Positive),
            (self.bristle_damping, NonNegative),
            (self.viscous_friction, NonNegative),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
        ])
    }
}

const WIRE_DENSITY_KG_M3: f64 = 7850.0;
const WIRE_RADIUS_M: f64 = 5e-4;
/// Fixed per Table 1; `c = 2 f0 L` scales instead (`chaigne_askenfelt` inverts that).
const WIRE_LENGTH_M: f64 = 1.0;
const NR_MAX_ITERATIONS: usize = 50;
const NR_TOLERANCE: f64 = 1e-7;

/// Table 1's wire at `c = 2 f0 L`.
fn build_grid(params: &WillemsenBilbaoSerafinParams, sr: f64) -> Option<StringGrid> {
    let rho = std::f64::consts::PI * WIRE_RADIUS_M * WIRE_RADIUS_M * WIRE_DENSITY_KG_M3;
    let wire = Wire {
        rho,
        c: 2.0 * params.f0 * WIRE_LENGTH_M,
        length: WIRE_LENGTH_M,
    };
    dispersive_grid(
        wire,
        params.f0,
        params.b,
        params.damp_dc,
        params.damp_freq,
        sr,
    )
}

fn sgn(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Eq. (7): `abs()` around `z_ss`, missing in the pre-DAFx-19 literature.
fn steady_state(v: f64, s0: f64, f_c: f64, f_s: f64, stribeck_vel: f64) -> f64 {
    sgn(v) / s0 * (f_c + (f_s - f_c) * (-(v / stribeck_vel).powi(2)).exp())
}

/// Eq. (8)-(9): `sgn(z)` on the sine offset; `|z|=z_ba`/`|z_ss|` resolved, not undefined.
fn adhesion(v: f64, z: f64, z_ba: f64, zss: f64) -> f64 {
    if sgn(v) != sgn(z) {
        return 0.0;
    }
    let (az, azss) = (z.abs(), zss.abs());
    if az <= z_ba {
        0.0
    } else if az >= azss {
        1.0
    } else {
        let span = (azss - z_ba).max(1e-300);
        let s = sgn(z);
        0.5 * (1.0 + s * (std::f64::consts::PI * (z - s * 0.5 * (azss + z_ba)) / span).sin())
    }
}

/// Eq. (6)/(15). `v == 0` avoids dividing by `z_ss(0) = 0`.
fn bristle_rate(v: f64, z: f64, s0: f64, f_c: f64, f_s: f64, stribeck_vel: f64, z_ba: f64) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let zss = steady_state(v, s0, f_c, f_s, stribeck_vel);
    let a = adhesion(v, z, z_ba, zss);
    v * (1.0 - a * z / zss)
}

/// Eq. (4), noise term `s3*w` dropped (disclosed v1 gap).
fn friction_force(v: f64, z: f64, r: f64, s0: f64, s1: f64, s2: f64) -> f64 {
    s0 * z + s1 * r + s2 * v
}

struct Coupling {
    coeff: f64,
    b_known: f64,
    s0: f64,
    s1: f64,
    s2: f64,
    f_c: f64,
    f_s: f64,
    stribeck_vel: f64,
    z_ba: f64,
    z_prev: f64,
    r_prev: f64,
    dt: f64,
}

impl Coupling {
    /// `g1` re-derives eq. (17)-(19): theirs assume centred damping, the FD string's is
    /// backward. `g2` keeps their eq. (20)-(21).
    fn residual(&self, v: f64, z: f64) -> (f64, f64) {
        let r = bristle_rate(
            v,
            z,
            self.s0,
            self.f_c,
            self.f_s,
            self.stribeck_vel,
            self.z_ba,
        );
        let f = friction_force(v, z, r, self.s0, self.s1, self.s2);
        let g1 = v + self.coeff * f - self.b_known;
        let a = 2.0 * (z - self.z_prev) / self.dt - self.r_prev;
        (g1, r - a)
    }

    /// A central-difference Jacobian, not Algorithm 1's analytic one.
    fn newton_step(&self, v: f64, z: f64) -> Option<(f64, f64)> {
        let (g1, g2) = self.residual(v, z);
        let hv = v.abs().max(1e-3) * 1e-6;
        let hz = z.abs().max(self.z_ba).max(1e-9) * 1e-6;
        let (g1_vp, g2_vp) = self.residual(v + hv, z);
        let (g1_vm, g2_vm) = self.residual(v - hv, z);
        let (g1_zp, g2_zp) = self.residual(v, z + hz);
        let (g1_zm, g2_zm) = self.residual(v, z - hz);
        let dg1_dv = (g1_vp - g1_vm) / (2.0 * hv);
        let dg2_dv = (g2_vp - g2_vm) / (2.0 * hv);
        let dg1_dz = (g1_zp - g1_zm) / (2.0 * hz);
        let dg2_dz = (g2_zp - g2_zm) / (2.0 * hz);
        let det = dg1_dv * dg2_dz - dg1_dz * dg2_dv;
        if !det.is_finite() || det.abs() < 1e-300 {
            return None;
        }
        let dv = (g1 * dg2_dz - g2 * dg1_dz) / det;
        let dz = (dg1_dv * g2 - dg2_dv * g1) / det;
        let (new_v, new_z) = (v - dv, z - dz);
        if !new_v.is_finite() || !new_z.is_finite() {
            return None;
        }
        Some((new_v, new_z))
    }
}

pub struct WillemsenBilbaoSerafinSite {
    strings: Vec<StringGrid>,
    /// Read and spread alike.
    bow: Vec<f64>,
    bow_self: f64,
    tension: f64,
    dt: f64,
    bow_vel: f64,
    f_c: f64,
    f_s: f64,
    stribeck_vel: f64,
    s0: f64,
    s1: f64,
    s2: f64,
    z_ba: f64,
    z: f64,
    z_prev: f64,
    /// `r^{n-1}`, eq. (20)-(21)'s extra trapezoidal state.
    r_prev: f64,
    last_v: f64,
    last_f: f64,
    /// Samples stepped, which a refusal names.
    steps: usize,
}

impl WillemsenBilbaoSerafinSite {
    pub fn new(params: &WillemsenBilbaoSerafinParams, sr: f64) -> Result<Self, SampleError> {
        let grid = build_grid(params, sr).ok_or(SampleError::StringPastRate {
            model: "willemsen_bilbao_serafin",
        })?;
        let bow = point_weights(grid.n, params.bow_pos);
        let bow_self = read_at(&bow, &bow);
        let tension = grid_tension(&grid, 1.0 / sr);
        let f_c = params.mu_c * params.bow_force;
        let f_s = params.mu_s * params.bow_force;
        // Table 1: z_ba = 0.7*f_C/s0 — off the kinetic (mu_c) force, not the static one.
        let z_ba = 0.7 * f_c / params.bristle_stiffness;
        Ok(WillemsenBilbaoSerafinSite {
            strings: vec![grid],
            bow,
            bow_self,
            tension,
            dt: 1.0 / sr,
            bow_vel: params.bow_vel,
            f_c,
            f_s,
            stribeck_vel: params.stribeck_vel,
            s0: params.bristle_stiffness,
            s1: params.bristle_damping,
            s2: params.viscous_friction,
            z_ba,
            z: 0.0,
            z_prev: 0.0,
            r_prev: 0.0,
            last_v: 0.0,
            last_f: 0.0,
            steps: 0,
        })
    }
}

impl Solver for WillemsenBilbaoSerafinSite {
    fn step(&mut self) -> Result<f64, SampleError> {
        let dt = self.dt;
        let coupling_prev = (self.z, self.r_prev);

        let grid = &mut self.strings[0];
        let n = grid.n;
        for i in 1..n {
            grid.y_next[i] = stencil_update(grid, i, 0.0, 0.0);
        }
        grid.y_next[0] = 0.0;
        grid.y_next[n] = 0.0;
        let free_next = read_at(&self.bow, &grid.y_next);
        let bow_prev = read_at(&self.bow, &grid.y_prev);

        let coupling = Coupling {
            coeff: dt * self.bow_self / (2.0 * grid.rho * grid.dx),
            b_known: (free_next - bow_prev) / (2.0 * dt) - self.bow_vel,
            s0: self.s0,
            s1: self.s1,
            s2: self.s2,
            f_c: self.f_c,
            f_s: self.f_s,
            stribeck_vel: self.stribeck_vel,
            z_ba: self.z_ba,
            z_prev: coupling_prev.0,
            r_prev: coupling_prev.1,
            dt,
        };

        let (mut v, mut z) = (self.last_v, self.z);
        let mut converged = false;
        for _ in 0..NR_MAX_ITERATIONS {
            let Some((new_v, new_z)) = coupling.newton_step(v, z) else {
                break;
            };
            let step_norm = ((new_v - v).powi(2) + (new_z - z).powi(2)).sqrt();
            v = new_v;
            z = new_z;
            if step_norm < NR_TOLERANCE {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err(SampleError::ContactUnsettled {
                model: "willemsen_bilbao_serafin",
                sample: self.steps,
            });
        }
        self.steps += 1;

        let r = bristle_rate(
            v,
            z,
            self.s0,
            self.f_c,
            self.f_s,
            self.stribeck_vel,
            self.z_ba,
        );
        let f = friction_force(v, z, r, self.s0, self.s1, self.s2);
        let grid = &mut self.strings[0];
        spread(grid, &self.bow, -f, dt);

        self.z_prev = self.z;
        self.z = z;
        self.r_prev = r;
        self.last_v = v;
        self.last_f = f;

        let sample = self.tension * (grid.y_now[n] - grid.y_now[n - 1]) / grid.dx;

        std::mem::swap(&mut grid.y_prev, &mut grid.y_now);
        std::mem::swap(&mut grid.y_now, &mut grid.y_next);

        Ok(sample)
    }
}
