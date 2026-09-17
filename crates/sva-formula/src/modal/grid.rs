// Concern: how far a multi-index geometry must be walked to hold its lowest modes | Non-concern: what any index means to a geometry (the sibling files) | IO: (lengths, count) -> per-axis reach

use crate::closed_form::Mode;

/// Taking one axis alone already gives `count` modes at or below its own `count`-th, so no
/// mode of the whole grid below that frequency has an index past this reach.
pub(crate) fn reach(lengths: &[f64], count: usize) -> Vec<usize> {
    let longest = lengths.iter().copied().fold(0.0f64, f64::max);
    lengths
        .iter()
        .map(|l| ((count as f64) * l / longest).ceil() as usize + 1)
        .collect()
}

pub(crate) fn lowest(mut found: Vec<Mode>, count: usize) -> Vec<Mode> {
    found.sort_by(|a, b| a.omega.total_cmp(&b.omega));
    found.truncate(count);
    found
}
