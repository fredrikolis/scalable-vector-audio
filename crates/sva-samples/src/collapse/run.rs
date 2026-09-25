// Concern: sums one run of lines at an instant and bounds that sum's rounding | Non-concern: which lines form a run (sva-formula) | IO: (&Run, t) -> C64; (&Run) -> f64

use std::f64::consts::TAU;

use sva_formula::{C64, Mirror, Run};

/// Horner's rule in the rotor, whose turns are reduced from the exact product.
pub fn at(run: &Run, t: f64) -> C64 {
    let step = turns(run.step, t);
    let start = wrap(turns(run.offset, t) + wrap(run.first as f64 * step));
    let rotor = cis(start);
    let z = match (run.amps.len(), start.to_bits() == step.to_bits()) {
        (1, _) => C64::ONE,
        (_, true) => rotor,
        (_, false) => cis(step),
    };
    let up = rotor * horner(&run.amps, z);
    match &run.mirror {
        Mirror::None => up,
        Mirror::Conjugate => up + up.conj(),
        Mirror::Held(amps) => up + rotor.conj() * horner(amps, z.conj()),
    }
}

/// `|at(run, t) - S(t)|` at every finite `t`, given `sin` and `cos` within `2^-52`.
pub fn bound(run: &Run) -> f64 {
    let rotor = (32.0 + 12.0 * (run.first as f64).abs()) * U;
    let term = |j: usize| (rotor + j as f64 * STEP + (j + 1) as f64 * OPS).exp_m1();
    let ladder = |amps: &[C64]| -> (f64, f64) {
        amps.iter()
            .enumerate()
            .fold((0.0, 0.0), |(err, sum), (j, a)| {
                (err + a.abs() * term(j), sum + a.abs())
            })
    };
    let (up, up_sum) = ladder(&run.amps);
    let (down, down_sum) = match &run.mirror {
        Mirror::None => (0.0, 0.0),
        Mirror::Conjugate => (up, up_sum),
        Mirror::Held(amps) => ladder(amps),
    };
    let err = (1.0 + U) * (up + down) + U * (up_sum + down_sum);
    err * (1.0 + 4.0 * (run.len() + 2) as f64 * U)
}

const U: f64 = f64::EPSILON / 2.0;

const STEP: f64 = 24.0 * U;

/// Per Horner step, a complex product's `sqrt(5)` and a sum's one.
const OPS: f64 = (2.236_067_977_499_8 + 1.0) * U;

fn horner(amps: &[C64], z: C64) -> C64 {
    let mut rest = amps.iter().rev();
    let mut acc = rest.next().copied().unwrap_or(C64::ZERO);
    for a in rest {
        acc = acc * z + *a;
    }
    acc
}

fn turns(x: f64, t: f64) -> f64 {
    let p = x * t;
    let e = x.mul_add(t, -p);
    wrap(wrap(p) + wrap(e))
}

fn wrap(x: f64) -> f64 {
    x - x.round_ties_even()
}

fn cis(turns: f64) -> C64 {
    let (sin, cos) = (TAU * turns).sin_cos();
    C64::new(cos, sin)
}
