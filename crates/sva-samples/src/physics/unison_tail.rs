// Concern: a unison's energy on its shared bridge | Non-concern: stepping the site (chaigne_askenfelt.rs), one pinned string (string_tail.rs) | IO: (&ChaigneAskenfeltSite) -> energy

//! Times `k^2`, a unison steps as `P dv + K pbar + B vbar = 0`, `dp = vbar`, over every free
//! node and the bridge, `P`, `K` and `B` symmetric. So `E = (v'Pv + pbar'K pbar)/(2k^2)`
//! falls by exactly `vbar'B vbar/k^2` a step.

use crate::physics::chaigne_askenfelt::ChaigneAskenfeltSite;
use crate::physics::string_tail::{energy, gamma};

/// `(joules, a bound over their rounding)`: each string's energy with its end at the bridge,
/// and the bridge's own `(M + k R/2) (dp/k)^2/2`.
pub(crate) fn unison_energy(site: &ChaigneAskenfeltSite) -> (f64, f64) {
    let (mut joules, mut upper) = (0.0, 0.0);
    for (i, grid) in site.strings.iter().enumerate() {
        let (e, bound) = energy(grid, site.dt, site.springs(i));
        joules += e;
        upper += bound;
    }
    let (p, pp) = (site.bridge_now, site.bridge_prev);
    let mass = site.bridge_mass + site.dt * site.bridge_r() / 2.0;
    let moved = (p - pp) / site.dt;
    let size = (p.abs() + pp.abs()) / site.dt;
    let bridge = mass / 2.0 * moved * moved;
    let slack = mass / 2.0 * size * size * gamma(24.0);
    let summed = gamma(2.0 * site.strings.len() as f64 + 4.0);
    (
        joules + bridge,
        (upper + bridge + slack) * (1.0 + summed) * (1.0 + 4.0 * f64::EPSILON),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Solver;
    use crate::physics::chaigne_askenfelt::ChaigneAskenfeltParams;

    /// `vbar'B vbar/k^2` joules for the step just taken from `older`.
    fn dissipated(site: &ChaigneAskenfeltSite, older: &[Vec<f64>], share: Option<f64>) -> f64 {
        let k = site.dt;
        let mut summed = 0.0;
        let mut bridge = site.dt * site.bridge_r();
        for (i, grid) in site.strings.iter().enumerate() {
            let n = grid.n;
            let vbar: Vec<f64> = (0..=n)
                .map(|j| (grid.y_now[j] - older[i][j]) / 2.0)
                .collect();
            let own: f64 = vbar[1..n].iter().map(|x| x * x).sum();
            let bent: f64 = (0..n).map(|j| (vbar[j + 1] - vbar[j]).powi(2)).sum();
            let felt: f64 = site.felt[i]
                .iter()
                .map(|&(j, _, rho)| 2.0 * rho * share.unwrap_or(0.0) * vbar[j] * vbar[j])
                .sum();
            let m = grid.rho * grid.dx;
            summed += m * (grid.damp_a * own + grid.damp_b * bent + felt);
            bridge += m * grid.courant_sq;
        }
        let vbar_p = (site.bridge_now - older[0][site.strings[0].n]) / 2.0;
        (summed + bridge * vbar_p * vbar_p) / (k * k)
    }

    fn unison(f0: f64, bridge_mass: f64, release: f64) -> ChaigneAskenfeltParams {
        ChaigneAskenfeltParams {
            unison_count: 3.0,
            detune: 1.0,
            bridge_coupling: 100.0,
            bridge_mass,
            string_cents: [-0.2, 0.0, 0.25],
            string_hammer_k_ratio: [1.0, 0.8, 0.6],
            release,
            ..ChaigneAskenfeltParams::at(f0)
        }
    }

    #[test]
    fn a_unison_loses_exactly_what_its_losses_and_its_bridge_dissipate() {
        for (f0, bridge_mass, release) in [
            (65.406, 1.0, f64::INFINITY),
            (261.63, 1.0, 0.05),
            (261.63, 0.0, f64::INFINITY),
            (2093.0, 0.3, 0.05),
        ] {
            let p = unison(f0, bridge_mass, release);
            let mut site = ChaigneAskenfeltSite::new(&p, 44_100.0).expect("a grid");
            while !site.let_go() {
                site.step().expect("a sample");
            }
            for step in 0..4410 {
                let older: Vec<Vec<f64>> = site.strings.iter().map(|g| g.y_prev.clone()).collect();
                let (before, share) = (site.energy(), site.pressing());
                site.step().expect("a sample");
                let residual = site.energy() - before + dissipated(&site, &older, share);
                assert!(
                    residual.abs() <= 1e-12 * before,
                    "f0 {f0} bridge {bridge_mass} step {step}: E {before} misses its balance \
                     by {residual}"
                );
            }
        }
    }
}
