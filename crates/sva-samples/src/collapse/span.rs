// Concern: the sample spans a lane's own windows leave it nonzero over | Non-concern: reading one (reading.rs), pricing them (plan.rs) | IO: (&Lane, rate, Extent) -> spans

use sva_formula::Lane;

use super::Extent;
use crate::Grid;

/// Where every atom is windowed, the lane is zero outside their union.
pub(super) fn nonzero(lane: &Lane, extent: Extent, rate: u32) -> Vec<(usize, usize)> {
    let local = |n: i64| (n - extent.start) as usize;
    absolute(lane, extent, rate)
        .into_iter()
        .map(|(from, to)| (local(from), local(to)))
        .collect()
}

/// The same spans on the grid's own index.
pub(super) fn absolute(lane: &Lane, extent: Extent, rate: u32) -> Vec<(i64, i64)> {
    let Some(spans) = windows(lane, rate) else {
        return vec![(extent.start, extent.end)];
    };
    let within = |n: i64| n.clamp(extent.start, extent.end);
    spans
        .into_iter()
        .map(|(from, to)| (within(from), within(to)))
        .filter(|(from, to)| from < to)
        .collect()
}

pub(super) fn windows(lane: &Lane, rate: u32) -> Option<Vec<(i64, i64)>> {
    if !lane.is_finite_sum() || lane.atoms.iter().any(|a| a.ind.is_none()) {
        return None;
    }
    let grid = Grid::of(rate);
    let mut spans: Vec<(i64, i64)> = lane
        .atoms
        .iter()
        .filter_map(|a| {
            let window = a.ind?;
            let (from, to) = (
                grid.first_at(window.l.value()),
                grid.first_at(window.r.value()),
            );
            (from < to).then_some((from, to))
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::with_capacity(spans.len());
    for (from, to) in spans {
        match merged.last_mut() {
            Some(held) if from <= held.1 => held.1 = held.1.max(to),
            _ => merged.push((from, to)),
        }
    }
    Some(merged)
}
