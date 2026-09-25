// Concern: a unison's energy, its scheme's stability, and the bound both give later samples | Non-concern: stepping the site (chaigne_askenfelt.rs) | IO: (&ChaigneAskenfeltSite) -> energy, bound

//! Times `k^2`, a unison steps as `P dv + K pbar + B vbar = 0`, `dp = vbar`, `P`, `K`, `B`
//! symmetric, so `E = (v'Pv + pbar'K pbar)/(2k^2)` falls by `vbar'B vbar/k^2` a step. Over
//! `sqrt(m_i) u` in sine modes and `sqrt(d) p`, each is an arrow: modes down the diagonal,
//! the bridge's row coupling them.

use crate::physics::arrow::{Arrow, ceiling, floor};
use crate::physics::ball::Ball;
use crate::physics::chaigne_askenfelt::ChaigneAskenfeltSite;
use crate::physics::stiff_string::StringGrid;
use crate::physics::string_tail::{Settling, Unringing, energy, gamma, stencil_mass};

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

/// `sin(m pi/n)`, `sin(2m pi/n)`, and `alpha = 1 - sigma/2 - D/4`, `D` and `sigma`.
struct Mode {
    s1: Ball,
    s2: Ball,
    alpha: Ball,
    d: Ball,
    sigma: Ball,
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
                d,
                sigma,
            }
        })
        .collect()
}

fn node_mass(grid: &StringGrid) -> Ball {
    exact(grid.rho).scale(grid.dx)
}

/// `T/h = rho dx lambda^2/k^2`.
fn tension_pull(grid: &StringGrid, k: f64) -> Ball {
    over(node_mass(grid).scale(grid.courant_sq), exact(k).square())
}

/// `P`, `K` and `B` over the bridge corner `d` of `P`, and each string's modes.
struct Forms {
    mass: Arrow,
    stiff: Arrow,
    loss: Arrow,
    strings: Vec<Vec<Mode>>,
    scale: f64,
}

/// Rows `n-1` and `n-2` meet the bridge, per unit node mass, at `(lambda^2 + 2mu^2)/4 + b/2`
/// and `-mu^2/4` in `P`, `-(lambda^2 + 2mu^2)` and `mu^2` in `K`, and row `n-1` at `-b` in `B`.
fn forms(site: &ChaigneAskenfeltSite) -> Option<Forms> {
    let k = site.dt;
    let r = site.bridge_r_enclosed();
    let mut corner = [
        exact(site.bridge_mass).add(r.scale(k / 2.0)),
        exact(0.0),
        r.scale(k),
    ];
    for grid in &site.strings {
        let m = node_mass(grid);
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let own = exact(l2 / 2.0)
            .sub(exact(l2).add(exact(u2)).scale(0.25))
            .sub(exact(b / 2.0));
        corner[0] = corner[0].add(m.mul(own));
        corner[1] = corner[1].add(m.mul(exact(l2).add(exact(u2))));
        corner[2] = corner[2].add(m.mul(exact(l2).add(exact(b))));
    }
    (corner[0].lo() > 0.0).then_some(())?;
    let scale = corner[0].c;
    let [mut mass, mut stiff, mut loss] = [Vec::new(), Vec::new(), Vec::new()];
    let mut strings = Vec::new();
    for grid in &site.strings {
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let g = root(over(
            node_mass(grid).scale(2.0 / grid.n as f64),
            exact(scale),
        ));
        let bend = exact(l2).add(exact(u2).scale(2.0));
        let pull_p = (bend.scale(0.25).add(exact(b / 2.0)), exact(-u2 / 4.0));
        let pull_k = (bend.neg(), exact(u2));
        let found = modes(grid);
        for mode in &found {
            let at = |(one, two): (Ball, Ball)| g.mul(one.mul(mode.s1).add(two.mul(mode.s2)));
            mass.push((mode.alpha, at(pull_p)));
            stiff.push((mode.d, at(pull_k)));
            loss.push((mode.sigma, g.mul(mode.s1.scale(-b))));
        }
        strings.push(found);
    }
    let [cm, ck, cb] = corner.map(|c| over(c, exact(scale)));
    Some(Forms {
        mass: Arrow {
            diag: mass,
            corner: cm,
        },
        stiff: Arrow {
            diag: stiff,
            corner: ck,
        },
        loss: Arrow {
            diag: loss,
            corner: cb,
        },
        strings,
        scale,
    })
}

