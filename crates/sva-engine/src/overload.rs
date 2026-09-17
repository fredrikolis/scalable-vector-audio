// Concern: the signature of every builtin and operator, and the one refusal a call writes | Non-concern: evaluating them, the named-argument tables (vocabulary.rs) | IO: (name, &[Ty]) -> Ty

use sva_formula::{Codomain, Held, MAX_WIDTH, Mismatch as MeetMismatch, Ty, Var};

use crate::cast::{Cast, Mismatch};
use crate::error::{Diagnostic, Located};
use crate::vocabulary::recognized_named;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Signal,
    Scalar,
    Index,
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: &'static str,
    pub required: bool,
    pub kind: ParamKind,
}

pub struct Signature {
    pub name: &'static str,
    pub params: &'static [Param],
    /// `join` takes two to `MAX_WIDTH` components; nothing else takes an open tail.
    pub variadic: bool,
    pub result: fn(&[Ty]) -> Result<Ty, Mismatch>,
}

impl Signature {
    /// One table per name, held where the CLI already prints it from.
    pub fn named(&self) -> &'static [&'static str] {
        recognized_named(self.name).unwrap_or(&[])
    }

    pub fn required(&self) -> usize {
        self.params.iter().filter(|p| p.required).count()
    }
}

const fn need(name: &'static str, kind: ParamKind) -> Param {
    Param {
        name,
        required: true,
        kind,
    }
}

const fn opt(name: &'static str, kind: ParamKind) -> Param {
    Param {
        name,
        required: false,
        kind,
    }
}

const SIGNAL: &[Param] = &[need("x", ParamKind::Signal)];
const TWO: &[Param] = &[need("a", ParamKind::Signal), need("b", ParamKind::Signal)];
const WAVE: &[Param] = &[
    need("hz", ParamKind::Scalar),
    opt("phase", ParamKind::Scalar),
];
const CROP: &[Param] = &[
    need("x", ParamKind::Signal),
    need("a", ParamKind::Scalar),
    need("b", ParamKind::Scalar),
];
const DRIVEN: &[Param] = &[
    need("x", ParamKind::Signal),
    opt("drive", ParamKind::Scalar),
];
const RAND: &[Param] = &[
    opt("key", ParamKind::Signal),
    opt("seed", ParamKind::Scalar),
];
const SERIES_PARAMS: &[Param] = &[
    need("k", ParamKind::Index),
    need("lo", ParamKind::Bound),
    need("hi", ParamKind::Bound),
    need("term", ParamKind::Signal),
];
const CHANNEL_PARAMS: &[Param] = &[need("x", ParamKind::Signal), need("k", ParamKind::Index)];
const FUNDAMENTAL: &[Param] = &[need("f0", ParamKind::Scalar)];
const SEED: &[Param] = &[need("seed", ParamKind::Scalar)];
const LENGTH: &[Param] = &[need("length", ParamKind::Scalar)];
const VELOCITY: &[Param] = &[need("vel", ParamKind::Scalar)];
const VOLUME: &[Param] = &[need("volume", ParamKind::Scalar)];
const RECTANGLE: &[Param] = &[need("lx", ParamKind::Scalar), opt("ly", ParamKind::Scalar)];
const BOX_SIDES: &[Param] = &[
    need("lx", ParamKind::Scalar),
    opt("ly", ParamKind::Scalar),
    opt("lz", ParamKind::Scalar),
];
const FILTER_ONE_POLE: &[Param] = &[
    need("x", ParamKind::Signal),
    need("cutoff", ParamKind::Scalar),
];
const FILTER_Q: &[Param] = &[
    need("x", ParamKind::Signal),
    need("cutoff", ParamKind::Scalar),
    opt("q", ParamKind::Scalar),
];
const FILTER_GAIN: &[Param] = &[
    need("x", ParamKind::Signal),
    need("cutoff", ParamKind::Scalar),
    opt("q", ParamKind::Scalar),
    opt("gain", ParamKind::Scalar),
];

