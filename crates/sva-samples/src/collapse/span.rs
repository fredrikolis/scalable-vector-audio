// Concern: the sample spans a lane's own windows leave it nonzero over | Non-concern: reading one (reading.rs), pricing them (plan.rs) | IO: (&Lane, rate, Horizon) -> spans

use sva_formula::Lane;

use super::Horizon;

/// Where every atom is windowed, the lane is zero outside their union.
pub(super) fn nonzero(lane: &Lane, horizon: Horizon, rate: u32, len: usize) -> Vec<(usize, usize)> {
    if !lane.is_finite_sum() || lane.atoms.iter().any(|a| a.ind.is_none()) {
        return vec![(0, len)];
    }
    let edge = |secs: f64| {
        (((secs - horizon.start_secs) * f64::from(rate)).ceil()).clamp(0.0, len as f64) as usize
    };
    let mut spans: Vec<(usize, usize)> = lane
        .atoms
        .iter()
        .filter_map(|a| {
            let window = a.ind?;
            let (from, to) = (edge(window.l.value()), edge(window.r.value()));
            (from < to).then_some((from, to))
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (from, to) in spans {
        match merged.last_mut() {
            Some(held) if from <= held.1 => held.1 = held.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    merged
}
