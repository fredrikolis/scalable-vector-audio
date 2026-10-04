// Concern: the builders and readings every suite here shares | Non-concern: what any suite asserts | IO: (a form, a seed) -> a buffer, a draw

#![allow(dead_code)]

pub mod fd;
pub mod partials;

use sva_formula::{Body, ClosedForm, Part};
use sva_samples::{Buffer, CollapseError, Extent, Label, Profile, Rows};

/// The form's rows over `secs`, and the label they state.
pub fn render(
    form: &ClosedForm,
    rate: u32,
    secs: (f64, f64),
    profile: &Profile,
) -> Result<(Buffer, Label), CollapseError> {
    let extent = Extent::secs(rate, secs.0, secs.1);
    let rows = Rows::of(form, sva_samples::Grid::of(rate), profile)?;
    let mut buffer = Buffer::of_planes(rate, rows.planes(extent.start, extent.end)?);
    buffer.start = extent.start;
    Ok((buffer, rows.label(profile)))
}

pub fn part(body: Body) -> Part {
    Part::bare(body)
}

pub fn whole_second() -> (f64, f64) {
    (0.0, 1.0)
}

/// A keyed hash, so a run is deterministic without depending on an evaluator.
pub fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

pub fn drawn(seed: f64, index: i64) -> f64 {
    let bits = splitmix64((index as u64) ^ splitmix64(seed.to_bits()));
    (bits >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}
