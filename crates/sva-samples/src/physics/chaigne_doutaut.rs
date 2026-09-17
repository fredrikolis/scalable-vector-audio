// Concern: one hammer/free-free-bar call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! Chaigne & Doutaut, JASA 101(1) 539-557 (1997): `u_tt = -kappa^2 u_xxxx`, free-free BCs,
//! uniform cross-section, driven by `hammer.rs`.

use crate::physics::Solver;

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;
use crate::physics::hammer::Hammer;

#[derive(Clone, Debug, PartialEq)]
pub struct ChaigneDoutautParams {
    pub f0: f64,
    pub strike_pos: f64,
    pub vel: f64,
    pub hammer_mass: f64,
    pub hammer_k: f64,
    pub hammer_p: f64,
    pub damp_dc: f64,
    pub damp_freq: f64,
}

impl ChaigneDoutautParams {
    /// `chaigne_askenfelt`'s hammer defaults, at the fundamental asked for.
    pub fn at(f0: f64) -> ChaigneDoutautParams {
        ChaigneDoutautParams {
            f0,
            strike_pos: 0.15,
            vel: 3.2,
            hammer_mass: 2.9e-3,
            hammer_k: 2.6646e8,
            hammer_p: 2.5,
            damp_dc: 0.6,
            damp_freq: 1.6e-4,
        }
    }
}

impl ChaigneDoutautParams {
    pub fn valid(&self) -> bool {
        all(&[
            (self.f0, Positive),
            (self.strike_pos, OpenUnit),
            (self.vel, Positive),
            (self.hammer_mass, Positive),
            (self.hammer_k, Positive),
            (self.hammer_p, Positive),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
        ])
    }
}

/// Aluminum, tuned to land a C4 bar near ~45cm.
const BAR_YOUNGS_MODULUS_PA: f64 = 7.0e10;
const BAR_DENSITY_KG_M3: f64 = 2700.0;
const BAR_THICKNESS_M: f64 = 0.01;
/// Cancels out of `kappa`; sets the contact-injection mass scale.
const BAR_WIDTH_M: f64 = 1.0;
/// Free-free root of `cos(beta L)cosh(beta L) = 1`.
const BETA1_L: f64 = 4.730040744862704;
/// A free-bar anti-node for nearly every low partial, off the strike.
const PICKUP_POS: f64 = 0.93;

pub(crate) struct BarGrid {
    pub(crate) u_now: Vec<f64>,
    pub(crate) u_prev: Vec<f64>,
    pub(crate) u_next: Vec<f64>,
    pub(crate) n: usize,
    pub(crate) dx: f64,
    pub(crate) rho: f64,
    #[allow(dead_code)]
    pub(crate) kappa: f64,
    pub(crate) stiff_sq: f64,
    pub(crate) damp_a: f64,
    pub(crate) damp_b: f64,
    pub(crate) bar_substeps: usize,
}

/// Free-end ghosts: `u_xx=0` gives `u_{-1}=2u_0-u_1`, then `u_xxx=0` gives
/// `u_{-2}=4u_0-4u_1+u_2`.
pub(crate) fn free_ghost(u: &[f64], n: usize, idx: isize) -> f64 {
    if idx >= 0 && idx as usize <= n {
        return u[idx as usize];
    }
    if idx < 0 {
        match idx {
            -1 => 2.0 * u[0] - u[1],
            -2 => 4.0 * u[0] - 4.0 * u[1] + u[2],
            _ => unreachable!(),
        }
    } else {
        match idx as usize - n {
            1 => 2.0 * u[n] - u[n - 1],
            2 => 4.0 * u[n] - 4.0 * u[n - 1] + u[n - 2],
            _ => unreachable!(),
        }
    }
}

/// von Neumann: `mu = kappa*dt/dx^2 <= 0.5`, at a 90% margin.
pub(crate) fn bar_grid(
    rho: f64,
    kappa: f64,
    length: f64,
    damp_dc: f64,
    damp_freq: f64,
    sr: f64,
) -> BarGrid {
    let dt = 1.0 / sr;
    let mu_max = 0.5;
    let mu_target = 0.9 * mu_max;
    let dx_target = (kappa * dt / mu_target).sqrt();
    let n = ((length / dx_target).round() as usize).max(4);
    let dx = length / n as f64;
    // The n>=4 floor can outrun `dx_target` for a short (high-`f0`) bar; sub-step then.
    let mu_at_dt = kappa * dt / (dx * dx);
    let bar_substeps = (mu_at_dt / mu_target).ceil().max(1.0) as usize;
    let dt_sub = dt / bar_substeps as f64;
    let stiff_sq = kappa * kappa * dt_sub * dt_sub / dx.powi(4);

    BarGrid {
        u_now: vec![0.0; n + 1],
        u_prev: vec![0.0; n + 1],
        u_next: vec![0.0; n + 1],
        n,
        dx,
        rho,
        kappa,
        stiff_sq,
        damp_a: 2.0 * damp_dc * dt_sub,
        damp_b: 2.0 * damp_freq * dt_sub / (dx * dx),
        bar_substeps,
    }
}

