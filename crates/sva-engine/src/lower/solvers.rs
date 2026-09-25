// Concern: the finite-difference parameter set each written call names | Non-concern: solving one (sva-samples), the modal family (physics.rs) | IO: (name, f0, named) -> Params

use sva_samples::Params;
use sva_samples::physics::botteldooren::BotteldoorenParams;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;
use sva_samples::physics::chaigne_doutaut::ChaigneDoutautParams;
use sva_samples::physics::darabundit_scavone::{BoreParams, MAX_HOLES, ToneholeSpec};
use sva_samples::physics::rhaouti_chaigne_joly::RhaoutiChaigneJolyParams;
use sva_samples::physics::willemsen_bilbao_serafin::WillemsenBilbaoSerafinParams;

use super::calls::named_or;

/// One written name and the field of a model's parameter set it reads and writes.
type Field<P> = (&'static str, fn(&mut P) -> &mut f64);

/// The positional first, under the name the model gives it.
const CHAIGNE_ASKENFELT: &[Field<ChaigneAskenfeltParams>] = &[
    ("f0", |p| &mut p.f0),
    ("b", |p| &mut p.b),
    ("strike_pos", |p| &mut p.strike_pos),
    ("vel", |p| &mut p.vel),
    ("hammer_mass", |p| &mut p.hammer_mass),
    ("hammer_k", |p| &mut p.hammer_k),
    ("hammer_p", |p| &mut p.hammer_p),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
    ("unison_count", |p| &mut p.unison_count),
    ("detune", |p| &mut p.detune),
    ("bridge_coupling", |p| &mut p.bridge_coupling),
    ("bridge_mass", |p| &mut p.bridge_mass),
    ("string1_cents", |p| &mut p.string_cents[0]),
    ("string2_cents", |p| &mut p.string_cents[1]),
    ("string3_cents", |p| &mut p.string_cents[2]),
    ("string1_hammer_k_ratio", |p| {
        &mut p.string_hammer_k_ratio[0]
    }),
    ("string2_hammer_k_ratio", |p| {
        &mut p.string_hammer_k_ratio[1]
    }),
    ("string3_hammer_k_ratio", |p| {
        &mut p.string_hammer_k_ratio[2]
    }),
    ("release", |p| &mut p.release),
    ("damper_pos", |p| &mut p.damper_pos),
    ("damper_r", |p| &mut p.damper_r),
    ("damper_k", |p| &mut p.damper_k),
    ("damper_ramp", |p| &mut p.damper_ramp),
];

const WILLEMSEN_BILBAO_SERAFIN: &[Field<WillemsenBilbaoSerafinParams>] = &[
    ("f0", |p| &mut p.f0),
    ("b", |p| &mut p.b),
    ("bow_pos", |p| &mut p.bow_pos),
    ("bow_vel", |p| &mut p.bow_vel),
    ("bow_force", |p| &mut p.bow_force),
    ("mu_s", |p| &mut p.mu_s),
    ("mu_c", |p| &mut p.mu_c),
    ("stribeck_vel", |p| &mut p.stribeck_vel),
    ("bristle_stiffness", |p| &mut p.bristle_stiffness),
    ("bristle_damping", |p| &mut p.bristle_damping),
    ("viscous_friction", |p| &mut p.viscous_friction),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
];

/// Tone holes are optional slots, read and answered by `holes`.
const DARABUNDIT_SCAVONE: &[Field<BoreParams>] = &[
    ("length", |p| &mut p.length),
    ("radius_in", |p| &mut p.radius_in),
    ("radius_out", |p| &mut p.radius_out),
    ("excite_pos", |p| &mut p.excite_pos),
    ("pulse_amp", |p| &mut p.pulse_amp),
    ("pulse_width", |p| &mut p.pulse_width),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
];

const RHAOUTI_CHAIGNE_JOLY: &[Field<RhaoutiChaigneJolyParams>] = &[
    ("f0", |p| &mut p.f0),
    ("aspect_ratio", |p| &mut p.aspect_ratio),
    ("strike_x", |p| &mut p.strike_x),
    ("strike_y", |p| &mut p.strike_y),
    ("vel", |p| &mut p.vel),
    ("hammer_mass", |p| &mut p.hammer_mass),
    ("hammer_k", |p| &mut p.hammer_k),
    ("hammer_p", |p| &mut p.hammer_p),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
];

const CHAIGNE_DOUTAUT: &[Field<ChaigneDoutautParams>] = &[
    ("f0", |p| &mut p.f0),
    ("strike_pos", |p| &mut p.strike_pos),
    ("vel", |p| &mut p.vel),
    ("hammer_mass", |p| &mut p.hammer_mass),
    ("hammer_k", |p| &mut p.hammer_k),
    ("hammer_p", |p| &mut p.hammer_p),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
];

const BOTTELDOOREN: &[Field<BotteldoorenParams>] = &[
    ("f0", |p| &mut p.f0),
    ("aspect_y", |p| &mut p.aspect_y),
    ("aspect_z", |p| &mut p.aspect_z),
    ("listener_x", |p| &mut p.listener_x),
    ("listener_y", |p| &mut p.listener_y),
    ("listener_z", |p| &mut p.listener_z),
    ("pulse_amp", |p| &mut p.pulse_amp),
    ("pulse_width", |p| &mut p.pulse_width),
    ("damp_dc", |p| &mut p.damp_dc),
    ("damp_freq", |p| &mut p.damp_freq),
];

/// Each model's own reference set at the positional asked for, with every written name
/// overriding the one field it names.
pub(super) fn params(name: &str, first: f64, named: &[(&str, f64)]) -> Params {
    match name {
        "chaigne_askenfelt" => Params::ChaigneAskenfelt(written(
            ChaigneAskenfeltParams::at(first),
            CHAIGNE_ASKENFELT,
            named,
        )),
        "willemsen_bilbao_serafin" => Params::WillemsenBilbaoSerafin(written(
            WillemsenBilbaoSerafinParams::at(first),
            WILLEMSEN_BILBAO_SERAFIN,
            named,
        )),
        "darabundit_scavone" => Params::DarabunditScavone(BoreParams {
            holes: holes(named),
            ..written(BoreParams::at(first), DARABUNDIT_SCAVONE, named)
        }),
        "rhaouti_chaigne_joly" => Params::RhaoutiChaigneJoly(written(
            RhaoutiChaigneJolyParams::at(first),
            RHAOUTI_CHAIGNE_JOLY,
            named,
        )),
        "chaigne_doutaut" => Params::ChaigneDoutaut(written(
            ChaigneDoutautParams::at(first),
            CHAIGNE_DOUTAUT,
            named,
        )),
        "botteldooren" => {
            Params::Botteldooren(written(BotteldoorenParams::at(first), BOTTELDOOREN, named))
        }
        other => unreachable!("{other} is not a finite-difference builtin"),
    }
}

/// The positional is the reference set's own; only a named field is overridden.
fn written<P>(mut p: P, fields: &[Field<P>], named: &[(&str, f64)]) -> P {
    for (key, field) in &fields[1..] {
        let held = *field(&mut p);
        *field(&mut p) = named_or(named, key, held);
    }
    p
}

/// What a solver was handed, read back off its own parameter set through the same fields
/// `params` writes, its positional first.
pub(super) fn handed(params: &Params) -> Vec<(String, f64)> {
    fn read<P: Clone>(p: &P, fields: &[Field<P>]) -> Vec<(String, f64)> {
        let mut held = p.clone();
        fields
            .iter()
            .map(|(key, field)| (key.to_string(), *field(&mut held)))
            .collect()
    }
    match params {
        Params::ChaigneAskenfelt(p) => read(p, CHAIGNE_ASKENFELT),
        Params::WillemsenBilbaoSerafin(p) => read(p, WILLEMSEN_BILBAO_SERAFIN),
        Params::DarabunditScavone(p) => {
            let mut out = read(p, DARABUNDIT_SCAVONE);
            for (slot, hole) in p.holes.iter().enumerate() {
                let Some(h) = hole else { continue };
                out.extend(hole_keys(slot + 1).into_iter().zip([
                    h.pos,
                    f64::from(u8::from(h.open)),
                    h.radius,
                    h.height,
                ]));
            }
            out
        }
        Params::RhaoutiChaigneJoly(p) => read(p, RHAOUTI_CHAIGNE_JOLY),
        Params::ChaigneDoutaut(p) => read(p, CHAIGNE_DOUTAUT),
        Params::Botteldooren(p) => read(p, BOTTELDOOREN),
    }
}

/// Hole `n`'s names: position, open, radius, height.
fn hole_keys(n: usize) -> [String; 4] {
    ["pos", "open", "radius", "height"].map(|k| format!("hole{n}_{k}"))
}

/// `holeN_pos` opens hole `N`; a bore with no `holeN_pos` has no tone holes.
fn holes(named: &[(&str, f64)]) -> [Option<ToneholeSpec>; MAX_HOLES] {
    let mut out = [None; MAX_HOLES];
    for (slot, hole) in out.iter_mut().enumerate() {
        let [pos, open, radius, height] = hole_keys(slot + 1);
        let Some(pos) = named.iter().find(|(k, _)| *k == pos).map(|(_, v)| *v) else {
            continue;
        };
        *hole = Some(ToneholeSpec {
            pos,
            open: named_or(named, &open, 1.0) != 0.0,
            radius: named_or(named, &radius, 0.005),
            height: named_or(named, &height, 0.003),
        });
    }
    out
}
