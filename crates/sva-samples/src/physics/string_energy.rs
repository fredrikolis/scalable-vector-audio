// Concern: a pinned stiff string's energy and its felt's press | Non-concern: stepping the grid (stiff_string.rs), the strike | IO: (&StringGrid) -> energy

use crate::physics::stiff_string::StringGrid;

const U: f64 = f64::EPSILON / 2.0;

pub(crate) fn gamma(n: f64) -> f64 {
    n * U / (1.0 - n * U)
}

fn slopes(y: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (0..n).map(move |j| pinned(y, n, j + 1) - pinned(y, n, j))
}

fn curvatures(y: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (1..n).map(move |j| pinned(y, n, j + 1) - 2.0 * y[j] + pinned(y, n, j - 1))
}

/// Node `j`, the agraffe pin at 0 read as zero; `y[n]` holds a rigid pin's zero or the bridge.
fn pinned(y: &[f64], _n: usize, j: usize) -> f64 {
    match j {
        0 => 0.0,
        j => y[j],
    }
}

/// `(j, k psi dt^2/(rho dx), R psi dt/(2 rho dx))`, `psi` node `j`'s share of the felt.
pub(crate) type Felt = (usize, f64, f64);

/// Spring and dashpot centred on `y^n`.
pub(crate) fn press(grid: &mut StringGrid, felt: &[Felt]) {
    for &(j, kappa, rho) in felt {
        grid.y_next[j] =
            (grid.y_next[j] + (rho - kappa / 2.0) * grid.y_prev[j]) / (1.0 + kappa / 2.0 + rho);
    }
}

/// `E = w (v'(I - A/2)v + y'K y- + (y'ky + y-'ky-)/2 + lambda^2 v_n^2/2)/2` joules, `k` the
/// felt's springs, `v_n` the bridge's step, and a bound over its rounding: each sum errs by at
/// most `gamma` of it taken over magnitudes.
pub(crate) fn energy(grid: &StringGrid, dt: f64, felt: &[Felt]) -> (f64, f64) {
    let (n, y, yp) = (grid.n, &grid.y_now, &grid.y_prev);
    let v: Vec<f64> = y.iter().zip(yp).map(|(a, b)| a - b).collect();
    let size: Vec<f64> = y.iter().zip(yp).map(|(a, b)| a.abs() + b.abs()).collect();
    let (y_abs, yp_abs): (Vec<f64>, Vec<f64>) =
        y.iter().zip(yp).map(|(a, b)| (a.abs(), b.abs())).unzip();
    let squares = v[1..n].iter().map(|x| x * x).sum::<f64>();
    let bent = slopes(&v, n).map(|x| x * x).sum::<f64>();
    let tension = slopes(y, n)
        .zip(slopes(yp, n))
        .map(|(a, b)| a * b)
        .sum::<f64>();
    let bending = curvatures(y, n)
        .zip(curvatures(yp, n))
        .map(|(a, b)| a * b)
        .sum::<f64>();
    let spring: f64 = felt
        .iter()
        .map(|&(j, kappa, _)| kappa * (y[j] * y[j] + yp[j] * yp[j]) / 2.0)
        .sum();
    let value = (1.0 - grid.damp_a / 2.0) * squares - grid.damp_b / 2.0 * bent
        + grid.courant_sq * tension
        + grid.stiff_sq * bending
        + spring
        + grid.courant_sq / 2.0 * v[n] * v[n];
    let spread = (1.0 + grid.damp_a / 2.0) * size[1..n].iter().map(|x| x * x).sum::<f64>()
        + grid.damp_b / 2.0 * sums(&size, n).map(|x| x * x).sum::<f64>()
        + grid.courant_sq
            * sums(&y_abs, n)
                .zip(sums(&yp_abs, n))
                .map(|(a, b)| a * b)
                .sum::<f64>()
        + grid.stiff_sq
            * bends(&y_abs, n)
                .zip(bends(&yp_abs, n))
                .map(|(a, b)| a * b)
                .sum::<f64>()
        + spring
        + grid.courant_sq / 2.0 * size[n] * size[n];
    let w = grid.rho * grid.dx / (dt * dt) / 2.0;
    let joules = w * value;
    let slack = w * spread * gamma(2.0 * n as f64 + 96.0);
    (joules, (joules + slack) * (1.0 + 4.0 * U))
}

