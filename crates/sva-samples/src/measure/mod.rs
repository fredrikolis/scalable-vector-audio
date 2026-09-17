// Concern: groups the readings taken off samples, and declares the three things one can consume | Non-concern: which reading consumes which (sva-engine's query.rs) | IO: none

pub mod alias;
pub mod bands;
pub mod crest;
pub mod envelope;
pub mod formants;
pub mod ledger;
pub mod loudness;
pub mod pitch;
pub mod spectrum;
pub mod stereo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumes {
    ClosedForm,
    Buffer,
    Frames,
}
