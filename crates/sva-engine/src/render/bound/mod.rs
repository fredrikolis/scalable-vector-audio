// Concern: bounds every node's magnitude from each instant on, no sample rendered | Non-concern: where a node is cut by it (cut/) | IO: (NodeId, instants) -> bounds, or the class none is derived for

mod envelope;
mod filtered;
mod floor;
mod looped;
mod range;
mod ringing;
mod solver;

pub(crate) use solver::{Played, hear};

pub(crate) use envelope::{Bounds, Forms, Grid, Reach, STEP};
pub(crate) use ringing::Ringing;
