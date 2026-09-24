// Concern: declares the refusals the sampled half reaches, each with its code | Non-concern: the closed-form crate's own codes (sva-formula), rendering a diagnostic (sva-engine) | IO: none

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleError {
    HopOutsideCola {
        window: usize,
        hop: usize,
    },
    WindowNotPowerOfTwo {
        window: usize,
    },
    WidthMismatch {
        left: usize,
        right: usize,
    },
    ChannelOutOfRange {
        k: usize,
        width: usize,
    },
    /// A grid sized from the rate and the fundamental, past what one call may allocate.
    GridTooLarge {
        model: &'static str,
        nodes: usize,
        ceiling: usize,
    },
    /// A string no stable grid at this rate rings at the partials it was asked for.
    StringPastRate {
        model: &'static str,
    },
}

/// What a collapse refuses, per FORMAT 16.3.
#[derive(Clone, Debug, PartialEq)]
pub enum CollapseError {
    NoHorizon,
    EmptyBand {
        ceiling: f64,
        lowest: f64,
    },
    SingularInCt {
        at: f64,
        order: i32,
    },
    NotEvaluable(&'static str),
    NestedSeries {
        depth: usize,
        terms: usize,
        bound: usize,
    },
    LeftAlgebra(&'static str),
}

impl CollapseError {
    pub fn code(&self) -> &'static str {
        match self {
            CollapseError::NoHorizon => "collapse.no_horizon",
            CollapseError::EmptyBand { .. } => "collapse.empty_band",
            CollapseError::SingularInCt { .. } => "collapse.singular_in_ct",
            CollapseError::NotEvaluable(_) => "collapse.not_evaluable",
            CollapseError::NestedSeries { .. } => "collapse.series_nesting",
            CollapseError::LeftAlgebra(_) => "cast.left_algebra",
        }
    }
}

/// One repair per refusal: a window is what a missing horizon wants, and no window brings a
/// line back under a ceiling.
impl CollapseError {
    pub fn help(&self) -> &'static str {
        match self {
            CollapseError::NoHorizon => "give the observation a window with --from and --to",
            CollapseError::EmptyBand { ceiling, .. }
                if *ceiling < sva_formula::AUDIBLE_CEILING_HZ =>
            {
                "this rate's own half is the ceiling: raise --sample-rate past twice the \
                 lowest line"
            }
            CollapseError::EmptyBand { .. } => {
                "the profile's 20 kHz ceiling is what no rate raises; read it with \
                 `--as lines`, or bring the line into the band"
            }
            CollapseError::SingularInCt { .. } => {
                "write it inside a convolution, or sample the closed form it multiplies"
            }
            CollapseError::NestedSeries { .. } => {
                "write fewer levels, or a narrower band for the terms to clear"
            }
            CollapseError::NotEvaluable(_) | CollapseError::LeftAlgebra(_) => {
                "write the subterm inside sample(...) to leave A deliberately"
            }
        }
    }
}

impl std::fmt::Display for CollapseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollapseError::NoHorizon => {
                write!(
                    f,
                    "a closed form collapses against a horizon. give --from and --to"
                )
            }
            CollapseError::EmptyBand { ceiling, lowest } => write!(
                f,
                "every line is at or above the {ceiling} Hz ceiling, the lowest at {lowest} Hz"
            ),
            CollapseError::SingularInCt { at, order } => write!(
                f,
                "a singularity of order {order} at t = {at} has no value on the grid"
            ),
            CollapseError::NotEvaluable(what) => {
                write!(f, "{what} has no value at a point")
            }
            CollapseError::NestedSeries {
                depth,
                terms,
                bound,
            } => write!(
                f,
                "a series nested {depth} deep takes {terms} terms at one instant, past the \
                 {bound} one expansion holds"
            ),
            CollapseError::LeftAlgebra(clause) => write!(f, "the closed form left A. {clause}"),
        }
    }
}

impl SampleError {
    pub fn code(&self) -> &'static str {
        match self {
            SampleError::HopOutsideCola { .. } => "samples.hop_outside_cola",
            SampleError::WindowNotPowerOfTwo { .. } => "samples.window_not_power_of_two",
            SampleError::WidthMismatch { .. } => "type.width_mismatch",
            SampleError::ChannelOutOfRange { .. } => "samples.channel_out_of_range",
            SampleError::GridTooLarge { .. } => "samples.grid_too_large",
            SampleError::StringPastRate { .. } => "samples.string_past_rate",
        }
    }
}

impl std::fmt::Display for SampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SampleError::HopOutsideCola { window, hop } => write!(
                f,
                "a window of {window} at a hop of {hop} does not overlap-add to a constant. \
                 write a hop that divides the window"
            ),
            SampleError::WindowNotPowerOfTwo { window } => {
                write!(f, "a window of {window} is not a power of two")
            }
            SampleError::WidthMismatch { left, right } => write!(
                f,
                "{left} components meet {right}; an elementwise operation needs equal widths, \
                 or one side mono"
            ),
            SampleError::ChannelOutOfRange { k, width } => {
                write!(f, "component {k} of a value {width} wide")
            }
            SampleError::GridTooLarge {
                model,
                nodes,
                ceiling,
            } => write!(
                f,
                "`{model}` sizes a {nodes}-node grid at this rate, past the {ceiling} one \
                 call may hold. ask for a higher fundamental, or a lower --sample-rate"
            ),
            SampleError::StringPastRate { model } => write!(
                f,
                "`{model}` has no stable grid at this rate whose first two partials ring where \
                 asked. ask for a lower fundamental or `b`, or a higher --sample-rate"
            ),
        }
    }
}
