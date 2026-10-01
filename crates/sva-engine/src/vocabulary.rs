// Concern: each builtin's named arguments, and the maths each sva-ast filter name stands for | Non-concern: which names are builtins or reserved (sva-ast) | IO: (name) -> named keys, a Shape

use sva_ast::{CHANNEL, FILTERS, JOIN, SERIES};
use sva_formula::filter::{ALL_SHAPES, Shape};

/// Enough for surround; ambisonics is deferred rather than pretended at.
pub const MAX_WIDTH: usize = 8;

/// The maths of the filter sva-ast names `name`.
pub fn shape(name: &str) -> Option<Shape> {
    FILTERS
        .iter()
        .position(|held| *held == name)
        .map(|at| ALL_SHAPES[at])
}

const _: () = assert!(FILTERS.len() == ALL_SHAPES.len());

pub fn shape_name(shape: Shape) -> &'static str {
    let at = ALL_SHAPES.iter().position(|held| *held == shape);
    FILTERS[at.expect("ALL_SHAPES holds every shape")]
}

pub(crate) fn note_hz(name: &str) -> Option<f64> {
    sva_ast::note_midi(name).map(sva_formula::note::frequency)
}

/// Consts, so `recognized_named` hands back the same data a call site checks against.
const NO_NAMED: &[&str] = &[];
const WAVE_NAMED: &[&str] = &["tol"];
const SAT_NAMED: &[&str] = &["drive"];
const CROP_NAMED: &[&str] = &["start", "end", "rise", "fall"];
const RAND_NAMED: &[&str] = &["seed"];
const DELTA_NAMED: &[&str] = &["k"];
const STFT_NAMED: &[&str] = &["window", "hop"];
const NOISE_NAMED: &[&str] = &["period", "color"];
/// Every modal bank reads the same damping, mode budget and excitation beside its own
/// geometry's names. A cavity is one mode, so `one_mode` leaves the budget out.
macro_rules! modal_named {
    (one_mode $($own:literal),+ $(,)?) => {
        &[$($own,)+ "damp_dc", "damp_freq", "vel", "at", "contact", "p", "force"]
    };
    ($($own:literal),+ $(,)?) => {
        &[$($own,)+ "damp_dc", "damp_freq", "modes", "vel", "at", "contact", "p", "force"]
    };
}

const STRING_NAMED: &[&str] = modal_named!("inharmonicity", "strike");
const MEMBRANE_NAMED: &[&str] = modal_named!("tension", "density", "strike_x", "strike_y");
const BAR_NAMED: &[&str] = modal_named!("thickness", "young", "density");
const BORE_NAMED: &[&str] = modal_named!("radius", "speed", "closed");
const ROOM_NAMED: &[&str] = modal_named!("speed");
const HELMHOLTZ_NAMED: &[&str] =
    modal_named!(one_mode "neck_area", "neck_length", "radius", "speed");
