// Concern: a unison's energy and its scheme's stability | Non-concern: stepping the site (chaigne_askenfelt.rs) | IO: (&ChaigneAskenfeltSite) -> energy, stable or not

//! Times `k^2`, a unison steps as `P dv + K pbar + B vbar = 0`, `dp = vbar`, `P`, `K`, `B`
//! symmetric, so `E = (v'Pv + pbar'K pbar)/(2k^2)` falls by `vbar'B vbar/k^2` a step. Over
//! `sqrt(m_i) u` in sine modes and `sqrt(d) p`, `P` is an arrow: modes down the diagonal, the
//! bridge's row coupling them.

use crate::physics::arrow::Arrow;
use crate::physics::ball::Ball;
use crate::physics::chaigne_askenfelt::ChaigneAskenfeltSite;
use crate::physics::stiff_string::StringGrid;
use crate::physics::string_energy::{energy, gamma};

/// `(joules, a bound over their rounding)`, the bridge's own `(M + k R/2) (dp/k)^2/2` added.
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

fn exact(x: f64) -> Ball {
    Ball::exact(x)
}

fn over(x: Ball, by: Ball) -> Ball {
    x.div(by).expect("a positive divisor")
}

fn root(x: Ball) -> Ball {
    x.sqrt().expect("a positive bound")
}

/// `sin(m pi/n)`, `sin(2m pi/n)`, and `alpha = 1 - sigma/2 - D/4`.
struct Mode {
    s1: Ball,
    s2: Ball,
    alpha: Ball,
}

fn modes(grid: &StringGrid) -> Vec<Mode> {
    let n = exact(grid.n as f64);
    let [l2, u2, a, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_a, grid.damp_b];
    (1..grid.n)
        .map(|m| {
            let turn = Ball::pi().scale(m as f64);
            let s = over(turn, n.scale(2.0)).sin().square();
            let d = s.scale(4.0 * l2).add(s.square().scale(16.0 * u2));
            let sigma = s.scale(4.0 * b).add(exact(a));
            Mode {
                s1: over(turn, n).sin(),
                s2: over(turn.scale(2.0), n).sin(),
                alpha: exact(1.0).sub(sigma.scale(0.5)).sub(d.scale(0.25)),
            }
        })
        .collect()
}

fn node_mass(grid: &StringGrid) -> Ball {
    exact(grid.rho).scale(grid.dx)
}

/// `P` over its bridge corner `d`: rows `n-1` and `n-2` meet the bridge, per unit node mass,
/// at `(lambda^2 + 2mu^2)/4 + b/2` and `-mu^2/4`.
fn mass(site: &ChaigneAskenfeltSite) -> Option<Arrow> {
    let k = site.dt;
    let r = site.bridge_r_enclosed();
    let mut corner = exact(site.bridge_mass).add(r.scale(k / 2.0));
    for grid in &site.strings {
        let m = node_mass(grid);
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let own = exact(l2 / 2.0)
            .sub(exact(l2).add(exact(u2)).scale(0.25))
            .sub(exact(b / 2.0));
        corner = corner.add(m.mul(own));
    }
    (corner.lo() > 0.0).then_some(())?;
    let scale = corner.c;
    let mut diag = Vec::new();
    for grid in &site.strings {
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let g = root(over(
            node_mass(grid).scale(2.0 / grid.n as f64),
            exact(scale),
        ));
        let bend = exact(l2).add(exact(u2).scale(2.0));
        let (one, two) = (bend.scale(0.25).add(exact(b / 2.0)), exact(-u2 / 4.0));
        for mode in modes(grid) {
            diag.push((mode.alpha, g.mul(one.mul(mode.s1).add(two.mul(mode.s2)))));
        }
    }
    Some(Arrow {
        diag,
        corner: over(corner, exact(scale)),
    })
}

