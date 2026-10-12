// Concern: sums one run of lines at an instant and bounds that sum's rounding | Non-concern: which lines form a run (sva-formula) | IO: (&Run, t) -> C64; (&Run) -> f64

use std::f64::consts::TAU;

use sva_formula::{C64, Mirror, Run};

/// Horner's rule in the rotor, whose turns are reduced from the exact product.
pub fn at(run: &Run, t: f64) -> C64 {
    let mut out = [C64::ZERO];
    each(run, &[t], &mut out);
    out[0]
}

/// `at` at each of `t`: every instant's sum takes the operations `at` does, in its order.
pub fn each(run: &Run, t: &[f64], out: &mut [C64]) {
    each_at(Width::widest(), run, t, out);
}

pub fn each_at(width: Width, run: &Run, t: &[f64], out: &mut [C64]) {
    for (t, out) in t.chunks(LANES).zip(out.chunks_mut(LANES)) {
        side_by_side(width, run, t, out);
    }
}

/// Instants a ladder instruction steps: an instant's IEEE operations and their order are the
/// same at every width, none fused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Width(Kind);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// SSE2 on x86-64, SIMD128 on wasm32.
    Built,
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

impl Width {
    pub const BUILT: Width = Width(Kind::Built);

    pub fn widest() -> Width {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2") {
            return Width(Kind::Avx2);
        }
        Width::BUILT
    }

    pub fn all() -> Vec<Width> {
        let mut all = vec![Width::BUILT];
        if Width::widest() != Width::BUILT {
            all.push(Width::widest());
        }
        all
    }
}

/// Instants summed side by side: one instant's sum waits on its own last product alone.
const LANES: usize = 64;

fn side_by_side(width: Width, run: &Run, t: &[f64], out: &mut [C64]) {
    let n = t.len();
    let (mut rotor, mut z) = ([C64::ZERO; LANES], [C64::ZERO; LANES]);
    for ((rotor, z), t) in rotor.iter_mut().zip(&mut z).zip(t) {
        let step = turns(run.step, *t);
        let start = wrap(turns(run.offset, *t) + wrap(run.first as f64 * step));
        *rotor = cis(start);
        *z = match (run.amps.len(), start.to_bits() == step.to_bits()) {
            (1, _) => C64::ONE,
            (_, true) => *rotor,
            (_, false) => cis(step),
        };
    }
    let mut up = [C64::ZERO; LANES];
    horner(width, &run.amps, &z[..n], &mut up[..n]);
    let mut down = [C64::ZERO; LANES];
    if let Mirror::Held(amps) = &run.mirror {
        z.iter_mut().for_each(|z| *z = z.conj());
        horner(width, amps, &z[..n], &mut down[..n]);
    }
    for (i, out) in out.iter_mut().enumerate() {
        let up = rotor[i] * up[i];
        *out = match &run.mirror {
            Mirror::None => up,
            Mirror::Conjugate => up + up.conj(),
            Mirror::Held(_) => up + rotor[i].conj() * down[i],
        };
    }
}

/// A conjugate mirror takes none of its own.
pub fn steps(run: &Run) -> usize {
    match &run.mirror {
        Mirror::Held(amps) => run.amps.len() + amps.len(),
        Mirror::None | Mirror::Conjugate => run.amps.len(),
    }
}

/// `|at(run, t) - S(t)|` at every finite `t`, given `sin` and `cos` within `2^-52`.
pub fn bound(run: &Run) -> f64 {
    let rotor = (32.0 + 12.0 * (run.first as f64).abs()) * U;
    let term =
        |j: usize| (rotor + j as f64 * STEP_ROTOR + (j + 1) as f64 * PRODUCT_AND_SUM).exp_m1();
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

/// The largest magnitude the exact sum reaches, rounded up.
pub fn reach(run: &Run) -> f64 {
    let sum = |amps: &[C64]| amps.iter().map(|a| a.abs()).sum::<f64>();
    let mirrored = match &run.mirror {
        Mirror::None => 0.0,
        Mirror::Conjugate => sum(&run.amps),
        Mirror::Held(amps) => sum(amps),
    };
    (sum(&run.amps) + mirrored) * (1.0 + 2.0 * (run.len() + 1) as f64 * U)
}

const U: f64 = f64::EPSILON / 2.0;

const STEP_ROTOR: f64 = 24.0 * U;

const PRODUCT_AND_SUM: f64 = (2.236_067_977_499_8 + 1.0) * U;

fn horner(width: Width, amps: &[C64], z: &[C64], out: &mut [C64]) {
    match width.0 {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: `widest` builds this kind once the CPU reports AVX2.
        Kind::Avx2 => unsafe { ladder_avx2(amps, z, out) },
        Kind::Built => ladder(amps, z, out),
    }
}

/// FMA stays off, so no product and sum can fuse.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn ladder_avx2(amps: &[C64], z: &[C64], out: &mut [C64]) {
    ladder(amps, z, out);
}

#[inline(always)]
fn ladder(amps: &[C64], z: &[C64], out: &mut [C64]) {
    let n = z.len();
    let mut rest = amps.iter().rev();
    let last = rest.next().copied().unwrap_or(C64::ZERO);
    let (mut zr, mut zi) = ([0.0; LANES], [0.0; LANES]);
    for ((zr, zi), z) in zr.iter_mut().zip(&mut zi).zip(z) {
        (*zr, *zi) = (z.re, z.im);
    }
    let (mut re, mut im) = ([last.re; LANES], [last.im; LANES]);
    for a in rest {
        let lanes = re[..n].iter_mut().zip(&mut im[..n]);
        for ((re, im), (zr, zi)) in lanes.zip(zr[..n].iter().zip(&zi[..n])) {
            let (r, i) = (*re, *im);
            *re = (r * zr - i * zi) + a.re;
            *im = (r * zi + i * zr) + a.im;
        }
    }
    for ((out, re), im) in out.iter_mut().zip(re).zip(im) {
        *out = C64::new(re, im);
    }
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