/// Overload resolution knows no axis of its own; `lower::walk` reads the answer onto the
/// node's own before it becomes a type.
fn neutral() -> Ty {
    Ty::form(Var::T, true, Codomain::Real)
}

fn fault(m: MeetMismatch, args: &[Ty]) -> Mismatch {
    let (code, repair) = match m {
        MeetMismatch::Domain => (
            "type.domain_mismatch",
            "write ifourier on the f side to work in t, or fourier on the t side to work in f",
        ),
        MeetMismatch::SamplesInClosedForm => (
            "type.samples_in_closed_form",
            "write sample(...) on the closed form to move the whole expression into samples",
        ),
        MeetMismatch::Rate => ("type.rate_conflict", "one rate per expression"),
        MeetMismatch::Frames => ("type.frame_mismatch", "one window and hop per expression"),
        MeetMismatch::Width => (
            "type.width_mismatch",
            "give both sides the same width, or make one of them mono",
        ),
    };
    Mismatch::new(code, args, repair)
}

/// The meet of FORMAT 8.2, which is what every elementwise name resolves by.
fn meet(args: &[Ty]) -> Result<Ty, Mismatch> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(neutral());
    };
    let mut acc = *first;
    for ty in rest {
        acc = acc.meet(*ty).map_err(|m| fault(m, args))?;
    }
    Ok(acc)
}

fn scalar(_args: &[Ty]) -> Result<Ty, Mismatch> {
    Ok(neutral())
}

fn dual_form(_args: &[Ty]) -> Result<Ty, Mismatch> {
    Ok(neutral())
}

fn samples(_args: &[Ty]) -> Result<Ty, Mismatch> {
    Ok(Ty::discrete(Held::Sampled, Codomain::Real))
}

/// A closed form keeps its representation and `sva_formula::infer` decides which one; only
/// the closed-form-versus-samples split is settled here.
fn elementwise(args: &[Ty]) -> Result<Ty, Mismatch> {
    meet(args)
}

fn filter(args: &[Ty]) -> Result<Ty, Mismatch> {
    let signal = args.first().copied().unwrap_or_else(neutral);
    match signal.has_dual() || signal.held == Held::Sampled {
        true => meet(std::slice::from_ref(&signal)),
        false => Err(Mismatch::new(
            "type.filter_needs_a_dual",
            args,
            "write the filter over sample(x) to filter at the render rate",
        )),
    }
}

fn series(args: &[Ty]) -> Result<Ty, Mismatch> {
    Ok(args.get(3).copied().unwrap_or_else(neutral))
}

fn joined(args: &[Ty]) -> Result<Ty, Mismatch> {
    let mut width = 0u32;
    let mono: Vec<Ty> = args.iter().map(|t| Ty { width: 1, ..*t }).collect();
    for ty in args {
        width += u32::from(ty.width);
    }
    let acc = meet(&mono)?;
    let Ok(width) = u8::try_from(width) else {
        return Err(too_wide(args, width));
    };
    if width > MAX_WIDTH {
        return Err(too_wide(args, u32::from(width)));
    }
    Ok(Ty { width, ..acc })
}

fn too_wide(args: &[Ty], width: u32) -> Mismatch {
    Mismatch::new(
        "type.width_mismatch",
        args,
        format!("{width} components join; a value carries at most {MAX_WIDTH}"),
    )
}

fn channel(args: &[Ty]) -> Result<Ty, Mismatch> {
    let x = args.first().copied().unwrap_or_else(neutral);
    Ok(Ty { width: 1, ..x })
}

/// The window and hop a call wrote are not in a `Ty`, so the table types the representation
/// and the call site checks the arguments.
fn cast_of(name: &str, args: &[Ty]) -> Result<Ty, Mismatch> {
    Cast::from_name(name, &[("window", 1.0), ("hop", 1.0)])
        .expect("a cast signature names a cast")
        .resolve(args)
}

