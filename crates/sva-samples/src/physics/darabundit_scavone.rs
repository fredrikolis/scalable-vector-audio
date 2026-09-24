// Concern: one acoustic-bore call site's finite-difference state | Non-concern: argument evaluation, builtin dispatch | IO: (params, sr) -> a site; () -> a sample

//! A 1D acoustic bore with an optional fixed tonehole lattice — Darabundit & Scavone 2025
//! ("D&S", §7.1-7.2). Propagation: Bilbao & Harrison 2016, Bilbao & Chick 2013 §II.D.

use crate::error::SampleError;
use crate::physics::Solver;
use crate::physics::tonehole::{RadCoeffs, ToneholeBranch, build_tonehole, radiation_coeffs};

use crate::physics::bound::Bound::*;
use crate::physics::bound::all;

pub(super) const RHO0_KG_M3: f64 = 1.1769;
pub(super) const C0_M_S: f64 = 347.23;
const ETA0_PA_S: f64 = 1.846e-5;
const SQRT_PRANDTL: f64 = 0.8410;
const GAMMA0: f64 = 1.4017;
const MIN_SEGMENTS: usize = 1;
pub const MAX_HOLES: usize = 6;

/// D&S's `kb < 0.5` bound (eq. 120a-120b, §7.2).
pub fn nyquist_wavenumber(sr: f64) -> f64 {
    std::f64::consts::PI * sr / C0_M_S
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToneholeSpec {
    pub pos: f64,
    pub open: bool,
    pub radius: f64,
    /// `t_h`, eq. 118/120b.
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoreParams {
    pub length: f64,
    pub radius_in: f64,
    pub radius_out: f64,
    pub excite_pos: f64,
    pub pulse_amp: f64,
    pub pulse_width: f64,
    /// Scales the whole viscous branch family; `damp_freq` scales the whole thermal one.
    pub damp_dc: f64,
    pub damp_freq: f64,
    pub holes: [Option<ToneholeSpec>; MAX_HOLES],
}

impl BoreParams {
    /// A plain cylinder of the length asked for, every tonehole slot empty.
    pub fn at(length: f64) -> BoreParams {
        BoreParams {
            length,
            radius_in: 0.01,
            radius_out: 0.01,
            excite_pos: 0.0,
            pulse_amp: 1.0,
            pulse_width: 0.001,
            damp_dc: 1.0,
            damp_freq: 1.0,
            holes: [None; MAX_HOLES],
        }
    }
}

impl BoreParams {
    pub fn valid(&self) -> bool {
        all(&[
            (self.length, Positive),
            (self.radius_in, Positive),
            (self.radius_out, Positive),
            (self.excite_pos, HalfOpenUnit),
            (self.pulse_amp, Positive),
            (self.pulse_width, Positive),
            (self.damp_dc, NonNegative),
            (self.damp_freq, NonNegative),
        ]) && self.holes.iter().flatten().all(|h| self.hole_valid(h))
    }

    fn hole_valid(&self, hole: &ToneholeSpec) -> bool {
        let local_radius = self.radius_in + (self.radius_out - self.radius_in) * hole.pos;
        all(&[
            (hole.pos, HalfOpenUnit),
            (hole.radius, Positive),
            (hole.height, NonNegative),
        ]) && hole.radius < local_radius
    }
}

/// Backward-Euler, not the source's trapezoid: passive at any step size.
const LOSS_BRANCHES: usize = 6;

/// Fitted against the discrete Backward-Euler operator, not the s-domain: the near-Nyquist
/// reactance error left over is ≤2% of the `jωρ0`-dominated series term.
const GHAT: [f64; LOSS_BRANCHES] = [
    3.030565307333e-02,
    3.623164588771e-02,
    7.517234787811e-02,
    1.661722612526e-01,
    4.503547222868e-01,
    8.057882518470e+01,
];
const AHAT: [f64; LOSS_BRANCHES] = [
    7.367064671349e-04,
    5.320144731723e-03,
    2.447848326023e-02,
    1.144572621260e-01,
    6.460771511330e-01,
    3.000000000000e+02,
];

/// `Z = jωρ0 + R0 + Σ_q (R_q ‖ jωL_q)`: Harrison 2018 Table 2.5, eqs. 2.132–2.134, and
/// Bilbao & Harrison ISMRA 2016. Within 2.81% of Zwikker-Kosten for r ≥ 3mm, f ≥ 20Hz.
#[derive(Clone, Copy)]
struct LossNode {
    gain: f64,
    /// `(c_q, a_q, 1/(1+a_q))`.
    branch: [(f64, f64, f64); LOSS_BRANCHES],
}

impl LossNode {
    fn new(radius: f64, sr: f64, damp: f64, residue_scale: f64, r0: f64) -> LossNode {
        let unit = damp * residue_scale * 2.0 * (ETA0_PA_S / (RHO0_KG_M3 * sr)).sqrt() / radius;
        let mut branch = [(0.0, 0.0, 0.0); LOSS_BRANCHES];
        let mut sum_c = 0.0;
        for (slot, (&g, &a)) in branch.iter_mut().zip(GHAT.iter().zip(&AHAT)) {
            let inv_1pa = 1.0 / (1.0 + a);
            let c = unit * g * inv_1pa;
            sum_c += c;
            *slot = (c, a, inv_1pa);
        }
        LossNode {
            gain: 1.0 / (1.0 + damp * r0 / (RHO0_KG_M3 * sr) + sum_c),
            branch,
        }
    }

    fn viscous(radius: f64, sr: f64, damp: f64) -> LossNode {
        LossNode::new(radius, sr, damp, 1.0, 3.0 * ETA0_PA_S / (radius * radius))
    }

    /// No thermal `R0`: its analogue is negative and goes as `1/r²` while branches go as
    /// `1/r`, so a thin bore would drive `gain` negative and forfeit passivity. Biases loss
    /// high by ≤4% of α at 3mm/20Hz.
    fn thermal(radius: f64, sr: f64, damp: f64) -> LossNode {
        LossNode::new(radius, sr, damp, (GAMMA0 - 1.0) / SQRT_PRANDTL, 0.0)
    }

    /// Implicit in every branch state at once, a tonehole's admittance included.
    #[inline]
    fn close_out(
        &self,
        rhs: f64,
        state: &mut [f64; LOSS_BRANCHES],
        extra_y: f64,
        extra_hist: f64,
    ) -> f64 {
        let mut acc = rhs;
        for (&(c, _, _), s) in self.branch.iter().zip(state.iter()) {
            acc += c * s;
        }
        let next = if extra_y == 0.0 {
            self.gain * acc
        } else {
            (acc - extra_hist) / (1.0 / self.gain + extra_y)
        };
        for (&(_, a, inv_1pa), s) in self.branch.iter().zip(state.iter_mut()) {
            *s = (*s + a * next) * inv_1pa;
        }
        next
    }
}

pub(crate) struct DuctGrid {
    psi: Vec<f64>,
    v: Vec<f64>,
    n: usize,
    dz: f64,
    #[allow(dead_code)]
    courant: f64,
    s_half: Vec<f64>,
    bar_s: Vec<f64>,
    loss_v: Vec<LossNode>,
    loss_v_state: Vec<[f64; LOSS_BRANCHES]>,
    loss_t: Vec<LossNode>,
    loss_t_state: Vec<[f64; LOSS_BRANCHES]>,
    rad: RadCoeffs,
    /// Inertance current, R2||C node voltage.
    rad_state: (f64, f64),
    toneholes: Vec<ToneholeBranch>,
}

/// `build_grid`'s own `n`; a caller refuses `n<2` with it before building a site.
pub(crate) fn grid_segments(length: f64, sr: f64) -> usize {
    let dt = 1.0 / sr;
    let dz_bound = C0_M_S * dt;
    ((length / dz_bound).floor() as usize).max(MIN_SEGMENTS)
}

fn build_grid(params: &BoreParams, sr: f64) -> DuctGrid {
    let dt = 1.0 / sr;
    let n = grid_segments(params.length, sr);
    let dz = params.length / n as f64;
    let courant = C0_M_S * dt / dz;

    let radius_at = |z: f64| -> f64 {
        params.radius_in + (params.radius_out - params.radius_in) * (z / params.length)
    };
    let s_half: Vec<f64> = (0..n)
        .map(|l| {
            let r = radius_at((l as f64 + 0.5) * dz);
            std::f64::consts::PI * r * r
        })
        .collect();
    let bar_s: Vec<f64> = (0..=n)
        .map(|l| match (l.checked_sub(1), s_half.get(l)) {
            (Some(left), Some(&right)) => 0.5 * (s_half[left] + right),
            (Some(left), None) => s_half[left],
            (None, Some(&right)) => right,
            (None, None) => unreachable!("n >= MIN_SEGMENTS guarantees an s_half entry"),
        })
        .collect();

    let radius_of = |s: f64| (s / std::f64::consts::PI).sqrt();
    let loss_v: Vec<LossNode> = s_half
        .iter()
        .map(|&s| LossNode::viscous(radius_of(s), sr, params.damp_dc))
        .collect();
    let loss_v_state = vec![[0.0; LOSS_BRANCHES]; n];

    // Interior only: the boundaries are governed by the rigid/radiation conditions instead.
    let loss_t: Vec<LossNode> = bar_s[1..n]
        .iter()
        .map(|&s| LossNode::thermal(radius_of(s), sr, params.damp_freq))
        .collect();
    let loss_t_state = vec![[0.0; LOSS_BRANCHES]; n.saturating_sub(1)];

    let toneholes: Vec<ToneholeBranch> = params
        .holes
        .iter()
        .flatten()
        .map(|spec| {
            let node = (spec.pos * n as f64).round().clamp(1.0, (n - 1) as f64) as usize;
            build_tonehole(spec, radius_at(spec.pos * params.length), node, dt, dz)
        })
        .collect();

    DuctGrid {
        psi: vec![0.0; n + 1],
        v: vec![0.0; n],
        n,
        dz,
        courant,
        s_half,
        bar_s,
        loss_v,
        loss_v_state,
        loss_t,
        loss_t_state,
        rad: radiation_coeffs(params.radius_out, dt, dz),
        rad_state: (0.0, 0.0),
        toneholes,
    }
}

pub struct BoreSite {
    duct: DuctGrid,
    excite_index: usize,
    pulse_amp: f64,
    pulse_width: f64,
    dt: f64,
    sample_index: u64,
}

impl BoreSite {
    pub fn new(params: &BoreParams, sr: f64) -> BoreSite {
        let duct = build_grid(params, sr);
        let excite_index =
            ((params.excite_pos * duct.n as f64).round() as usize).min(duct.n.saturating_sub(1));
        BoreSite {
            duct,
            excite_index,
            pulse_amp: params.pulse_amp,
            pulse_width: params.pulse_width,
            dt: 1.0 / sr,
            sample_index: 0,
        }
    }
}

impl BoreSite {
    fn advance(&mut self) -> f64 {
        let t = self.sample_index as f64 * self.dt;
        let source = crate::physics::raised_cosine_pulse(t, self.pulse_amp, self.pulse_width);

        let duct = &mut self.duct;
        let n = duct.n;
        let rho_c2_dt = self.dt * RHO0_KG_M3 * C0_M_S * C0_M_S;

        for l in 0..n {
            let dpsi_dz = (duct.psi[l + 1] - duct.psi[l]) / duct.dz;
            let rhs = duct.v[l] - self.dt * dpsi_dz / RHO0_KG_M3;
            duct.v[l] = duct.loss_v[l].close_out(rhs, &mut duct.loss_v_state[l], 0.0, 0.0);
        }

        let hole_prep: Vec<(usize, f64, f64)> = duct
            .toneholes
            .iter()
            .map(|h| {
                let (y_eff, hist) = h.prepare();
                (h.node(), y_eff, hist)
            })
            .collect();

        for l in 1..n {
            let flux = duct.s_half[l] * duct.v[l] - duct.s_half[l - 1] * duct.v[l - 1];
            let coeff = rho_c2_dt / (duct.bar_s[l] * duct.dz);
            let mut rhs = duct.psi[l] - coeff * flux;
            if l == self.excite_index {
                rhs += coeff * source;
            }
            let mut extra_y = 0.0;
            let mut extra_hist = 0.0;
            for &(node, y_eff, hist) in &hole_prep {
                if node == l {
                    extra_y += coeff * y_eff;
                    extra_hist += coeff * hist;
                }
            }
            duct.psi[l] = duct.loss_t[l - 1].close_out(
                rhs,
                &mut duct.loss_t_state[l - 1],
                extra_y,
                extra_hist,
            );
        }

        for (hole, &(node, _, hist)) in duct.toneholes.iter_mut().zip(&hole_prep) {
            hole.commit(duct.psi[node], hist, self.dt);
        }

        {
            let flux = duct.s_half[0] * duct.v[0];
            let coeff = rho_c2_dt / (duct.bar_s[0] * duct.dz);
            let mut next = duct.psi[0] - coeff * flux;
            if self.excite_index == 0 {
                next += coeff * source;
            }
            duct.psi[0] = next;
        }

        {
            let RadCoeffs {
                a_p,
                b_p,
                bv,
                cg,
                r1,
                k,
            } = duct.rad;
            let (v1, p1) = duct.rad_state;
            let v_last = duct.v[n - 1];
            let base = v1 - (a_p / r1) * p1;
            let next = (duct.psi[n] + k * (v_last - base)) / (1.0 + k * cg);
            let p1_next = a_p * p1 + b_p * next;
            let v1_next = v1 + bv * next;
            duct.psi[n] = next;
            duct.rad_state = (v1_next, p1_next);
        }

        let sample = duct.psi[n];
        self.sample_index += 1;
        sample
    }
}

impl Solver for BoreSite {
    fn step(&mut self) -> Result<f64, SampleError> {
        Ok(self.advance())
    }
}