/// [`slopes`] and [`curvatures`] over magnitudes, every difference taken as a sum.
fn sums(m: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (0..n).map(move |j| pinned(m, n, j + 1) + pinned(m, n, j))
}

fn bends(m: &[f64], n: usize) -> impl Iterator<Item = f64> + '_ {
    (1..n).map(move |j| pinned(m, n, j + 1) + 2.0 * m[j] + pinned(m, n, j - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::twofold::Twofold;

    /// A stiff string bent smoothly and far, so each curvature cancels all but a sliver.
    fn bent_far(n: usize) -> StringGrid {
        let shape = |j: usize, phase: f64| {
            let x = std::f64::consts::PI * j as f64 / n as f64;
            1e3 * (x.sin() * phase.cos() + (3.0 * x).sin() * phase.sin() / 7.0)
        };
        let mut y_now: Vec<f64> = (0..=n).map(|j| shape(j, 0.3)).collect();
        let mut y_prev: Vec<f64> = (0..=n).map(|j| shape(j, 0.31)).collect();
        for y in [&mut y_now, &mut y_prev] {
            y[0] = 0.0;
            y[n] = 0.0;
        }
        StringGrid {
            y_next: vec![0.0; n + 1],
            y_now,
            y_prev,
            n,
            dx: 0.01,
            rho: 8.9e-3,
            courant_sq: 1e-4,
            stiff_sq: 0.24,
            damp_a: 1e-5,
            damp_b: 1e-4,
        }
    }

    fn exact(grid: &StringGrid, dt: f64, felt: &[Felt]) -> f64 {
        let t = Twofold::of;
        let (n, y, yp) = (grid.n, &grid.y_now, &grid.y_prev);
        let at = |z: &[f64], j: usize| t(pinned(z, n, j));
        let v = |j: usize| at(y, j).sub(at(yp, j));
        let bend = |z: &[f64], j: usize| at(z, j + 1).sub(t(2.0).mul(t(z[j]))).add(at(z, j - 1));
        let mut value = t(0.0);
        for j in 1..n {
            let own = t(1.0).sub(t(grid.damp_a).div(t(2.0)));
            value = value.add(own.mul(v(j)).mul(v(j)));
            let curved = bend(y, j).mul(bend(yp, j));
            value = value.add(t(grid.stiff_sq).mul(curved));
        }
        for j in 0..n {
            let dv = v(j + 1).sub(v(j));
            value = value.sub(t(grid.damp_b).div(t(2.0)).mul(dv).mul(dv));
            let slope = at(y, j + 1).sub(at(y, j)).mul(at(yp, j + 1).sub(at(yp, j)));
            value = value.add(t(grid.courant_sq).mul(slope));
        }
        for &(j, kappa, _) in felt {
            let pair = t(y[j]).mul(t(y[j])).add(t(yp[j]).mul(t(yp[j])));
            value = value.add(t(kappa).mul(pair).div(t(2.0)));
        }
        let w = t(grid.rho)
            .mul(t(grid.dx))
            .div(t(dt).mul(t(dt)))
            .div(t(2.0));
        w.mul(value).value()
    }

    #[test]
    fn a_string_energy_bound_covers_its_rounding_where_every_curvature_cancels() {
        let dt = 1.0 / 44_100.0;
        for n in [40, 400, 1600] {
            let grid = bent_far(n);
            let felt = [(n / 7, 3e-3, 0.0), (n / 7 + 1, 3e-3, 0.0)];
            for felt in [&[][..], &felt[..]] {
                let (joules, upper) = energy(&grid, dt, felt);
                let truth = exact(&grid, dt, felt);
                assert!(
                    (truth - joules).abs() <= upper - joules,
                    "n {n}: E {truth}, computed {joules}, bound {upper}"
                );
            }
        }
    }
}
