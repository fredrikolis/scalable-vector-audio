// Concern: duals the Gaussian family, completing the square | Non-concern: a Gaussian meeting an indicator, which left A | IO: (&SpectralAtom) -> Vec<SpectralAtom>

use std::f64::consts::{PI, TAU};

use crate::complex::C64;
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Singular, SpectralAtom};

/// Duals to a Gaussian of width `pi^2/a`, centred where the carrier put it, times the
/// Hermite lift.
pub(crate) fn row(a: &SpectralAtom) -> Vec<SpectralAtom> {
    let g = a.gauss.expect("the caller dispatched on a Gaussian");
    let alpha = a.exp.map_or(C64::ZERO, |e| C64::new(e.sigma, e.omega));
    let quadratic = C64::real(g.a);
    let linear = C64::real(2.0 * g.a * g.mu) + alpha;
    let nu = linear / quadratic.scale(2.0);

    let image_alpha = -C64::new(0.0, TAU) * nu;
    let width = PI * PI / g.a;
    let mu = image_alpha.re / (2.0 * width);
    let bulk = C64::new(
        g.a * (nu.re * nu.re - g.mu * g.mu),
        2.0 * g.a * nu.re * nu.im,
    )
    .exp();
    let amplitude = a.c * bulk * C64::real((PI / g.a).sqrt());

    let raised = hermite(a.poly, image_alpha, -2.0 * width);
    let lift = C64::new(0.0, 1.0 / TAU).powi(u32::from(a.poly));
    raised
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !c.is_zero())
        .map(|(k, c)| {
            SpectralAtom::new(
                amplitude * lift * c,
                Factors {
                    poly: u16::try_from(k).expect("a Hermite degree fits a u16"),
                    exp: Some(Exp::at(0.0, image_alpha.im)),
                    gauss: Some(Gauss { a: width, mu }),
                    ..Factors::NONE
                },
                Singular::Regular,
                a.origin,
            )
        })
        .collect()
}

/// `P_n` in `d^n/dx^n e^{g} = P_n e^{g}` for `g' = b0 + b1*x`, ascending coefficients.
fn hermite(n: u16, b0: C64, b1: f64) -> Vec<C64> {
    let mut p = vec![C64::ONE];
    for _ in 0..n {
        let mut next = vec![C64::ZERO; p.len() + 1];
        for (j, c) in p.iter().enumerate() {
            if j > 0 {
                next[j - 1] = next[j - 1] + c.scale(j as f64);
            }
            next[j] = next[j] + *c * b0;
            next[j + 1] = next[j + 1] + c.scale(b1);
        }
        p = next;
    }
    p
}
