// Concern: reads a lane or a written form onto the grid, one instant at a time | Non-concern: placing lines by transform (lines.rs), choosing this row (plan.rs) | IO: (a lane or a body, rate) -> a plane

use sva_formula::{ClosedForm, SpectralSum};

use crate::error::CollapseError;

use super::{Horizon, lines, plan, point, span};

pub(super) fn sampled_spectral_sum(
    sum: &SpectralSum,
    lanes: &[plan::LanePlan],
    rate: u32,
    horizon: Horizon,
    len: usize,
) -> Result<Vec<Vec<f64>>, CollapseError> {
    let step = 1.0 / f64::from(rate);
    let mut planes = Vec::with_capacity(sum.lanes.len());
    for (lane, taken) in sum.lanes.iter().zip(lanes) {
        if let plan::LanePlan::Grouped { groups, bins } = taken {
            planes.push(under_a_window(groups, *bins, rate, horizon, len)?);
            continue;
        }
        let mut plane = vec![0.0; len];
        for (from, to) in span::nonzero(lane, horizon, rate, len) {
            for (at, held) in plane.iter_mut().enumerate().take(to).skip(from) {
                *held = point::eval_lane(lane, horizon.start_secs + at as f64 * step)?.re;
            }
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
    horizon: Horizon,
    len: usize,
) -> Result<Vec<f64>, CollapseError> {
    let mut plane = vec![0.0; len];
    let step = 1.0 / f64::from(rate);
    for group in groups {
        let mut held = lines::transformed(&group.placed, horizon.start_secs, bins, rate, len);
        lines::add_direct(&mut held, &group.summed, horizon.start_secs, rate);
        for (at, value) in held.into_iter().enumerate() {
            plane[at] +=
                value * point::eval_atom(&group.factor, horizon.start_secs + at as f64 * step)?.re;
        }
    }
    Ok(plane)
}

pub(super) fn sampled_body(
    form: &ClosedForm,
    component: usize,
    rate: u32,
    horizon: Horizon,
    len: usize,
    scale: f64,
) -> Result<Vec<f64>, CollapseError> {
    let step = 1.0 / (f64::from(rate) * scale);
    (0..len)
        .map(|i| {
            point::eval_body(
                &form.body,
                component,
                horizon.start_secs + i as f64 * step,
                &point::NoRefs,
            )
            .map(|v| v.re)
        })
        .collect()
}
