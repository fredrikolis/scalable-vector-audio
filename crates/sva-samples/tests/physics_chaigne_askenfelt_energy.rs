// Concern: states a chaigne_askenfelt string never gains energy once let go, and a felt moves nothing before it presses | Non-concern: a unison's balance | IO: (Params) -> asserted energies

use sva_samples::Solver;
use sva_samples::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};

const RATE: u32 = 44_100;

fn note(f0: f64) -> ChaigneAskenfeltParams {
    ChaigneAskenfeltParams {
        vel: 4.5,
        strike_pos: 0.125,
        ..ChaigneAskenfeltParams::at(f0)
    }
}

/// The felt's dashpot at each sample, then its spring.
fn samples(p: &ChaigneAskenfeltParams, len: usize, felt: impl Fn(usize) -> [f64; 2]) -> Vec<f64> {
    let mut site = ChaigneAskenfeltSite::new(p, f64::from(RATE)).expect("a grid");
    (0..len)
        .map(|n| site.step(&felt(n)).expect("a sample"))
        .collect()
}

/// Steps `steps` more, and refuses the first step whose energy rose.
fn never_gains(site: &mut ChaigneAskenfeltSite, steps: usize, what: &str) {
    let mut held = site.energy();
    for k in 0..steps {
        site.step(&[]).expect("a sample");
        let now = site.energy();
        assert!(
            now <= held * (1.0 + 1e-10),
            "{what} step {k}: E rose {held} -> {now}"
        );
        held = now;
    }
}

#[test]
fn a_free_string_never_gains_energy_once_let_go() {
    for f0 in [65.406, 261.63, 2093.0] {
        let mut site = ChaigneAskenfeltSite::new(&note(f0), f64::from(RATE)).expect("a grid");
        while !site.let_go() {
            site.step(&[]).expect("a sample");
        }
        never_gains(&mut site, RATE as usize, &format!("f0 {f0}"));
    }
}

#[test]
fn a_felted_string_never_gains_energy_once_pressed() {
    for (f0, damper_k) in [(65.406, 0.0), (261.63, 0.0), (261.63, 5e3), (2093.0, 2e3)] {
        let p = ChaigneAskenfeltParams {
            damper_r: 0.1 * (262.0f64 / f0).powi(2),
            damper_k,
            ..note(f0)
        };
        let mut site = ChaigneAskenfeltSite::new(&p, f64::from(RATE)).expect("a grid");
        while !site.let_go() {
            site.step(&[]).expect("a sample");
        }
        never_gains(&mut site, RATE as usize / 2, &format!("f0 {f0}"));
    }
}

/// A felt that presses from `landing` on leaves every sample before it the held note's, and
/// takes the tail off after.
#[test]
fn a_released_note_is_the_held_note_until_the_felt_presses() {
    for unison_count in [1.0, 3.0] {
        let held = ChaigneAskenfeltParams {
            unison_count,
            ..note(261.63)
        };
        let (len, landing) = (RATE as usize, RATE as usize / 2);
        let before = samples(&held, len, |_| [0.0, 0.0]);
        let r = 0.1;
        let after = samples(&held, len, |n| [if n < landing { 0.0 } else { r }, 0.0]);
        assert_eq!(
            before[..=landing],
            after[..=landing],
            "{unison_count} strings"
        );
        let tail_of = |s: &[f64]| s[len - 4410..].iter().fold(0.0f64, |a, v| a.max(v.abs()));
        assert!(
            tail_of(&after) < tail_of(&before) / 3.0,
            "{unison_count} strings: the felt took off only {} of {}",
            tail_of(&after),
            tail_of(&before)
        );
    }
}