const fn plain(
    name: &'static str,
    params: &'static [Param],
    result: fn(&[Ty]) -> Result<Ty, Mismatch>,
) -> Signature {
    Signature {
        name,
        params,
        variadic: false,
        result,
    }
}

pub const OPERATORS: [&str; 5] = ["+", "-", "*", "/", "%"];

/// The six finite-difference builtins, which produce samples and never a closed form.
pub const FINITE_DIFFERENCE: [&str; 6] = [
    "chaigne_askenfelt",
    "willemsen_bilbao_serafin",
    "darabundit_scavone",
    "rhaouti_chaigne_joly",
    "chaigne_doutaut",
    "botteldooren",
];

pub static SIGNATURES: &[Signature] = &[
    plain("+", TWO, elementwise),
    plain("-", TWO, elementwise),
    plain("*", TWO, elementwise),
    plain("/", TWO, elementwise),
    plain("%", TWO, elementwise),
    plain("sin", SIGNAL, elementwise),
    plain("cos", SIGNAL, elementwise),
    plain("exp", SIGNAL, elementwise),
    plain("log", SIGNAL, elementwise),
    plain("sqrt", SIGNAL, elementwise),
    plain("abs", SIGNAL, elementwise),
    plain("tanh", SIGNAL, elementwise),
    plain("sat", DRIVEN, elementwise),
    plain("pow", TWO, elementwise),
    plain("max", TWO, elementwise),
    plain("min", TWO, elementwise),
    plain("saw", WAVE, dual_form),
    plain("square", WAVE, dual_form),
    plain("triangle", WAVE, dual_form),
    plain("crop", CROP, elementwise),
    plain("noise", SEED, dual_form),
    plain("string", FUNDAMENTAL, dual_form),
    plain("membrane", RECTANGLE, dual_form),
    plain("bar", LENGTH, dual_form),
    plain("bore", LENGTH, dual_form),
    plain("room", BOX_SIDES, dual_form),
    plain("hammer_pulse", VELOCITY, dual_form),
    plain("helmholtz", VOLUME, dual_form),
    plain("rand", RAND, scalar),
    plain("delta", SIGNAL, dual_form),
    plain("pv", SIGNAL, dual_form),
    plain("sum", SERIES_PARAMS, series),
    Signature {
        name: "join",
        params: TWO,
        variadic: true,
        result: joined,
    },
    plain("ch", CHANNEL_PARAMS, channel),
    plain("chaigne_askenfelt", FUNDAMENTAL, samples),
    plain("willemsen_bilbao_serafin", FUNDAMENTAL, samples),
    plain("darabundit_scavone", FUNDAMENTAL, samples),
    plain("rhaouti_chaigne_joly", FUNDAMENTAL, samples),
    plain("chaigne_doutaut", FUNDAMENTAL, samples),
    plain("botteldooren", FUNDAMENTAL, samples),
    plain("lp", FILTER_ONE_POLE, filter),
    plain("lowpass", FILTER_Q, filter),
    plain("highpass", FILTER_Q, filter),
    plain("bandpass", FILTER_Q, filter),
    plain("notch", FILTER_Q, filter),
    plain("peaking", FILTER_GAIN, filter),
    plain("lowshelf", FILTER_GAIN, filter),
    plain("highshelf", FILTER_GAIN, filter),
    plain("sample", SIGNAL, |a| cast_of("sample", a)),
    plain("fourier", SIGNAL, |a| cast_of("fourier", a)),
    plain("ifourier", SIGNAL, |a| cast_of("ifourier", a)),
    plain("stft", SIGNAL, |a| cast_of("stft", a)),
    plain("istft", SIGNAL, |a| cast_of("istft", a)),
];

pub fn signature(name: &str) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.name == name)
}

/// The signal operands only: a scalar is neutral by FORMAT 3.3's `Sc, S -> S` row, and `Ty`
/// has no scalar of its own to tell one from a constant closed form.
pub fn resolve(name: &str, args: &[Ty]) -> Result<Ty, Mismatch> {
    let Some(sig) = signature(name) else {
        return Err(Mismatch::new(
            "grammar.unknown_name",
            args,
            format!("`{name}` is not a builtin, a unit or a note name"),
        ));
    };
    (sig.result)(args)
}

