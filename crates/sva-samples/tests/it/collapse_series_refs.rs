// Concern: proves a series over a chain of refs is truncated reading each node per term, not per path | Non-concern: the samples it writes | IO: (a chain's depth) -> form lookups

use std::cell::Cell;
use std::f64::consts::TAU;

use sva_formula::{
    Body, Bound, C64, Edge, IndexId, Kept, Lane, NodeId, Origin, Part, Series, SpectralSum,
    Through, Unary, Var,
};
use sva_samples::{Audible, PSYCHOACOUSTIC_V1, truncate_spectral_sum_read};

fn bare(body: Body) -> Part {
    Part::bare(body)
}

fn cropped(of: Body) -> Body {
    Body::Crop {
        of: bare(of),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    }
}

/// `n0` a cropped tone, each `nk` three weighted reads of `n{k-1}`.
fn chain(depth: usize) -> Vec<Body> {
    let tone = Body::Apply(
        Unary::Sin,
        bare(Body::Mul(vec![
            bare(Body::Const(C64::real(TAU * 220.0))),
            bare(Body::Line),
        ])),
    );
    let mut forms = vec![cropped(tone)];
    for k in 1..=depth {
        let read = |w: f64| {
            bare(Body::Mul(vec![
                bare(Body::Node(NodeId(k as u32 - 1))),
                bare(Body::Const(C64::real(w))),
            ]))
        };
        forms.push(cropped(Body::Add(vec![read(0.5), read(0.3), read(0.2)])));
    }
    forms
}

/// `sum(k, 0, inf, 0.5^k * n_depth(t - k*17ms))`, as a closed loop over the chain writes it.
fn looped(depth: usize) -> SpectralSum {
    let index = IndexId(0);
    let at = Body::Add(vec![
        bare(Body::Line),
        bare(Body::Mul(vec![
            bare(Body::Const(C64::real(-0.017))),
            bare(Body::Index(index)),
        ])),
    ]);
    let power = Body::Apply(
        Unary::Exp,
        bare(Body::Mul(vec![
            bare(Body::Index(index)),
            bare(Body::Const(C64::real(0.5f64.ln()))),
        ])),
    );
    let term = Body::Mul(vec![
        bare(power),
        bare(Body::Warp {
            at: bare(at),
            of: bare(Body::Node(NodeId(depth as u32))),
        }),
    ]);
    let series = Series {
        index,
        lo: 0,
        hi: Bound::Infinite,
        term: bare(term),
    };
    SpectralSum::of(
        Var::T,
        vec![Lane {
            series: vec![series],
            ..Lane::default()
        }],
    )
}

fn lookups(depth: usize) -> (usize, usize) {
    let forms = chain(depth);
    let asked = Cell::new(0usize);
    let written = |id: NodeId| {
        asked.set(asked.get() + 1);
        forms.get(id.0 as usize).map(|f| (f, Origin::UNKNOWN))
    };
    let kept = Kept::default();
    let through = Through::new(&written, &kept);
    let band = Audible::of(&PSYCHOACOUSTIC_V1, 8_000);
    let truncated =
        truncate_spectral_sum_read(&looped(depth), band, &through).expect("a truncation");
    (asked.get(), truncated.atoms().count())
}

/// A reading per path would ask `3^6` times as often at twice the depth.
#[test]
fn a_series_over_a_chain_of_refs_reads_each_node_per_term_not_per_path() {
    let (shallow, atoms) = lookups(6);
    let (deep, deep_atoms) = lookups(12);
    assert!(
        atoms > 0 && atoms == deep_atoms,
        "{atoms} atoms, then {deep_atoms}"
    );
    assert!(
        deep <= 2 * shallow,
        "depth 12 asked {deep} times, depth 6 {shallow}"
    );
}
