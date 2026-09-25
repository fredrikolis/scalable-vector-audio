// Concern: what each builtin's named arguments mean, their unit and the model part each moves | Non-concern: which names a builtin takes (vocabulary.rs) | IO: (builtin, name) -> Meaning

use sva_formula::filter::Shape;

use crate::lower::physics::MODAL;
use crate::overload::FINITE_DIFFERENCE;

/// `unit` is `none` for a pure number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meaning {
    pub text: &'static str,
    pub unit: &'static str,
    pub part: Option<&'static str>,
}

const fn m(text: &'static str, unit: &'static str, part: &'static str) -> Meaning {
    Meaning {
        text,
        unit,
        part: Some(part),
    }
}

const fn plain(text: &'static str, unit: &'static str) -> Meaning {
    Meaning {
        text,
        unit,
        part: None,
    }
}

/// `None` for a name `builtin` does not take, by name or, for a solver or a bank, by position.
pub fn meaning(builtin: &str, key: &str) -> Option<Meaning> {
    if Shape::from_name(builtin).is_some() {
        return filter(key);
    }
    if FINITE_DIFFERENCE.contains(&builtin) {
        return solver(builtin, key);
    }
    if MODAL.contains(&builtin) {
        return modal(builtin, key);
    }
    Some(match (builtin, key) {
        ("saw" | "square" | "triangle", "tol") => plain(
            "admitted and unused: the series is truncated where the profile's floor ends",
            "none",
        ),
        ("sat", "drive") => plain("gain applied before the clip to [-1, 1]", "none"),
        ("crop", "start") => plain("where the window opens, the second positional", "s"),
        ("crop", "end") => plain("where the window closes, the third positional", "s"),
        ("crop", "rise") => plain("raised-cosine fade-in from the window's start", "s"),
        ("crop", "fall") => plain("raised-cosine fade-out into the window's end", "s"),
        ("rand", "seed") => plain("which hash the key is drawn through", "none"),
        ("delta", "k") => plain("derivative order of the impulse", "none"),
        ("stft", "window") => plain("frame length", "samples"),
        ("stft", "hop") => plain("frame advance", "samples"),
        ("noise", "period") => plain("repeat time; the lines fall every 1/period Hz", "s"),
        ("noise", "color") => plain("spectral tilt of the lines", "dB/octave"),
        _ => return None,
    })
}

fn filter(key: &str) -> Option<Meaning> {
    Some(match key {
        "cutoff" => plain("corner or centre frequency", "Hz"),
        "q" => plain("resonance: centre frequency over bandwidth", "none"),
        "gain" => plain("boost or cut of a shelf or peak", "dB"),
        _ => return None,
    })
}

/// A finite-difference model's losses act on its own grid: `damp_dc` on velocity,
/// `damp_freq` on the rate of curvature, except the bore's, which scale two loss families.
fn solver(builtin: &str, key: &str) -> Option<Meaning> {
    let part = match builtin {
        "chaigne_askenfelt" | "willemsen_bilbao_serafin" => "string",
        "darabundit_scavone" => "bore",
        "rhaouti_chaigne_joly" => "membrane",
        "chaigne_doutaut" => "bar",
        "botteldooren" => "room",
        _ => return None,
    };
    if let Some(n) = numbered(key, "string") {
        return match n.1 {
            "_cents" => Some(m(
                "string's offset from its unison placing",
                "cents",
                "string",
            )),
            "_hammer_k_ratio" => Some(m(
                "felt stiffness this string meets, as a multiple of hammer_k",
                "none",
                "hammer",
            )),
            _ => None,
        };
    }
    if let Some(n) = numbered(key, "hole") {
        return match n.1 {
            "_pos" => Some(m("tone hole position along the bore", "none", "tonehole")),
            "_open" => Some(m("1 leaves the hole open, 0 closes it", "none", "tonehole")),
            "_radius" => Some(m("tone hole radius", "m", "tonehole")),
            "_height" => Some(m("tone hole chimney height", "m", "tonehole")),
            _ => None,
        };
    }
    Some(match (builtin, key) {
        ("darabundit_scavone", "length") => m("bore length", "m", part),
        (_, "f0") => m("fundamental frequency", "Hz", part),
        ("darabundit_scavone", "damp_dc") => m("scale on the viscous wall loss", "none", part),
        ("darabundit_scavone", "damp_freq") => m("scale on the thermal wall loss", "none", part),
        (_, "damp_dc") => m("frequency-independent loss", "1/s", part),
        (_, "damp_freq") => m("frequency-dependent loss", "m^2/s", part),
        (_, "b") => m("string stiffness (inharmonicity)", "none", "string"),
        (_, "strike_pos") => m(
            "where the hammer strikes, along the length",
            "none",
            "hammer",
        ),
        (_, "strike_x") => m("where the hammer strikes, across x", "none", "hammer"),
        (_, "strike_y") => m("where the hammer strikes, across y", "none", "hammer"),
        (_, "vel") => m("hammer velocity at contact", "m/s", "hammer"),
        (_, "hammer_mass") => m("hammer mass", "kg", "hammer"),
        (_, "hammer_k") => m("felt stiffness K in F = K compression^p", "N/m^p", "hammer"),
        (_, "hammer_p") => m("felt stiffness exponent p", "none", "hammer"),
        (_, "unison_count") => m("strings struck together, 1 to 3", "none", "string"),
        (_, "detune") => m(
            "frequency ratio across the unison's outer strings",
            "none",
            "string",
        ),
        (_, "bridge_coupling") => m(
            "bridge resistance, in string wave impedances",
            "none",
            "bridge",
        ),
        (_, "bridge_mass") => m("bridge mass; 0 is massless", "kg", "bridge"),
        (_, "bow_pos") => m("where the bow touches, along the length", "none", "bow"),
        (_, "bow_vel") => m("bow velocity", "m/s", "bow"),
        (_, "bow_force") => m("force pressing the bow on the string", "N", "bow"),
        (_, "mu_s") => m("static friction coefficient", "none", "friction"),
        (_, "mu_c") => m("sliding (Coulomb) friction coefficient", "none", "friction"),
        (_, "stribeck_vel") => m(
            "slip speed over which static friction falls to sliding",
            "m/s",
            "friction",
        ),
        (_, "bristle_stiffness") => m("bristle stiffness s0", "N/m", "friction"),
        (_, "bristle_damping") => m("bristle damping s1", "N*s/m", "friction"),
        (_, "viscous_friction") => m("viscous friction s2", "N*s/m", "friction"),
        (_, "radius_in") => m("bore radius at the excited end", "m", "bore"),
        (_, "radius_out") => m("bore radius at the far end, a cone between", "m", "bore"),
        (_, "excite_pos") => m("where the pulse enters, along the length", "none", "source"),
        (_, "pulse_amp") => m("pressure pulse peak", "Pa", "source"),
        (_, "pulse_width") => m("pressure pulse duration", "s", "source"),
        (_, "aspect_ratio") => m("side ratio lx/ly at a fixed area", "none", "membrane"),
        (_, "aspect_y") => m("room depth, in room lengths", "none", "room"),
        (_, "aspect_z") => m("room height, in room lengths", "none", "room"),
        (_, "listener_x") => m("listener position along the length", "none", "listener"),
        (_, "listener_y") => m("listener position along the depth", "none", "listener"),
        (_, "listener_z") => m("listener position along the height", "none", "listener"),
        _ => return None,
    })
}

