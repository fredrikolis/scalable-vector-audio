// Concern: sorts a lane, merges like atoms and folds exact zeros | Non-concern: what makes two atoms alike (atom.rs) | IO: (&mut Lane) -> ()

use crate::closed_form::Edge;
use crate::complex::C64;
use crate::spectral_sum::Lane;
use crate::spectral_sum::atom::{Factors, Indicator, SpectralAtom};

/// Exact bits only. An epsilon would make equality intransitive, which breaks both the sort
/// and the hash, and the hash is the cache key.
pub fn simplify(lane: &mut Lane) {
    collect(lane);
    let folded = fold_windows(std::mem::take(&mut lane.atoms));
    lane.atoms = folded;
    collect(lane);
}

fn collect(lane: &mut Lane) {
    lane.atoms.sort_by_key(SpectralAtom::key);
    let mut merged: Vec<SpectralAtom> = Vec::with_capacity(lane.atoms.len());
    for atom in lane.atoms.drain(..) {
        match merged.last_mut() {
            Some(last) if last.key() == atom.key() => last.c = last.c + atom.c,
            _ => merged.push(atom),
        }
    }
    merged.retain(|a| !a.c.is_zero());
    lane.atoms = merged;
}

/// Atoms alike but for their window are one piecewise-constant amplitude over the line.
/// Summing it per interval is what turns the two halves of an `sgn` back into one window.
fn fold_windows(atoms: Vec<SpectralAtom>) -> Vec<SpectralAtom> {
    let mut ordered = atoms;
    ordered.sort_by_key(SpectralAtom::key_without_window);
    let mut out = Vec::with_capacity(ordered.len());
    let mut run: Vec<SpectralAtom> = Vec::new();
    for atom in ordered {
        let breaks = run
            .first()
            .is_some_and(|f| f.key_without_window() != atom.key_without_window());
        if breaks {
            out.extend(sweep(std::mem::take(&mut run)));
        }
        run.push(atom);
    }
    out.extend(sweep(run));
    out
}

fn sweep(run: Vec<SpectralAtom>) -> Vec<SpectralAtom> {
    if run.len() < 2 || run.iter().any(SpectralAtom::is_delta) {
        return run;
    }
    let mut cuts: Vec<f64> = vec![f64::NEG_INFINITY, f64::INFINITY];
    for atom in &run {
        let window = atom.ind.unwrap_or(Indicator::ALL);
        cuts.push(window.l.value());
        cuts.push(window.r.value());
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();

    let template = run[0];
    let factors = Factors {
        ind: None,
        ..template.factors()
    };
    let mut out: Vec<SpectralAtom> = Vec::new();
    let mut open: Option<(f64, C64)> = None;
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let height = run
            .iter()
            .filter(|a| {
                let w = a.ind.unwrap_or(Indicator::ALL);
                w.l.value() <= from && to <= w.r.value()
            })
            .fold(C64::ZERO, |acc, a| acc + a.c);
        match open {
            Some((_, standing)) if standing == height => continue,
            Some((start, standing)) => out.push(piece(&template, factors, standing, start, from)),
            None => {}
        }
        open = Some((from, height));
    }
    if let Some((start, standing)) = open {
        out.push(piece(&template, factors, standing, start, f64::INFINITY));
    }
    out.retain(|a| !a.c.is_zero());
    out
}

fn piece(
    template: &SpectralAtom,
    factors: Factors,
    height: C64,
    from: f64,
    to: f64,
) -> SpectralAtom {
    SpectralAtom::new(
        height,
        Factors {
            ind: Some(Indicator {
                l: Edge::at(from),
                r: Edge::at(to),
            }),
            ..factors
        },
        template.sing,
        template.origin,
    )
}
