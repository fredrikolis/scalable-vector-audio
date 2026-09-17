// Concern: declares this crate's refusal codes and Left, the atom that left A | Non-concern: rendering a diagnostic block (sva-engine) | IO: none

use crate::origin::Origin;
use crate::ty::{Mismatch, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    DomainMismatch,
    SamplesInClosedForm,
    RateConflict,
    FrameMismatch,
    WidthMismatch,
    NonAffineSingular,
    SeriesNotSummable,
    LeftAlgebra,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::DomainMismatch => "type.domain_mismatch",
            Code::SamplesInClosedForm => "type.samples_in_closed_form",
            Code::RateConflict => "type.rate_conflict",
            Code::FrameMismatch => "type.frame_mismatch",
            Code::WidthMismatch => "type.width_mismatch",
            Code::NonAffineSingular => "type.non_affine_singular",
            Code::SeriesNotSummable => "type.series_not_summable",
            Code::LeftAlgebra => "cast.left_algebra",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: Code,
    pub origin: Origin,
    pub message: String,
}

impl Refusal {
    pub fn new(code: Code, origin: Origin, message: impl Into<String>) -> Refusal {
        Refusal {
            code,
            origin,
            message: message.into(),
        }
    }
}

impl Refusal {
    /// The text for two operand types that have no common type, one repair per side.
    pub fn of_mismatch(m: Mismatch, origin: Origin, a: Ty, b: Ty) -> Refusal {
        let (code, message) = match m {
            Mismatch::Domain => (
                Code::DomainMismatch,
                "a closed form in t meets one in f. write fourier on the t side, or ifourier on \
                 the f side"
                    .to_string(),
            ),
            Mismatch::SamplesInClosedForm => (
                Code::SamplesInClosedForm,
                "this mixes a closed form and samples. write sample(...) on the closed form"
                    .to_string(),
            ),
            Mismatch::Rate => (
                Code::RateConflict,
                format!(
                    "{:?} Hz meets {:?} Hz; one rate per expression",
                    a.rate, b.rate
                ),
            ),
            Mismatch::Frames => (
                Code::FrameMismatch,
                "two frame layouts meet in one expression".to_string(),
            ),
            Mismatch::Width => (
                Code::WidthMismatch,
                format!(
                    "{} components meet {}; an elementwise operation needs equal widths, or one \
                     side mono",
                    a.width, b.width
                ),
            ),
        };
        Refusal::new(code, origin, message)
    }
}

/// One atom factor, so a refusal can name the pair that met rather than the whole product,
/// or `Value` where no factor is to blame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Factor {
    Value,
    Amplitude,
    Polynomial,
    Exponential,
    Gaussian,
    Indicator,
    Pole,
    PrincipalValue,
    Delta,
}

impl Factor {
    pub fn as_str(self) -> &'static str {
        match self {
            Factor::Value => "this subterm",
            Factor::Amplitude => "an amplitude",
            Factor::Polynomial => "a polynomial",
            Factor::Exponential => "an exponential",
            Factor::Gaussian => "a Gaussian",
            Factor::Indicator => "an indicator",
            Factor::Pole => "a pole",
            Factor::PrincipalValue => "a principal value",
            Factor::Delta => "a delta",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtomSketch {
    pub first: Factor,
    pub second: Option<Factor>,
}

impl AtomSketch {
    pub fn of(first: Factor) -> AtomSketch {
        AtomSketch {
            first,
            second: None,
        }
    }

    pub fn pair(first: Factor, second: Factor) -> AtomSketch {
        AtomSketch {
            first,
            second: Some(second),
        }
    }

    pub fn describe(self) -> String {
        match self.second {
            Some(second) => format!("{} times {}", self.first.as_str(), second.as_str()),
            None => self.first.as_str().to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftReason {
    NotTempered,
    GaussianTimesIndicator,
    GaussianTimesPole,
    PoleTimesIndicator,
    PoleOrder(u16),
    NonAffineArgument,
    KeyedOnASignal,
    Nonlinearity,
    Reciprocal,
    SeriesNotSummable,
    SeriesNonUniform,
    SeriesTimesSeries,
    HilbertOfPolynomial,
    NotInTable,
    NoValue,
    Unsubstituted,
}

impl LeftReason {
    /// The refusal's second line. Every one names what the dual would have to be, so "left A"
    /// never reads as "no transform exists".
    pub fn clause(self) -> &'static str {
        match self {
            LeftReason::NotTempered => {
                "an exponential growing on an unbounded side is not a tempered distribution"
            }
            LeftReason::GaussianTimesIndicator => {
                "a Gaussian times an indicator leaves A: its dual is an error function"
            }
            LeftReason::GaussianTimesPole => {
                "a Gaussian times a pole leaves A: its dual is a Faddeeva function"
            }
            LeftReason::PoleTimesIndicator => {
                "a pole times an indicator leaves A: its dual is an exponential integral"
            }
            LeftReason::PoleOrder(_) => {
                "A holds residues to total pole order 12, and a principal value only at order one"
            }
            LeftReason::NonAffineArgument => {
                "an argument that is not affine in the free variable spreads to infinitely \
                 many lines"
            }
            LeftReason::KeyedOnASignal => {
                "a hash keyed on a moving value is piecewise constant, and a piecewise \
                 constant has no finite atom sum"
            }
            LeftReason::Nonlinearity => "a bounded nonlinearity has no finite atom sum",
            LeftReason::Reciprocal => {
                "a reciprocal of a non-constant closed form is not an atom sum"
            }
            LeftReason::SeriesNotSummable => {
                "a series converges in A only with polynomially bounded coefficients"
            }
            LeftReason::SeriesNonUniform => {
                "a series index entering a Gaussian width or an indicator bound has no \
                 termwise dual"
            }
            LeftReason::SeriesTimesSeries => {
                "two series multiply term by term only where one of them is a finite atom sum"
            }
            LeftReason::HilbertOfPolynomial => {
                "the Hilbert transform of a polynomial has no tempered value"
            }
            LeftReason::NotInTable => {
                "no row of this table version holds this atom; a later version may"
            }
            LeftReason::NoValue => {
                "a division or a remainder by zero names no number, and no atom holds one"
            }
            LeftReason::Unsubstituted => {
                "a node or a parameter reaches the closed form unsubstituted; the graph resolves one \
                 before it normalizes"
            }
        }
    }
}

/// The atom that left A, the token naming where it was written, and the factor pair that
/// did it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Left {
    pub origin: Origin,
    pub sketch: AtomSketch,
    pub reason: LeftReason,
}

impl Left {
    pub fn new(origin: Origin, sketch: AtomSketch, reason: LeftReason) -> Left {
        Left {
            origin,
            sketch,
            reason,
        }
    }

    pub fn refusal(self) -> Refusal {
        Refusal::new(
            Code::LeftAlgebra,
            self.origin,
            format!(
                "{} left A. {}",
                self.sketch.describe(),
                self.reason.clause()
            ),
        )
    }
}
