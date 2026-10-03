// Concern: closed forms in t and f, their spectral sum, typing and transform | Non-concern: buffers and rates (sva-samples), the graph (sva-engine) | IO: (&ClosedForm, &Env) -> Ty, SpectralSum, Hash

pub mod affine;
pub mod alg;
pub mod calculus;
pub mod closed_form;
pub mod complex;
pub mod env;
pub mod filter;
pub mod hash;
pub mod infer;
mod lanes;
pub mod modal;
pub mod noise;
pub mod note;
pub mod origin;
pub mod rational;
pub mod refusal;
pub mod run;
pub mod series;
pub mod spectral_sum;
pub mod table;
pub mod through;
pub mod ty;
pub mod underflow;

pub use calculus::{Envelope, analytic, d_dt, envelope, envelope_read, hilbert};
pub use closed_form::{
    Body, Bound, ClosedForm, Edge, Excitation, Fold, IndexId, ModalBank, Mode, Part, Rational,
    Series, Unary, Var,
};
pub use complex::C64;
pub use env::{Env, NodeId, ParamId};
pub use filter::{ALL_SHAPES, Shape, design};
pub use hash::{
    Hash, draw, draw_nearest, hash_closed_form, hash_closed_form_with, hash_spectral_sum,
    hash_spectral_sum_with,
};
pub use infer::infer;
pub use lanes::Lanes;
pub use modal::damping::Damping;
pub use modal::{Geometry, modes};
pub use noise::noise;
pub use origin::Origin;
pub use refusal::{AtomSketch, Code, Factor, Left, LeftReason, Refusal};
pub use run::{Mirror, Run};
pub use series::{
    Enumerated, Line, Lines, Rung, commensurate, line_atoms, line_atoms_read, lines, lines_read,
    spacing, spacing_read, summable,
};
pub use spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
pub use spectral_sum::build::{normalize, normalize_closed_form, normalize_read};
pub use spectral_sum::image::crop_peeled;
pub use spectral_sum::{Lane, SpectralSum};
pub use table::{FAMILIES, TABLE_VERSION, dual, dual_read, inverse, inverse_read, reflect};
pub use through::{Kept, Opaque, Reads, Through};
pub use ty::{Codomain, Held, MAX_WIDTH, Mismatch, Ty};
pub use underflow::exp_zero_at;