/// How many arguments a name takes, checked against the call as written. A signal is
/// positional, a required parameter is answered once, and an unread key is refused.
pub fn check_arity(name: &str, positional: usize, named: &[String]) -> Result<(), Mismatch> {
    let Some(sig) = signature(name) else {
        return Ok(());
    };
    let ceiling = if sig.variadic {
        usize::from(MAX_WIDTH)
    } else {
        sig.params.len()
    };
    let mut by_name = 0;
    for key in named {
        let Some(at) = sig.params.iter().position(|p| p.name == *key) else {
            if sig.named().contains(&key.as_str()) {
                continue;
            }
            return Err(Mismatch::new(
                "grammar.unknown_named_argument",
                &[],
                format!("`{name}` does not read `{key}`"),
            ));
        };
        let param = sig.params[at];
        if param.kind == ParamKind::Signal {
            return Err(Mismatch::new(
                "grammar.arity",
                &[],
                format!("give the signal `{name}` reads by position, not as `{key}=`"),
            ));
        }
        if at < positional {
            return Err(Mismatch::new(
                "grammar.arity",
                &[],
                format!("give `{key}` to `{name}` by position or by name, not both"),
            ));
        }
        by_name += usize::from(param.required);
    }
    match positional + by_name >= sig.required() && positional <= ceiling {
        true => Ok(()),
        false => Err(Mismatch::new(
            "grammar.arity",
            &[],
            format!("`{name}` takes {} arguments", sig.params.len()),
        )),
    }
}

/// FORMAT 3.1's notation column, which a table prints where a sentence has no room.
pub fn notation(ty: Ty) -> &'static str {
    match (ty.held, ty.has_dual()) {
        (Held::Form(Var::T), false) => "Form(t)",
        (Held::Form(Var::T), true) => "Form(t) dual",
        (Held::Form(Var::F), false) => "Form(f)",
        (Held::Form(Var::F), true) => "Form(f) dual",
        (Held::Sampled, _) => "Sampled",
        (Held::Frames, _) => "Frames",
    }
}

/// FORMAT 3.1's name column, read out: the same types `notation` spells, in a sentence.
pub fn describe(ty: Ty) -> String {
    let word = match ty.held {
        Held::Form(var) => format!(
            "a closed form in `{}` with {} dual",
            var.as_str(),
            match ty.has_dual() {
                true => "a",
                false => "no",
            }
        ),
        Held::Sampled => "samples".to_string(),
        Held::Frames => "frames".to_string(),
    };
    match (ty.held, ty.rate, ty.width) {
        (Held::Sampled, Some(rate), _) => format!("samples at {rate} Hz"),
        (_, _, 1) => word,
        (_, _, width) => format!("{word}, {width} components"),
    }
}

/// The one producer of the three-part shape of FORMAT 7.1 and the operand-then-repair shape
/// of 8.3: every code of section 16 reaches a diagnostic through here.
pub fn format_refusal(
    call: &str,
    m: &Mismatch,
    at: Located,
    operands: &[(String, Ty)],
) -> Diagnostic {
    let types: Vec<String> = m.got.iter().map(|t| describe(*t)).collect();
    let named: Vec<String> = operands
        .iter()
        .map(|(name, ty)| format!("{name}: {}", describe(*ty)))
        .collect();
    let head = match &m.blocker {
        Some((_, what)) => {
            format!("`{call}` refused: subterm {what} left A")
        }
        None => format!("`{call}` has no overload for ({}).", types.join(", ")),
    };
    let message = match named.is_empty() {
        true => head,
        false => format!("{head} {}.", named.join(". ")),
    };
    Diagnostic {
        code: m.code.to_string(),
        message,
        location: at,
        help: m.repair.clone(),
    }
}
