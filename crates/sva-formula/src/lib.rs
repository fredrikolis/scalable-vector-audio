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
pub mod ty;

pub use calculus::{Envelope, analytic, d_dt, envelope, hilbert};
pub use closed_form::{
    Body, Bound, ClosedForm, Edge, Excitation, Fold, IndexId, ModalBank, Mode, Part, Rational,
    Series, Unary, Var,
};
pub use complex::C64;
pub use env::{Env, NodeId, ParamId};
pub use filter::{ALL_SHAPES, Shape, design};
pub use hash::{Hash, draw, hash_closed_form, hash_spectral_sum, keyed};
pub use infer::infer;
pub use lanes::Lanes;
pub use modal::damping::Damping;
pub use modal::{Geometry, modes};
pub use noise::noise;
pub use origin::Origin;
pub use refusal::{AtomSketch, Code, Factor, Left, LeftReason, Refusal};
pub use run::{Mirror, Run};
pub use series::{
    AUDIBLE_CEILING_HZ, Enumerated, Line, Lines, Rung, commensurate, line_atoms, lines, spacing,
    summable,
};
pub use spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
pub use spectral_sum::build::{normalize, normalize_closed_form};
pub use spectral_sum::image::crop_peeled;
pub use spectral_sum::{Lane, SpectralSum};
pub use table::{FAMILIES, TABLE_VERSION, dual, inverse, reflect};
pub use ty::{Codomain, Held, MAX_WIDTH, Mismatch, Ty};