const HAMMER_NAMED: &[&str] = &["at", "contact", "p", "force"];
const FILTER_NAMED: &[&str] = &["cutoff", "q", "gain"];
const CHAIGNE_ASKENFELT_NAMED: &[&str] = &[
    "b",
    "strike_pos",
    "vel",
    "hammer_mass",
    "hammer_k",
    "hammer_p",
    "damp_dc",
    "damp_freq",
    "unison_count",
    "detune",
    "bridge_coupling",
    "bridge_mass",
    "string1_cents",
    "string2_cents",
    "string3_cents",
    "string1_hammer_k_ratio",
    "string2_hammer_k_ratio",
    "string3_hammer_k_ratio",
    "damper_pos",
    "damper_r",
    "damper_k",
];
const WILLEMSEN_BILBAO_SERAFIN_NAMED: &[&str] = &[
    "b",
    "bow_pos",
    "bow_vel",
    "bow_force",
    "mu_s",
    "mu_c",
    "stribeck_vel",
    "bristle_stiffness",
    "bristle_damping",
    "viscous_friction",
    "damp_dc",
    "damp_freq",
];
/// `holeN_*` is flat and capped at 6: no array-valued argument exists in this grammar.
const DARABUNDIT_SCAVONE_NAMED: &[&str] = &[
    "radius_in",
    "radius_out",
    "excite_pos",
    "pulse_amp",
    "pulse_width",
    "damp_dc",
    "damp_freq",
    "hole1_pos",
    "hole1_open",
    "hole1_radius",
    "hole1_height",
    "hole2_pos",
    "hole2_open",
    "hole2_radius",
    "hole2_height",
    "hole3_pos",
    "hole3_open",
    "hole3_radius",
    "hole3_height",
    "hole4_pos",
    "hole4_open",
    "hole4_radius",
    "hole4_height",
    "hole5_pos",
    "hole5_open",
    "hole5_radius",
    "hole5_height",
    "hole6_pos",
    "hole6_open",
    "hole6_radius",
    "hole6_height",
];
const RHAOUTI_CHAIGNE_JOLY_NAMED: &[&str] = &[
    "aspect_ratio",
    "strike_x",
    "strike_y",
    "vel",
    "hammer_mass",
    "hammer_k",
    "hammer_p",
    "damp_dc",
    "damp_freq",
];
const CHAIGNE_DOUTAUT_NAMED: &[&str] = &[
    "strike_pos",
    "vel",
    "hammer_mass",
    "hammer_k",
    "hammer_p",
    "damp_dc",
    "damp_freq",
];
const BOTTELDOOREN_NAMED: &[&str] = &[
    "aspect_y",
    "aspect_z",
    "listener_x",
    "listener_y",
    "listener_z",
    "pulse_amp",
    "pulse_width",
    "damp_dc",
    "damp_freq",
];

/// A filter's cutoff, q and gain, and a solver's varying parameters, may move with `t` or
/// read a signal: the builtin routes each itself. Every other named argument is one number.
pub fn named_may_move(name: &str, key: &str) -> bool {
    (shape(name).is_some() && FILTER_NAMED.contains(&key))
        || sva_samples::physics::varying(name)
            .iter()
            .any(|(k, _)| *k == key)
}

/// `None` for a name `is_builtin` does not recognize.
pub fn recognized_named(name: &str) -> Option<&'static [&'static str]> {
    if shape(name).is_some() {
        return Some(FILTER_NAMED);
    }
    Some(match name {
        "sin" | "cos" | "exp" | "sqrt" | "abs" | "tanh" | "step" | "max" | "min" | "pow"
        | "log" => NO_NAMED,
        "saw" | "square" | "triangle" => WAVE_NAMED,
        "sat" => SAT_NAMED,
        SERIES | "pv" => NO_NAMED,
        "crop" => CROP_NAMED,
        "rand" => RAND_NAMED,
        "delta" => DELTA_NAMED,
        "chaigne_askenfelt" => CHAIGNE_ASKENFELT_NAMED,
        "willemsen_bilbao_serafin" => WILLEMSEN_BILBAO_SERAFIN_NAMED,
        "darabundit_scavone" => DARABUNDIT_SCAVONE_NAMED,
        "rhaouti_chaigne_joly" => RHAOUTI_CHAIGNE_JOLY_NAMED,
        "chaigne_doutaut" => CHAIGNE_DOUTAUT_NAMED,
        "botteldooren" => BOTTELDOOREN_NAMED,
        JOIN | CHANNEL => NO_NAMED,
        "stft" => STFT_NAMED,
        "sample" | "fourier" | "ifourier" | "istft" => NO_NAMED,
        "noise" => NOISE_NAMED,
        "string" => STRING_NAMED,
        "membrane" => MEMBRANE_NAMED,
        "bar" => BAR_NAMED,
        "bore" => BORE_NAMED,
        "room" => ROOM_NAMED,
        "helmholtz" => HELMHOLTZ_NAMED,
        "hammer_pulse" => HAMMER_NAMED,
        _ => return None,
    })
}
