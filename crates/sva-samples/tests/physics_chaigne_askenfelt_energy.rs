// Concern: states a chaigne_askenfelt string never gains energy once let go, and a release moves nothing before its felt lands | Non-concern: a unison's balance | IO: (Params) -> asserted energies

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

fn samples(p: &ChaigneAskenfeltParams, len: usize) -> Vec<f64> {
    let mut site = ChaigneAskenfeltSite::new(p, f64::from(RATE)).expect("a grid");
    (0..len).map(|_| site.step().expect("a sample")).collect()
}

/// Steps `steps` more, and refuses the first step whose energy rose.
fn never_gains(site: &mut ChaigneAskenfeltSite, steps: usize, what: &str) {
    let mut held = site.energy();
    for k in 0..steps {
        site.step().expect("a sample");
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
            site.step().expect("a sample");
        }
        never_gains(&mut site, RATE as usize, &format!("f0 {f0}"));
    }
}

#[test]
fn a_felted_string_never_gains_energy_once_pressed() {
    for (f0, damper_k) in [(65.406, 0.0), (261.63, 0.0), (261.63, 5e3), (2093.0, 2e3)] {
        let p = ChaigneAskenfeltParams {
            damper_k,
            release: 0.1,
            ..note(f0)
        };
        let mut site = ChaigneAskenfeltSite::new(&p, f64::from(RATE)).expect("a grid");
        for _ in 0..=(0.1 * f64::from(RATE)).ceil() as usize {
            site.step().expect("a sample");
        }
        never_gains(&mut site, RATE as usize / 2, &format!("f0 {f0}"));
    }
}

#[test]
fn a_released_note_is_the_held_note_until_the_felt_lands() {
    for unison_count in [1.0, 3.0] {
        let held = ChaigneAskenfeltParams {
            unison_count,
            ..note(261.63)
        };
        let len = RATE as usize;
        let before = samples(&held, len);
        let after = samples(
            &ChaigneAskenfeltParams {
                release: 0.5,
                ..held
            },
            len,
        );
        let landing = (0.5 * f64::from(RATE)).ceil() as usize;
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