impl Forms {
    /// Node `j` of string `i` in that string's modes, `sqrt(2/n) sin(m pi j/n)`, and the sign
    /// `(-1)^(m+1)` its bridge coupling carries.
    fn node(&self, site: &ChaigneAskenfeltSite, i: usize, j: usize) -> Vec<(usize, Ball, Ball)> {
        let start: usize = site.strings[..i].iter().map(|g| g.n - 1).sum();
        let n = site.strings[i].n;
        let norm = root(exact(2.0 / n as f64));
        (1..n)
            .map(|m| {
                let x = norm.mul(over(Ball::pi().scale((m * j) as f64), exact(n as f64)).sin());
                let sign = exact(if m % 2 == 1 { 1.0 } else { -1.0 });
                (start + m - 1, x, sign)
            })
            .collect()
    }
}

pub(crate) fn unison_stable(site: &ChaigneAskenfeltSite) -> bool {
    forms(site).is_some_and(|f| f.mass.proven_positive())
}

/// Bounds on `P`, `K` and `B` with the felt pressed whole: plain eigenvalues in the scaled
/// coordinates, and ratios `x'Xx/x'Yx`.
struct Ratios {
    mass: (f64, f64),
    stiff: (f64, f64),
    loss_mass: (f64, f64),
    stiff_mass: f64,
    mass_stiff: f64,
    loss_stiff: f64,
}

/// The felt at node `f` adds `k_f e_f e_f'` to `K`, a quarter of it to `P` and `2 rho_f` to
/// `B`, and `(e_f'x)^2 <= (P^-1)_ff x'Px`.
fn ratios(site: &ChaigneAskenfeltSite, forms: &Forms) -> Option<Ratios> {
    let id = Arrow::identity(forms.mass.diag.len());
    let [mut spring_p, mut dashpot_p, mut dashpot_k] = [exact(0.0); 3];
    let mut spring = 0.0f64;
    for (i, felt) in site.felt.iter().enumerate() {
        for &(j, kappa, rho) in felt {
            let parts = forms.node(site, i, j);
            let over_p = forms.mass.inverse_at(&parts)?;
            let over_k = forms.stiff.inverse_at(&parts)?;
            spring_p = spring_p.add(over_p.scale(kappa));
            dashpot_p = dashpot_p.add(over_p.scale(2.0 * rho));
            dashpot_k = dashpot_k.add(over_k.scale(2.0 * rho));
            spring = spring.max(kappa);
        }
    }
    let plus = |a: f64, b: Ball| exact(a).add(b).hi();
    let spread = exact(1.0).add(spring_p.scale(0.25));
    Some(Ratios {
        mass: (
            floor(&forms.mass, &id)?,
            plus(ceiling(&forms.mass, &id)?, exact(spring / 4.0)),
        ),
        stiff: (
            floor(&forms.stiff, &id)?,
            plus(ceiling(&forms.stiff, &id)?, exact(spring)),
        ),
        loss_mass: (
            over(exact(floor(&forms.loss, &forms.mass)?), spread).lo(),
            plus(ceiling(&forms.loss, &forms.mass)?, dashpot_p),
        ),
        stiff_mass: plus(ceiling(&forms.stiff, &forms.mass)?, spring_p),
        mass_stiff: ceiling(&forms.mass, &forms.stiff)?.max(0.25),
        loss_stiff: plus(ceiling(&forms.loss, &forms.stiff)?, dashpot_k),
    })
}

/// `|u|^2 + |u-|^2 <= reach 2 k^2 E` over the scaled nodes, bridge and both steps.
fn node_reach(mass_lo: f64, stiff_lo: f64) -> Option<f64> {
    (mass_lo > 0.0 && stiff_lo > 0.0).then_some(())?;
    Some(
        over(exact(2.0), exact(stiff_lo))
            .hi()
            .max(over(exact(0.5), exact(mass_lo)).hi()),
    )
}

/// `c` with `|sample| <= c sqrt(E)`: `c^2 = 2 (sum T/h + k^2/4 l'P^-1 l)` for the tension
/// force `l'u`, and the rounding of the sample itself.
pub(crate) fn unison_gain(site: &ChaigneAskenfeltSite) -> Option<f64> {
    let forms = forms(site)?;
    let id = Arrow::identity(forms.mass.diag.len());
    let reach = node_reach(floor(&forms.mass, &id)?, floor(&forms.stiff, &id)?)?;
    let k = site.dt;
    let schur = forms.mass.schur()?;
    (schur.lo() > 0.0).then_some(())?;
    let [mut own, mut through, mut pull, mut read] = [exact(0.0); 4];
    let bridge = root(exact(forms.scale)).recip()?;
    for (grid, found) in site.strings.iter().zip(&forms.strings) {
        let t = tension_pull(grid, k);
        let m = node_mass(grid);
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let one = exact(l2)
            .add(exact(u2).scale(2.0))
            .scale(0.25)
            .add(exact(b / 2.0));
        let two = exact(-u2 / 4.0);
        let [mut alone, mut crossed] = [exact(0.0); 2];
        for mode in found {
            let coupled = one.mul(mode.s1).add(two.mul(mode.s2));
            alone = alone.add(mode.s1.square().div(mode.alpha)?);
            crossed = crossed.add(mode.s1.mul(coupled).div(mode.alpha)?);
        }
        let per = exact(2.0 / grid.n as f64);
        own = own.add(t.square().div(m)?.mul(per).mul(alone));
        through = through.add(t.mul(exact(1.0).add(per.mul(crossed))));
        pull = pull.add(t);
        read = read.add(t.mul(root(m).recip()?.add(bridge)));
    }
    let inverse = own.add(through.square().div(exact(forms.scale).mul(schur))?);
    let main = root(pull.add(inverse.scale(k * k / 4.0)).scale(2.0));
    let rounding = read.mul(root(exact(2.0 * reach))).scale(k * gamma(16.0));
    Some(main.add(rounding).hi())
}

