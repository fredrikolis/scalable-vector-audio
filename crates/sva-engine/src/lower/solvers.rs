// Concern: the finite-difference parameter set each written call names | Non-concern: solving one (sva-samples), the modal family (physics.rs) | IO: (name, f0, named) -> Params

use sva_samples::Params;
use sva_samples::physics::botteldooren::BotteldoorenParams;
use sva_samples::physics::chaigne_askenfelt::ChaigneAskenfeltParams;
use sva_samples::physics::chaigne_doutaut::ChaigneDoutautParams;
use sva_samples::physics::darabundit_scavone::{BoreParams, MAX_HOLES, ToneholeSpec};
use sva_samples::physics::rhaouti_chaigne_joly::RhaoutiChaigneJolyParams;
use sva_samples::physics::willemsen_bilbao_serafin::WillemsenBilbaoSerafinParams;

use super::calls::named_or;

/// Each model's own reference set at the fundamental asked for, with every written name
/// overriding one field of it.
pub(super) fn params(name: &str, first: f64, named: &[(&str, f64)]) -> Params {
    let at = |key: &str, held: f64| named_or(named, key, held);
    match name {
        "chaigne_askenfelt" => {
            let p = ChaigneAskenfeltParams::at(first);
            Params::ChaigneAskenfelt(ChaigneAskenfeltParams {
                b: at("b", p.b),
                strike_pos: at("strike_pos", p.strike_pos),
                vel: at("vel", p.vel),
                hammer_mass: at("hammer_mass", p.hammer_mass),
                hammer_k: at("hammer_k", p.hammer_k),
                hammer_p: at("hammer_p", p.hammer_p),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                unison_count: at("unison_count", p.unison_count),
                detune: at("detune", p.detune),
                bridge_coupling: at("bridge_coupling", p.bridge_coupling),
                bridge_mass: at("bridge_mass", p.bridge_mass),
                string_cents: [
                    at("string1_cents", p.string_cents[0]),
                    at("string2_cents", p.string_cents[1]),
                    at("string3_cents", p.string_cents[2]),
                ],
                string_hammer_k_ratio: [
                    at("string1_hammer_k_ratio", p.string_hammer_k_ratio[0]),
                    at("string2_hammer_k_ratio", p.string_hammer_k_ratio[1]),
                    at("string3_hammer_k_ratio", p.string_hammer_k_ratio[2]),
                ],
                ..p
            })
        }
        "willemsen_bilbao_serafin" => {
            let p = WillemsenBilbaoSerafinParams::at(first);
            Params::WillemsenBilbaoSerafin(WillemsenBilbaoSerafinParams {
                b: at("b", p.b),
                bow_pos: at("bow_pos", p.bow_pos),
                bow_vel: at("bow_vel", p.bow_vel),
                bow_force: at("bow_force", p.bow_force),
                mu_s: at("mu_s", p.mu_s),
                mu_c: at("mu_c", p.mu_c),
                stribeck_vel: at("stribeck_vel", p.stribeck_vel),
                bristle_stiffness: at("bristle_stiffness", p.bristle_stiffness),
                bristle_damping: at("bristle_damping", p.bristle_damping),
                viscous_friction: at("viscous_friction", p.viscous_friction),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                ..p
            })
        }
        "darabundit_scavone" => {
            let p = BoreParams::at(first);
            Params::DarabunditScavone(BoreParams {
                radius_in: at("radius_in", p.radius_in),
                radius_out: at("radius_out", p.radius_out),
                excite_pos: at("excite_pos", p.excite_pos),
                pulse_amp: at("pulse_amp", p.pulse_amp),
                pulse_width: at("pulse_width", p.pulse_width),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                holes: holes(named),
                ..p
            })
        }
        "rhaouti_chaigne_joly" => {
            let p = RhaoutiChaigneJolyParams::at(first);
            Params::RhaoutiChaigneJoly(RhaoutiChaigneJolyParams {
                aspect_ratio: at("aspect_ratio", p.aspect_ratio),
                strike_x: at("strike_x", p.strike_x),
                strike_y: at("strike_y", p.strike_y),
                vel: at("vel", p.vel),
                hammer_mass: at("hammer_mass", p.hammer_mass),
                hammer_k: at("hammer_k", p.hammer_k),
                hammer_p: at("hammer_p", p.hammer_p),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                ..p
            })
        }
        "chaigne_doutaut" => {
            let p = ChaigneDoutautParams::at(first);
            Params::ChaigneDoutaut(ChaigneDoutautParams {
                strike_pos: at("strike_pos", p.strike_pos),
                vel: at("vel", p.vel),
                hammer_mass: at("hammer_mass", p.hammer_mass),
                hammer_k: at("hammer_k", p.hammer_k),
                hammer_p: at("hammer_p", p.hammer_p),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                ..p
            })
        }
        "botteldooren" => {
            let p = BotteldoorenParams::at(first);
            Params::Botteldooren(BotteldoorenParams {
                aspect_y: at("aspect_y", p.aspect_y),
                aspect_z: at("aspect_z", p.aspect_z),
                listener_x: at("listener_x", p.listener_x),
                listener_y: at("listener_y", p.listener_y),
                listener_z: at("listener_z", p.listener_z),
                pulse_amp: at("pulse_amp", p.pulse_amp),
                pulse_width: at("pulse_width", p.pulse_width),
                damp_dc: at("damp_dc", p.damp_dc),
                damp_freq: at("damp_freq", p.damp_freq),
                ..p
            })
        }
        other => unreachable!("{other} is not a finite-difference builtin"),
    }
}

/// `holeN_pos` opens hole `N`; a bore with no `holeN_pos` has no tone holes.
fn holes(named: &[(&str, f64)]) -> [Option<ToneholeSpec>; MAX_HOLES] {
    let mut out = [None; MAX_HOLES];
    for (slot, hole) in out.iter_mut().enumerate() {
        let n = slot + 1;
        let Some(pos) = named
            .iter()
            .find(|(k, _)| *k == format!("hole{n}_pos"))
            .map(|(_, v)| *v)
        else {
            continue;
        };
        *hole = Some(ToneholeSpec {
            pos,
            open: named_or(named, &format!("hole{n}_open"), 1.0) != 0.0,
            radius: named_or(named, &format!("hole{n}_radius"), 0.005),
            height: named_or(named, &format!("hole{n}_height"), 0.003),
        });
    }
    out
}