pub(crate) fn unison_stable(site: &ChaigneAskenfeltSite) -> bool {
    mass(site).is_some_and(|m| m.proven_positive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Solver;
    use crate::physics::chaigne_askenfelt::ChaigneAskenfeltParams;

    fn dissipated(site: &ChaigneAskenfeltSite, older: &[Vec<f64>]) -> f64 {
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
                .map(|&(j, _, rho)| 2.0 * rho * vbar[j] * vbar[j])
                .sum();
            let m = grid.rho * grid.dx;
            summed += m * (grid.damp_a * own + grid.damp_b * bent + felt);
            bridge += m * grid.courant_sq;
        }
        let vbar_p = (site.bridge_now - older[0][site.strings[0].n]) / 2.0;
        (summed + bridge * vbar_p * vbar_p) / (k * k)
    }

    fn unison(f0: f64, bridge_mass: f64, damper_r: f64) -> ChaigneAskenfeltParams {
        ChaigneAskenfeltParams {
            unison_count: 3.0,
            detune: 1.0,
            bridge_coupling: 100.0,
            bridge_mass,
            string_cents: [-0.2, 0.0, 0.25],
            string_hammer_k_ratio: [1.0, 0.8, 0.6],
            damper_r,
            ..ChaigneAskenfeltParams::at(f0)
        }
    }

    #[test]
    fn a_unison_loses_exactly_what_its_losses_and_its_bridge_dissipate() {
        for (f0, bridge_mass, damper_r) in [
            (65.406, 1.0, 0.0),
            (261.63, 1.0, 0.1),
            (261.63, 0.0, 0.0),
            (2093.0, 0.3, 0.1),
        ] {
            let p = unison(f0, bridge_mass, damper_r);
            let mut site = ChaigneAskenfeltSite::new(&p, 44_100.0).expect("a grid");
            while !site.let_go() {
                site.step(&[]).expect("a sample");
            }
            for step in 0..4410 {
                let older: Vec<Vec<f64>> = site.strings.iter().map(|g| g.y_prev.clone()).collect();
                let before = site.energy();
                site.step(&[]).expect("a sample");
                let residual = site.energy() - before + dissipated(&site, &older);
                assert!(
                    residual.abs() <= 1e-12 * before,
                    "f0 {f0} bridge {bridge_mass} damper {damper_r} step {step}: E {before} misses its balance \
                     by {residual}"
                );
            }
        }
    }

    /// A dashpot moving every sample dissipates what it removes that sample; a spring that
    /// jumps adds, at the jump, its new stiffness's energy over the motion it finds.
    #[test]
    fn a_moving_felt_loses_what_it_dissipates_and_a_spring_jump_adds_its_own_energy() {
        let mut site =
            ChaigneAskenfeltSite::new(&unison(261.63, 1.0, 0.0), 44_100.0).expect("a grid");
        while !site.let_go() {
            site.step(&[0.0, 0.0]).expect("a sample");
        }
        let mut seed: u64 = 0x5eed;
        for step in 0..4410 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let r = 0.2 * (seed >> 11) as f64 / (1u64 << 53) as f64;
            let k = if step < 2000 { 0.0 } else { 2e3 };
            let older: Vec<Vec<f64>> = site.strings.iter().map(|g| g.y_prev.clone()).collect();
            let found: Vec<(Vec<f64>, Vec<f64>)> = site
                .strings
                .iter()
                .map(|g| (g.y_now.clone(), g.y_prev.clone()))
                .collect();
            let (before, springs) = (site.energy(), site.felt.clone());
            site.step(&[r, k]).expect("a sample");
            let jumped: f64 = site
                .strings
                .iter()
                .enumerate()
                .map(|(i, grid)| {
                    let (y, yp) = &found[i];
                    let w = grid.rho * grid.dx / (site.dt * site.dt) / 2.0;
                    let moved: f64 = site.felt[i]
                        .iter()
                        .zip(&springs[i])
                        .map(|(&(j, now, _), &(_, was, _))| {
                            (now - was) * (y[j] * y[j] + yp[j] * yp[j]) / 2.0
                        })
                        .sum();
                    w * moved
                })
                .sum();
            let residual = site.energy() - before + dissipated(&site, &older) - jumped;
            assert!(
                residual.abs() <= 1e-12 * before,
                "step {step}: E {before} misses its balance by {residual} (jump {jumped})"
            );
        }
    }
}
