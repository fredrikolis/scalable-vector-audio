// Concern: bounds every node's magnitude from each instant on, no sample rendered | Non-concern: where a node is cut by it (cut/) | IO: (NodeId) -> Envelope, or the class no bound is derived for

mod envelope;
mod floor;
mod range;
mod ringing;

pub(crate) use envelope::{Bounds, Envelope, Forms, Found, Grid, STEP};
pub(crate) use ringing::Ringing;
