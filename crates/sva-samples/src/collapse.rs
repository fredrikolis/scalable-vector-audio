// Concern: the collapse surface: a form's rows and its value at an instant | Non-concern: its spectral sum (sva-formula), measuring samples (measure/) | IO: (&ClosedForm, grid) -> Rows

mod active;
mod addends;
mod atoms;
mod blocks;
mod column;
mod lines;
mod point;
pub mod run;
mod tail;
mod truncate;

use sva_formula::{Body, C64, SpectralSum};

use crate::error::CollapseError;

pub use blocks::Rows;
pub(crate) use column::Program;
pub use lines::summed_bounds;
pub use point::{At, crop_gain, lane_of, shoulders, unary};
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

/// One component of a written form, evaluated at any instant; a node it reads refuses.
pub struct Evaluator(Program);

impl Evaluator {
    pub fn of(body: &Body, component: usize) -> Evaluator {
        Evaluator(Program::of(&[body], &[], component))
    }

    pub fn at(&self, at: At) -> Result<C64, CollapseError> {
        let mut columns = self.0.columns(1);
        columns.t[0] = at.t();
        if let At::Sample(grid, n) = at {
            columns.grid = grid;
            columns.on[0] = Some(n);
        }
        columns.root(0, (0, 1)).get(0).map_err(Clone::clone)
    }
}