/// `f0 = kappa*(beta1 L)^2/(2 pi L^2)`, solved for `L`.
fn build_grid(params: &ChaigneDoutautParams, sr: f64) -> BarGrid {
    let kappa =
        (BAR_YOUNGS_MODULUS_PA / BAR_DENSITY_KG_M3).sqrt() * BAR_THICKNESS_M / 12.0f64.sqrt();
    let length = (kappa * BETA1_L * BETA1_L / (2.0 * std::f64::consts::PI * params.f0)).sqrt();
    let rho = BAR_DENSITY_KG_M3 * BAR_WIDTH_M * BAR_THICKNESS_M;
    bar_grid(rho, kappa, length, params.damp_dc, params.damp_freq, sr)
}

pub struct ChaigneDoutautSite {
    bar: BarGrid,
    hammer: Hammer,
    detached: bool,
    contact_index: usize,
    pickup_index: usize,
    dt: f64,
}

impl ChaigneDoutautSite {
    pub fn new(params: &ChaigneDoutautParams, sr: f64) -> ChaigneDoutautSite {
        let bar = build_grid(params, sr);
        let contact_index = (params.strike_pos * bar.n as f64)
            .round()
            .clamp(1.0, (bar.n - 1) as f64) as usize;
        let pickup_index = (PICKUP_POS * bar.n as f64)
            .round()
            .clamp(1.0, (bar.n - 1) as f64) as usize;
        let dt = 1.0 / sr;
        ChaigneDoutautSite {
            bar,
            hammer: Hammer::new(
                params.hammer_mass,
                params.hammer_k,
                params.hammer_p,
                params.vel,
                dt,
            ),
            detached: false,
            contact_index,
            pickup_index,
            dt,
        }
    }
}

impl Solver for ChaigneDoutautSite {
    fn step(&mut self) -> f64 {
        let bar = &mut self.bar;
        let n = bar.n;
        let u_h = bar.u_now[self.contact_index];

        let mut forces = [0.0f64; 1];
        self.hammer.substeps(
            self.dt,
            &[u_h],
            std::slice::from_mut(&mut self.detached),
            &mut forces,
        );
        let force = forces[0];

        let sample = bar.u_now[self.pickup_index];

        let dt_sub = self.dt / bar.bar_substeps as f64;
        let injection = (dt_sub * dt_sub / (bar.rho * bar.dx)) * force;
        for _ in 0..bar.bar_substeps {
            for i in 0..=n {
                let ii = i as isize;
                let lap_now = free_ghost(&bar.u_now, n, ii + 1) - 2.0 * bar.u_now[i]
                    + free_ghost(&bar.u_now, n, ii - 1);
                let biharm = free_ghost(&bar.u_now, n, ii + 2)
                    - 4.0 * free_ghost(&bar.u_now, n, ii + 1)
                    + 6.0 * bar.u_now[i]
                    - 4.0 * free_ghost(&bar.u_now, n, ii - 1)
                    + free_ghost(&bar.u_now, n, ii - 2);
                let lap_prev = free_ghost(&bar.u_prev, n, ii + 1) - 2.0 * bar.u_prev[i]
                    + free_ghost(&bar.u_prev, n, ii - 1);
                let mut next = 2.0 * bar.u_now[i]
                    - bar.u_prev[i]
                    - bar.stiff_sq * biharm
                    - bar.damp_a * (bar.u_now[i] - bar.u_prev[i])
                    + bar.damp_b * (lap_now - lap_prev);
                if i == self.contact_index {
                    next += injection;
                }
                bar.u_next[i] = next;
            }
            std::mem::swap(&mut bar.u_prev, &mut bar.u_now);
            std::mem::swap(&mut bar.u_now, &mut bar.u_next);
        }

        sample
    }
}
