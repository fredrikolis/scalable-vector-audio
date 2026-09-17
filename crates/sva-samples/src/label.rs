// Concern: states how exact one sampled value is and what it cost | Non-concern: deciding that (collapse.rs), printing it | IO: none

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Exact,
    Measured,
}

/// A line the grid could not hold, levelled against amplitude 1.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dropped {
    pub hz: f64,
    pub db: f64,
}

/// The rows declared once: enum, printed name, name read back.
macro_rules! rules {
    ($($variant:ident => $text:literal,)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Rule {
            $($variant,)+
        }

        impl Rule {
            /// The row of FORMAT 9.1 that ran, as the label prints it.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Rule::$variant => $text,)+
                }
            }

            pub fn named(text: &str) -> Option<Rule> {
                match text {
                    $($text => Some(Rule::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

rules! {
    LineSpectrumExact => "line spectrum, no crop",
    LineSpectrumSummed => "line spectrum, summed directly",
    LineSpectrumMixed => "line spectrum, placed and summed",
    BandLimited => "band-limited projection",
    CroppedPair => "cropped pair, tail truncated",
    PointSampled => "point sampling",
    InverseSpectrum => "inverse spectrum",
    Istft => "inverse short-time transform",
    Reading => "reading",
    Added => "sum, addend by addend",
}

#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Lines {
        rule: Rule,
        /// Lines the transform placed; the rest were summed directly.
        placed: usize,
        summed: usize,
        dropped: Vec<Dropped>,
        dropped_more: usize,
        terms: Option<usize>,
        tail_db: Option<f64>,
    },
    Continuous {
        rule: Rule,
    },
    Cropped {
        rule: Rule,
        tail_db: Option<f64>,
    },
    Point {
        rule: Rule,
        alias_db: Option<f64>,
    },
    Spectrum {
        rule: Rule,
        wrap_db: f64,
    },
    Roundtrip {
        rule: Rule,
        edited: bool,
    },
    /// A reading off a buffer that exists; no collapse ran under it.
    Reading {
        rule: Rule,
    },
    /// A sum whose addends named different rows; the planes added.
    Added {
        parts: Vec<Detail>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub flops: u128,
    pub budget: u128,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub source: Source,
    pub profile: &'static str,
    pub rate: u32,
    pub detail: Detail,
    pub cost: Option<Cost>,
}

impl Label {
    pub fn measured(profile: &'static str, rate: u32) -> Label {
        Label {
            source: Source::Measured,
            profile,
            rate,
            detail: Detail::Reading {
                rule: Rule::Reading,
            },
            cost: None,
        }
    }

    pub fn new(source: Source, profile: &'static str, rate: u32, detail: Detail) -> Label {
        Label {
            source,
            profile,
            rate,
            detail,
            cost: None,
        }
    }

    pub fn costing(self, flops: u128, budget: u128) -> Label {
        Label {
            cost: Some(Cost { flops, budget }),
            ..self
        }
    }

    pub fn rule(&self) -> Rule {
        self.detail.rule()
    }
}

impl Detail {
    pub fn rule(&self) -> Rule {
        match *self {
            Detail::Lines { rule, .. }
            | Detail::Continuous { rule }
            | Detail::Cropped { rule, .. }
            | Detail::Point { rule, .. }
            | Detail::Spectrum { rule, .. }
            | Detail::Roundtrip { rule, .. }
            | Detail::Reading { rule } => rule,
            Detail::Added { .. } => Rule::Added,
        }
    }
}