/// The felt's largest spring and dashpot on any node, per unit node mass.
fn felt_extent(site: &ChaigneAskenfeltSite) -> (f64, f64) {
    let felt = site.felt.iter().flatten();
    let spring = felt.clone().map(|f| f.1).fold(0.0f64, f64::max);
    let dashpot = felt.map(|f| f.2).fold(0.0f64, f64::max);
    (spring, dashpot)
}

/// `sqrt(||C||_1 ||C||_inf)` for the scaled magnitudes one step's rounding reads: each string
/// row through its stencil, the bridge and the felt, the bridge row through its sum over `D`.
fn rounding_norm(site: &ChaigneAskenfeltSite, scale: f64) -> Option<f64> {
    let k = site.dt;
    let (spring, dashpot) = felt_extent(site);
    let felt = exact(dashpot).add(exact(spring / 2.0));
    let damper = site.bridge_r_enclosed().div(exact(k))?;
    let inertia = exact(site.bridge_mass).div(exact(k).square())?;
    let [mut denominator, mut own, mut own_now] = [
        damper.add(inertia),
        damper.add(inertia.scale(3.0)),
        damper.add(inertia.scale(2.0)),
    ];
    let mut near = exact(0.0);
    for grid in &site.strings {
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let w = over(node_mass(grid), exact(k).square());
        denominator = denominator.add(w.scale(l2));
        own = own.add(w.mul(exact(u2).add(exact(2.0 * b))));
        own_now = own_now.add(w.mul(exact(u2).add(exact(b))));
        let ratio = root(over(node_mass(grid), exact(scale)));
        near = near.add(ratio.mul(exact(l2).add(exact(7.0 * u2)).add(exact(b))));
    }
    let (mut rows, mut cols, mut bridge_row) = (0.0f64, 0.0f64, own);
    for grid in &site.strings {
        let [l2, u2, b] = [grid.courant_sq, grid.stiff_sq, grid.damp_b];
        let w = over(node_mass(grid), exact(k).square());
        let ratio = root(over(node_mass(grid), exact(scale)));
        let stencil = exact(stencil_mass(grid)).add(felt);
        let reads_bridge = exact(l2).add(exact(6.0 * u2)).add(exact(2.0 * b));
        rows = rows.max(stencil.add(ratio.mul(reads_bridge)).hi());
        let read_by = w.mul(exact(l2).add(exact(2.0 * u2)).add(exact(b)));
        let col = stencil
            .add(exact(2.0 * u2))
            .add(read_by.div(ratio.mul(denominator))?);
        cols = cols.max(col.hi());
        let reach = w.mul(exact(l2).add(exact(3.0 * u2)).add(exact(2.0 * b)));
        bridge_row = bridge_row.add(reach.div(ratio)?);
    }
    rows = rows.max(bridge_row.div(denominator)?.hi());
    cols = cols.max(near.add(own_now.div(denominator)?).hi());
    Some(root(exact(rows).mul(exact(cols))).hi())
}

