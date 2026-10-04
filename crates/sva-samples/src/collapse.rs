// Concern: the collapse surface: a form's rows and its value at an instant | Non-concern: its spectral sum (sva-formula), measuring samples (measure/) | IO: (&ClosedForm, grid) -> Rows

mod active;
mod addends;
mod atoms;
mod blocks;
mod lines;
mod point;
mod reading;
pub mod run;
mod tail;
mod truncate;

use sva_formula::{Body, C64, SpectralSum};

use crate::error::CollapseError;

pub use blocks::Rows;
pub use lines::summed_bounds;
pub use point::{At, Refs, crop_gain, lane_of, shoulders, unary};
pub(crate) use point::{Shared, terms};
pub use truncate::{
    Audible, dropped_db, spectral_sum as truncate_spectral_sum,
    spectral_sum_read as truncate_spectral_sum_read, written as truncate_written,
    written_with as truncate_written_with,
};

pub fn eval_spectral_sum_at(
    sum: &SpectralSum,
    component: usize,
    at: At,
) -> Result<C64, CollapseError> {
    point::eval_spectral_sum_on(sum, component, at)
}

/// Every `Body::Node` in `body` answered by `refs`.
pub fn eval_written_at(
    body: &Body,
    component: usize,
    at: At,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    point::eval_body_on(body, component, at, refs)
}
