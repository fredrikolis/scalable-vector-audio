// Concern: reads a written form onto the grid an instant at a time, each addend inside its window | Non-concern: one instant's value (point.rs) | IO: (a body, span, grid) -> a plane

use sva_formula::Body;

use crate::error::CollapseError;
use crate::grid::Grid;

use super::active::{self, Window};
use super::{addends, point};

/// Samples `[from, to)` of `grid` of one component of a written form. A sum
/// visits each addend only inside its window from `addends::addend_windows`.
pub(super) fn written(
    body: &Body,
    windows: &[Window],
    component: usize,
    (from, to): Window,
    grid: Grid,
) -> Result<Vec<f64>, CollapseError> {
    let Some(parts) = addends::summed(body) else {
        return (from..to)
            .map(|n| {
                let at = point::At::Sample(grid, n);
                Ok(point::eval_body_on(body, component, at, &point::NoRefs)?.re)
            })
            .collect();
    };
    let mut out = vec![0.0; (to - from) as usize];
    active::sweep_by(windows, (from, to), &mut out, |n, live| {
        Ok(point::eval_addends(&parts, live, component, (grid, n))?.re)
    })?;
    Ok(out)
}