/// A step's rounding lifts `sqrt(E)` by at most `per_step`; `F = E + eps (pbar'P v + pbar'B
/// pbar/2)` contracts by `q` a step once the felt is whole, so all rounding after lifts
/// `sqrt(E)` by at most `after`.
pub(crate) fn unison_settling(site: &ChaigneAskenfeltSite) -> Result<Settling, Unringing> {
    let forms = forms(site).ok_or(Unringing::Lossless)?;
    let r = ratios(site, &forms).ok_or(Unringing::Lossless)?;
    let reach = node_reach(r.mass.0, r.stiff.0).ok_or(Unringing::Lossless)?;
    if r.loss_mass.0 <= 0.0 {
        return Err(Unringing::Lossless);
    }
    let node = rounding_norm(site, forms.scale).ok_or(Unringing::Lossless)?;
    let lift_per = root(
        exact(r.mass.1)
            .add(exact(r.stiff.1).scale(0.25))
            .scale(reach),
    );
    let gamma_e = lift_per.scale(node * gamma(32.0));
    let c1 = root(exact(r.mass_stiff));
    let cross = c1.add(exact(r.loss_stiff));
    let swing = root(
        exact(r.stiff_mass)
            .scale(2.0)
            .add(exact(r.loss_mass.1).square()),
    );
    let widest = exact(1.0).add(swing.scale(0.5)).square();
    let most = r.loss_mass.0 / 2.0 * (1.0 - 4.0 * f64::EPSILON);
    let mut best: Option<(Ball, Ball)> = None;
    for share in [0.5, 0.25, 0.125, 0.0625, 0.03125] {
        let eps = exact(most.min(over(exact(share), cross).lo()));
        let (low, high) = (exact(1.0).sub(eps.mul(c1)), exact(1.0).add(eps.mul(cross)));
        if low.lo() <= 0.0 {
            continue;
        }
        let fall = over(eps.scale(2.0), high.mul(widest));
        let q = root(exact(1.0).add(fall).recip().expect("a positive fall"));
        let lift = gamma_e.mul(root(over(high, low)));
        let margin = exact(1.0).sub(q).sub(lift);
        if margin.lo() > 0.0 && best.is_none_or(|(m, _)| margin.lo() > m.lo()) {
            best = Some((margin, lift));
        }
    }
    let (margin, lift) = best.ok_or(Unringing::Rounding)?;
    Ok(Settling {
        per_step: exact(1.0).add(gamma_e).hi(),
        after: exact(1.0).add(over(lift, margin)).hi(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Solver;
    use crate::physics::chaigne_askenfelt::ChaigneAskenfeltParams;

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

    /// `x'Ax` of one arrow over a state given node by node, each string moved into its sine
    /// modes and scaled by `sqrt(m)`, the bridge by `sqrt(d)`.
    fn quadratic(arrow: &Arrow, site: &ChaigneAskenfeltSite, forms: &Forms, x: &[Vec<f64>]) -> f64 {
        let mut sum = 0.0;
        let mut index = 0;
        let bridge = x[0][site.strings[0].n] * forms.scale.sqrt();
        for (grid, nodes) in site.strings.iter().zip(x) {
            let n = grid.n;
            let scale = (grid.rho * grid.dx * 2.0 / n as f64).sqrt();
            for m in 1..n {
                let q: f64 = (1..n)
                    .map(|j| nodes[j] * (std::f64::consts::PI * (m * j) as f64 / n as f64).sin())
                    .sum::<f64>()
                    * scale;
                let (a, z) = arrow.diag[index];
                let sign = if m % 2 == 1 { 1.0 } else { -1.0 };
                sum += a.c * q * q + 2.0 * bridge * sign * z.c * q;
                index += 1;
            }
        }
        sum + arrow.corner.c * bridge * bridge
    }

    #[test]
    fn the_arrows_are_the_forms_the_unison_steps_by() {
        for (f0, bridge_mass) in [(65.406, 1.0), (261.63, 0.0), (2093.0, 0.3)] {
            let p = unison(f0, bridge_mass, f64::INFINITY);
            let mut site = ChaigneAskenfeltSite::new(&p, 44_100.0).expect("a grid");
            for _ in 0..3000 {
                site.step().expect("a sample");
            }
            let forms = forms(&site).expect("a positive corner");
            let k2 = site.dt * site.dt;
            let older: Vec<Vec<f64>> = site.strings.iter().map(|g| g.y_prev.clone()).collect();
            let pair = |f: fn(f64, f64) -> f64| -> Vec<Vec<f64>> {
                site.strings
                    .iter()
                    .map(|g| {
                        g.y_now
                            .iter()
                            .zip(&g.y_prev)
                            .map(|(a, b)| f(*a, *b))
                            .collect()
                    })
                    .collect()
            };
            let (v, mid) = (pair(|a, b| a - b), pair(|a, b| (a + b) / 2.0));
            let held = quadratic(&forms.mass, &site, &forms, &v)
                + quadratic(&forms.stiff, &site, &forms, &mid);
            let energy = site.energy();
            assert!(
                (held / 2.0 - k2 * energy).abs() <= 1e-9 * k2 * energy,
                "f0 {f0}: the arrows hold {} where the site holds {}",
                held / 2.0 / k2,
                energy
            );
            site.step().expect("a sample");
            let vbar: Vec<Vec<f64>> = site
                .strings
                .iter()
                .zip(&older)
                .map(|(g, o)| g.y_now.iter().zip(o).map(|(a, b)| (a - b) / 2.0).collect())
                .collect();
            let lost = quadratic(&forms.loss, &site, &forms, &vbar) / k2;
            let direct = dissipated(&site, &older, None);
            assert!(
                (lost - direct).abs() <= 1e-9 * direct,
                "f0 {f0}: B dissipates {lost}, the step {direct}"
            );
        }
    }
}
