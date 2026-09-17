// Concern: builds one deterministic noise realization as a conjugate-symmetric line series | Non-concern: line enumeration (series.rs) | IO: (seed, period, color) -> Series

use std::f64::consts::TAU;

use crate::closed_form::{Body, Bound, IndexId, Part, Series, Unary};
use crate::complex::C64;

pub const LINE: IndexId = IndexId(u32::MAX);

pub fn noise(seed: u64, period: f64, color_db_per_octave: f64) -> Series {
    let amplitude = Body::Apply(
        Unary::Exp,
        Part::bare(Body::Mul(vec![
            Part::bare(Body::Const(C64::real(exponent(color_db_per_octave)))),
            Part::bare(Body::Apply(Unary::Log, Part::bare(Body::Index(LINE)))),
        ])),
    );
    let phase = Body::Mul(vec![
        Part::bare(Body::Const(C64::real(TAU))),
        Part::bare(Body::Keyed {
            seed,
            of: Part::bare(Body::Index(LINE)),
        }),
    ]);
    let turning = Body::Mul(vec![
        Part::bare(Body::Const(C64::real(TAU / period))),
        Part::bare(Body::Index(LINE)),
        Part::bare(Body::Line),
    ]);
    Series {
        index: LINE,
        lo: 1,
        hi: Bound::Infinite,
        term: Part::bare(Body::Mul(vec![
            Part::bare(amplitude),
            Part::bare(Body::Apply(
                Unary::Cos,
                Part::bare(Body::Add(vec![Part::bare(turning), Part::bare(phase)])),
            )),
        ])),
    }
}

/// `10^(color * log2(k) / 20)` is `k^e`: a power of the index, so the series is summable.
fn exponent(color_db_per_octave: f64) -> f64 {
    color_db_per_octave / (20.0 * 2f64.log10())
}
