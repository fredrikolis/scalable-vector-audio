// Concern: the callable names this language answers and the named arguments each takes | Non-concern: what any of them denotes, or evaluating one | IO: (name) -> is-a-builtin, named keys

use sva_formula::filter::Shape;

use crate::lower::physics::MODAL;
use crate::overload::FINITE_DIFFERENCE as PHYSICS;

/// Enough for surround; ambisonics is deferred rather than pretended at.
pub const MAX_WIDTH: usize = 8;

pub use sva_ast::{JOIN, SERIES};
pub const CHANNEL: &str = "ch";

const ARITHMETIC: [&str; 21] = [
    "sin", "cos", "exp", "log", "pow", "sqrt", "abs", "tanh", "max", "min", "saw", "square",
    "triangle", "sat", "crop", "rand", "delta", "pv", SERIES, JOIN, CHANNEL,
];

/// The five written crossings of FORMAT 7, callable like any other name.
pub const CASTS: [&str; 5] = ["sample", "fourier", "ifourier", "stft", "istft"];

/// The noise realization, beside the modal instruments `lower/physics.rs` dispatches:
/// every one of them a closed form.
const WRITTEN_LAWS: [&str; 1] = ["noise"];

const LAWS: [&str; WRITTEN_LAWS.len() + MODAL.len()] = {
    let mut all = [""; WRITTEN_LAWS.len() + MODAL.len()];
    let mut i = 0;
    while i < WRITTEN_LAWS.len() {
        all[i] = WRITTEN_LAWS[i];
        i += 1;
    }
    let mut j = 0;
    while j < MODAL.len() {
        all[WRITTEN_LAWS.len() + j] = MODAL[j];
        j += 1;
    }
    all
};

/// Every callable name besides the filter shapes.
pub const BUILTINS: [&str; ARITHMETIC.len() + PHYSICS.len() + CASTS.len() + LAWS.len()] = {
    let mut all = [""; ARITHMETIC.len() + PHYSICS.len() + CASTS.len() + LAWS.len()];
    let mut i = 0;
    while i < ARITHMETIC.len() {
        all[i] = ARITHMETIC[i];
        i += 1;
    }
    let mut j = 0;
    while j < PHYSICS.len() {
        all[ARITHMETIC.len() + j] = PHYSICS[j];
        j += 1;
    }
    let mut k = 0;
    while k < CASTS.len() {
        all[ARITHMETIC.len() + PHYSICS.len() + k] = CASTS[k];
        k += 1;
    }
    let mut n = 0;
    while n < LAWS.len() {
        all[ARITHMETIC.len() + PHYSICS.len() + CASTS.len() + n] = LAWS[n];
        n += 1;
    }
    all
};

pub fn is_builtin(name: &str) -> bool {
    Shape::from_name(name).is_some() || BUILTINS.contains(&name)
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

/// A filter's cutoff, q and gain may move with `t` or read a signal: the filter routes each
/// itself. Every other named argument is one number.
pub fn named_may_move(name: &str, key: &str) -> bool {
    Shape::from_name(name).is_some() && FILTER_NAMED.contains(&key)
}

/// `None` for a name `is_builtin` does not recognize.
pub fn recognized_named(name: &str) -> Option<&'static [&'static str]> {
    if Shape::from_name(name).is_some() {
        return Some(FILTER_NAMED);
    }
    Some(match name {
        "sin" | "cos" | "exp" | "sqrt" | "abs" | "tanh" | "max" | "min" | "pow" | "log" => NO_NAMED,
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
