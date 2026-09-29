// Concern: reads a lane or a written form onto the grid, one instant at a time | Non-concern: placing lines by transform (lines.rs), choosing this row (plan.rs) | IO: (a lane or a body, rate) -> a plane

use sva_formula::{Body, ClosedForm, SpectralSum};

use crate::error::CollapseError;

use super::active::{self, Window};
use super::{Extent, lines, plan, point, span};

pub(super) fn sampled_spectral_sum(
    sum: &SpectralSum,
    lanes: &[plan::LanePlan],
    rate: u32,
    extent: Extent,
    len: usize,
) -> Result<Vec<Vec<f64>>, CollapseError> {
    let step = 1.0 / f64::from(rate);
    let mut planes = Vec::with_capacity(sum.lanes.len());
    for (lane, taken) in sum.lanes.iter().zip(lanes) {
        if let plan::LanePlan::Grouped { groups, bins } = taken {
            planes.push(under_a_window(groups, *bins, rate, extent, len)?);
            continue;
        }
        let mut plane = vec![0.0; len];
        let windows = active::windows(lane, step);
        for (from, to) in span::nonzero(lane, extent, rate) {
            let over = (extent.start + from as i64, extent.start + to as i64);
            active::sweep(lane, &windows, over, step, &mut plane[from..to])?;
        }
        planes.push(plane);
    }
    Ok(planes)
}

/// Each group's lines are placed by FORMAT 9.2's routes and its factor is read once an
/// instant, not once a term.
fn under_a_window(
    groups: &[plan::Group],
    bins: usize,
    rate: u32,
    extent: Extent,
    len: usize,
) -> Result<Vec<f64>, CollapseError> {
    let mut plane = vec![0.0; len];
    let step = 1.0 / f64::from(rate);
    for group in groups {
        let mut held = lines::transformed(&group.placed, extent, bins, rate, len);
        let live = extent.intersect(Extent::new(group.live.0, group.live.1));
        let (from, to) = match live.is_empty() {
            true => (0, 0),
            false => (
                (live.start - extent.start) as usize,
                (live.end - extent.start) as usize,
            ),
        };
        lines::add_direct(&mut held[from..to], &group.summed, live, rate);
        for (at, value) in held.into_iter().enumerate().take(to).skip(from) {
            plane[at] += value * point::eval_atom(&group.factor, extent.instant(at, 1, step))?.re;
        }
    }
    Ok(plane)
}

pub(super) fn sampled_body(
    form: &ClosedForm,
    component: usize,
    rate: u32,
    extent: Extent,
    len: usize,
    scale: usize,
) -> Result<Vec<f64>, CollapseError> {
    let step = 1.0 / (f64::from(rate) * scale as f64);
    let from = extent.start * scale as i64;
    let windows =
        plan::summed(&form.body).map_or_else(Vec::new, |parts| plan::addend_windows(&parts, step));
    written(
        &form.body,
        &windows,
        component,
        (from, from + len as i64),
        step,
    )
}

/// Samples `[from, to)` of a grid `step` apart of one component of a written form. A sum
/// visits each addend only inside its window from `plan::addend_windows`.
pub(super) fn written(
    body: &Body,
    windows: &[Window],
    component: usize,
    (from, to): Window,
    step: f64,
) -> Result<Vec<f64>, CollapseError> {
    let at = |n: i64| n as f64 * step;
    let Some(parts) = plan::summed(body) else {
        return (from..to)
            .map(|n| Ok(point::eval_body(body, component, at(n), &point::NoRefs)?.re))
            .collect();
    };
    let mut out = vec![0.0; (to - from) as usize];
    active::sweep_by(windows, (from, to), &mut out, |n, live| {
        Ok(point::eval_addends(&parts, live, component, at(n))?.re)
    })?;
    Ok(out)
}