/// `string2_cents` is `(2, "_cents")`.
fn numbered<'k>(key: &'k str, stem: &str) -> Option<(u32, &'k str)> {
    let rest = key.strip_prefix(stem)?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    Some((rest[..digits].parse().ok()?, &rest[digits..]))
}

/// A strike's hammer, and the decay every mode shares: `1/tau = damp_dc + damp_freq f^2`.
fn modal(builtin: &str, key: &str) -> Option<Meaning> {
    Some(match (builtin, key) {
        ("hammer_pulse", "vel") => m("strike velocity", "m/s", "hammer"),
        ("helmholtz", "volume") => m("cavity volume", "m^3", "cavity"),
        ("string", "f0") => m("fundamental frequency", "Hz", "string"),
        ("membrane" | "room", "lx") => m("side along x", "m", builtin_part(builtin)),
        ("membrane" | "room", "ly") => m("side along y", "m", builtin_part(builtin)),
        ("room", "lz") => m("side along z", "m", "room"),
        ("bar" | "bore", "length") => m("length", "m", builtin_part(builtin)),
        (_, "damp_dc") => m("decay rate every mode shares", "1/s", "damping"),
        (_, "damp_freq") => m("decay rate per squared hertz", "s", "damping"),
        (_, "modes") => plain("how many modes the bank holds", "none"),
        (_, "vel") => m(
            "strike velocity; omitted, an impulse rings the bank",
            "m/s",
            "hammer",
        ),
        (_, "at") => m("when the strike lands", "s", "hammer"),
        (_, "contact") => m("contact time at 3.2 m/s", "s", "hammer"),
        (_, "p") => m("felt stiffness exponent", "none", "hammer"),
        (_, "force") => m("peak force at 3.2 m/s", "N", "hammer"),
        ("string", "inharmonicity") => m("string stiffness B", "none", "string"),
        ("string", "strike") => m("where the strike lands, along the length", "none", "hammer"),
        ("membrane", "tension") => m("membrane tension", "N/m", "membrane"),
        ("membrane", "density") => m("membrane areal density", "kg/m^2", "membrane"),
        ("membrane", "strike_x") => m("where the strike lands, across x", "none", "hammer"),
        ("membrane", "strike_y") => m("where the strike lands, across y", "none", "hammer"),
        ("bar", "thickness") => m("bar thickness", "m", "bar"),
        ("bar", "young") => m("Young's modulus", "Pa", "bar"),
        ("bar", "density") => m("bar density", "kg/m^3", "bar"),
        ("bore", "radius") => m("bore radius", "m", "bore"),
        ("bore" | "room" | "helmholtz", "speed") => m("speed of sound", "m/s", "air"),
        ("bore", "closed") => m("1 closes one end, 0 leaves both open", "none", "bore"),
        ("helmholtz", "neck_area") => m("neck cross-section", "m^2", "neck"),
        ("helmholtz", "neck_length") => m("neck length", "m", "neck"),
        ("helmholtz", "radius") => m("neck radius, for the 1.7 r end correction", "m", "neck"),
        _ => return None,
    })
}

fn builtin_part(builtin: &str) -> &'static str {
    match builtin {
        "membrane" => "membrane",
        "room" => "room",
        "bar" => "bar",
        _ => "bore",
    }
}
