// Concern: the five written casts, the type each produces and the mismatch it writes | Non-concern: performing the approximation (sva-samples) | IO: (Cast, &[Ty]) -> Ty or Mismatch

use sva_formula::{Codomain, Held, Origin, Ty, Var};

/// The operand types, the subterm that blocked, and the cast to write.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub code: &'static str,
    pub got: Vec<Ty>,
    pub blocker: Option<(Origin, String)>,
    pub repair: String,
}

impl Mismatch {
    pub fn new(code: &'static str, got: &[Ty], repair: impl Into<String>) -> Mismatch {
        Mismatch {
            code,
            got: got.to_vec(),
            blocker: None,
            repair: repair.into(),
        }
    }

    pub fn blocked_by(mut self, origin: Origin, what: impl Into<String>) -> Mismatch {
        self.blocker = Some((origin, what.into()));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cast {
    Sample,
    Fourier,
    IFourier,
    Stft { window: usize, hop: usize },
    Istft,
}

/// The named arguments a call carries, already folded to numbers.
pub type Named<'a> = [(&'a str, f64)];

impl Cast {
    pub const NAMES: [&'static str; 5] = ["sample", "fourier", "ifourier", "stft", "istft"];

    pub fn from_name(name: &str, named: &Named) -> Option<Cast> {
        let read = |key: &str| named.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        Some(match name {
            "sample" => Cast::Sample,
            "fourier" => Cast::Fourier,
            "ifourier" => Cast::IFourier,
            "stft" => Cast::Stft {
                window: read("window").unwrap_or(0.0) as usize,
                hop: read("hop").unwrap_or(0.0) as usize,
            },
            "istft" => Cast::Istft,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Cast::Sample => "sample",
            Cast::Fourier => "fourier",
            Cast::IFourier => "ifourier",
            Cast::Stft { .. } => "stft",
            Cast::Istft => "istft",
        }
    }

    /// A window and a hop belong to the cast, so they are checked where the call is written.
    pub fn check_arguments(self) -> Result<(), Mismatch> {
        match self {
            Cast::Stft { window, hop } if window == 0 || hop == 0 => Err(Mismatch::new(
                "cast.missing_window",
                &[],
                "stft needs window= and hop=",
            )),
            _ => Ok(()),
        }
    }

    pub fn resolve(self, args: &[Ty]) -> Result<Ty, Mismatch> {
        let [arg] = args else {
            return Err(Mismatch::new(
                "grammar.arity",
                args,
                format!("`{}` takes one signal", self.name()),
            ));
        };
        match self {
            Cast::Fourier => self.project(*arg, Var::F, args),
            Cast::IFourier => self.project(*arg, Var::T, args),
            Cast::Sample => match arg.held {
                Held::Form(Var::T) => Ok(sampled(arg)),
                Held::Form(Var::F) if arg.has_dual() => Ok(sampled(arg)),
                Held::Form(Var::F) => Err(Mismatch::new(
                    "cast.left_algebra",
                    args,
                    "write sample(ifourier(x)) to sample a spectrum in t",
                )),
                Held::Sampled | Held::Frames => Err(Mismatch::new(
                    "type.samples_in_closed_form",
                    args,
                    "this is already samples; drop the sample(...)",
                )),
            },
            Cast::Stft { .. } => match arg.held {
                Held::Sampled => Ok(Ty {
                    held: Held::Frames,
                    ..*arg
                }),
                Held::Frames => Err(Mismatch::new(
                    "cast.stft_needs_samples",
                    args,
                    "this is already frames; drop the stft(...)",
                )),
                _ => Err(Mismatch::new(
                    "cast.stft_needs_samples",
                    args,
                    "stft reads samples. write stft(sample(x), window=, hop=)",
                )),
            },
            Cast::Istft => match arg.held {
                Held::Frames => Ok(sampled(arg)),
                _ => Err(Mismatch::new(
                    "cast.istft_needs_frames",
                    args,
                    "istft reads frames. write istft(stft(x, window=, hop=))",
                )),
            },
        }
    }

    /// A retyping cast selects the other axis of a value that has both, and changes nothing:
    /// what it states is the axis the expression above it is written on.
    fn project(self, arg: Ty, onto: Var, args: &[Ty]) -> Result<Ty, Mismatch> {
        match arg.held {
            Held::Form(_) if arg.has_dual() => Ok(Ty {
                held: Held::Form(onto),
                codomain: match onto {
                    Var::F => Codomain::Complex,
                    Var::T => arg.codomain,
                },
                ..arg
            }),
            Held::Sampled | Held::Frames => Err(Mismatch::new(
                "cast.samples_are_terminal",
                args,
                "nothing returns from samples to a closed form",
            )),
            _ => Err(Mismatch::new(
                "cast.left_algebra",
                args,
                format!(
                    "write {}(sample(x)) for a short-time spectrum, labeled measured",
                    self.name()
                ),
            )),
        }
    }
}

fn sampled(arg: &Ty) -> Ty {
    Ty {
        held: Held::Sampled,
        dual: false,
        ..*arg
    }
}
