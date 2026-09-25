// Concern: one tonehole's two-port branch and the radiation load it opens onto | Non-concern: the duct it shunts (darabundit_scavone.rs) | IO: (ToneholeSpec, dt, dz) -> a branch

use super::darabundit_scavone::{C0_M_S, RHO0_KG_M3, ToneholeSpec};
#[derive(Clone)]
pub(super) struct RadCoeffs {
    pub(super) a_p: f64,
    pub(super) b_p: f64,
    pub(super) bv: f64,
    pub(super) cg: f64,
    pub(super) r1: f64,
    pub(super) k: f64,
}

const LEVINE_SCHWINGER_END_CORRECTION: f64 = 0.613;

/// Ours: the model gives the topology and not these two ratios.
const RADIATION_RESISTANCE_RATIO: f64 = 0.505;
const RADIATION_COMPLIANCE_FACTOR: f64 = 1.111;

pub(super) fn radiation_coeffs(radius_out: f64, dt: f64, dz: f64) -> RadCoeffs {
    let r1 = RHO0_KG_M3 * C0_M_S;
    let r2 = RADIATION_RESISTANCE_RATIO * r1;
    let l = LEVINE_SCHWINGER_END_CORRECTION * RHO0_KG_M3 * radius_out;
    let c = RADIATION_COMPLIANCE_FACTOR * radius_out / (RHO0_KG_M3 * C0_M_S * C0_M_S);
    let alpha = dt / (c * r1);
    let beta = dt / (c * r2);
    let denom = 1.0 + alpha + beta;
    let a_p = 1.0 / denom;
    let b_p = alpha / denom;
    let bv = dt / l;
    let cg = bv + (1.0 - b_p) / r1;
    let k = dt * RHO0_KG_M3 * C0_M_S * C0_M_S / dz;
    RadCoeffs {
        a_p,
        b_p,
        bv,
        cg,
        r1,
        k,
    }
}

/// One hole's fixed shunt (eq. 114a-120b); D&S §7.3's switching PHS is skipped.
#[derive(Clone)]
pub(super) enum ToneholeBranch {
    /// Series `L_i`-`C_c`-`R_c` (eq. 118's small-angle `cot`, §7.2.2).
    Closed {
        node: usize,
        y_eff: f64,
        li_over_dt: f64,
        inv_cc: f64,
        i: f64,
        qc: f64,
    },
    /// Series `L_total = L_i+L_o` (eq. 120b) with the radiation two-port at radius `b` (eq. 120a).
    Open {
        node: usize,
        y_eff: f64,
        l_over_dt: f64,
        rad: RadCoeffs,
        i: f64,
        v1: f64,
        p1: f64,
    },
}

impl ToneholeBranch {
    pub(super) fn node(&self) -> usize {
        match self {
            ToneholeBranch::Closed { node, .. } | ToneholeBranch::Open { node, .. } => *node,
        }
    }

    pub(super) fn prepare(&self) -> (f64, f64) {
        match self {
            ToneholeBranch::Closed {
                y_eff,
                li_over_dt,
                inv_cc,
                i,
                qc,
                ..
            } => (*y_eff, *y_eff * (*li_over_dt * *i - *inv_cc * *qc)),
            ToneholeBranch::Open {
                y_eff,
                l_over_dt,
                rad,
                i,
                v1,
                p1,
                ..
            } => {
                let base = *v1 - (rad.a_p / rad.r1) * *p1;
                (*y_eff, *y_eff * *l_over_dt * *i + (*y_eff / rad.cg) * base)
            }
        }
    }

    pub(super) fn commit(&mut self, p_new: f64, hist: f64, dt: f64) {
        match self {
            ToneholeBranch::Closed { y_eff, i, qc, .. } => {
                let i_next = *y_eff * p_new + hist;
                *qc += dt * i_next;
                *i = i_next;
            }
            ToneholeBranch::Open {
                y_eff,
                l_over_dt,
                rad,
                i,
                v1,
                p1,
                ..
            } => {
                let i_next = *y_eff * p_new + hist;
                let p_junction = p_new - *l_over_dt * (i_next - *i);
                *p1 = rad.a_p * *p1 + rad.b_p * p_junction;
                *v1 += rad.bv * p_junction;
                *i = i_next;
            }
        }
    }
}

/// Our own choice; §7.2.2 names the goal and no formula.
const CLOSED_ZETA_FLOOR: f64 = 0.05;

/// eq. 114a-120b; `t_h` is `spec.height`.
pub(super) fn build_tonehole(
    spec: &ToneholeSpec,
    local_radius: f64,
    node: usize,
    dt: f64,
    dz: f64,
) -> ToneholeBranch {
    let b = spec.radius;
    let t_h = spec.height;
    let d = b / local_radius;
    let s_h = std::f64::consts::PI * b * b;
    let t_i = (0.822 - 0.095 * d - 1.566 * d.powi(2) + 2.138 * d.powi(3) - 1.640 * d.powi(4)
        + 0.502 * d.powi(5))
        * b;
    let t_m = (b * d / 8.0) * (1.0 + 0.207 * d.powi(3));
    let l_i = (RHO0_KG_M3 / s_h) * t_i;

    if spec.open {
        let l_o = (RHO0_KG_M3 / s_h) * (t_h + t_m);
        let l_total = l_i + l_o;
        let l_over_dt = l_total / dt;
        let rad = radiation_coeffs(b, dt, dz);
        let y_eff = rad.cg / (1.0 + rad.cg * l_over_dt);
        ToneholeBranch::Open {
            node,
            y_eff,
            l_over_dt,
            rad,
            i: 0.0,
            v1: 0.0,
            p1: 0.0,
        }
    } else {
        // eq. 118 small-angle cot(x)≈1/x: Helmholtz C_c=S_h*(t_h+t_m)/(rho*c^2).
        let cc = s_h * (t_h + t_m) / (RHO0_KG_M3 * C0_M_S * C0_M_S);
        let f_res = 1.0 / (2.0 * std::f64::consts::PI * (l_i * cc).sqrt());
        let f_nyq = 0.5 / dt;
        let zeta = (f_res / f_nyq).clamp(CLOSED_ZETA_FLOOR, 1.0);
        let r_c = 2.0 * zeta * (l_i / cc).sqrt();
        let li_over_dt = l_i / dt;
        let y_eff = 1.0 / (li_over_dt + r_c + dt / cc);
        ToneholeBranch::Closed {
            node,
            y_eff,
            li_over_dt,
            inv_cc: 1.0 / cc,
            i: 0.0,
            qc: 0.0,
        }
    }
}
