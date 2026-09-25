// Concern: states that a chaigne_askenfelt tail bound holds every later sample and its energy never rises | Non-concern: rendering until silent (sva-engine) | IO: (Params) -> asserted bounds

use sva_samples::physics::chaigne_askenfelt::{ChaigneAskenfeltParams, ChaigneAskenfeltSite};
use sva_samples::{Params, Solver, tail};

const RATE: u32 = 44_100;
const STEP: usize = 256;

fn note(f0: f64, vel: f64, strike_pos: f64) -> ChaigneAskenfeltParams {
    ChaigneAskenfeltParams {
        vel,
        strike_pos,
        ..ChaigneAskenfeltParams::at(f0)
    }
}

fn samples(p: &ChaigneAskenfeltParams, len: usize) -> Vec<f64> {
    let mut site = ChaigneAskenfeltSite::new(p, f64::from(RATE)).expect("a grid");
    (0..len).map(|_| site.step().expect("a sample")).collect()
}

/// `at[j]` against the loudest sample from `j * STEP` to the end of a render `secs` long,
/// and how far under the peak the bound has fallen by then.
fn dominates(p: &ChaigneAskenfeltParams, secs: f64) -> f64 {
    let len = (secs * f64::from(RATE)) as usize;
    let heard = samples(p, len);
    let points = len / STEP + 1;
    let at = tail(&Params::ChaigneAskenfelt(p.clone()), RATE, STEP, points).expect("a bound");
    let mut later = 0.0f64;
    for k in (0..len).rev() {
        later = later.max(heard[k].abs());
        if k % STEP == 0 {
            let j = k / STEP;
            assert!(
                at[j] >= later,
                "f0 {} vel {} strike {}: bound {} under {later} from step {k}",
                p.f0,
                p.vel,
                p.strike_pos,
                at[j]
            );
        }
    }
    let peak = heard.iter().fold(0.0f64, |a, s| a.max(s.abs()));
    20.0 * (at[points - 1] / peak).log10()
}

#[test]
fn the_tail_bound_holds_every_later_sample_across_the_keyboard() {
    for (f0, secs) in [(65.406, 2.0), (261.63, 3.0), (2093.0, 3.0)] {
        for (vel, strike_pos) in [(1.5, 0.08), (4.5, 0.125), (6.0, 0.3)] {
            let fallen = dominates(&note(f0, vel, strike_pos), secs);
            assert!(fallen < -3.0, "f0 {f0}: the bound fell only {fallen} dB");
        }
    }
}

#[test]
fn a_free_string_never_gains_energy_and_its_energy_bounds_every_later_sample() {
    for f0 in [65.406, 261.63, 2093.0] {
        let p = note(f0, 4.5, 0.125);
        let mut site = ChaigneAskenfeltSite::new(&p, f64::from(RATE)).expect("a grid");
        while !site.let_go() {
            site.step().expect("a sample");
        }
        let gain = site.energy_gain().expect("one string");
        let mut held = site.energy().expect("one string");
        let slack = 1.0 + 1e-10;
        for k in 0..RATE as usize {
            let sample = site.step().expect("a sample").abs();
            assert!(
                sample <= gain * held.sqrt() * slack,
                "f0 {f0} step {k}: {sample} over c sqrt(E) = {}",
                gain * held.sqrt()
            );
            let now = site.energy().expect("one string");
            assert!(
                now <= held * slack,
                "f0 {f0} step {k}: E rose {held} -> {now}"
            );
            held = now;
        }
    }
}

#[test]
fn a_unison_on_its_bridge_has_no_tail_bound() {
    let p = ChaigneAskenfeltParams {
        unison_count: 3.0,
        ..ChaigneAskenfeltParams::at(261.63)
    };
    let refused = tail(&Params::ChaigneAskenfelt(p.clone()), RATE, STEP, 100).unwrap_err();
    assert!(refused.contains("unison"), "{refused}");
    let site = ChaigneAskenfeltSite::new(&p, f64::from(RATE)).expect("a grid");
    assert!(site.energy().is_none());
}

#[test]
fn a_horizon_that_ends_before_the_hammer_lets_go_bounds_nothing() {
    let p = Params::ChaigneAskenfelt(note(261.63, 4.5, 0.125));
    let mut site =
        ChaigneAskenfeltSite::new(&note(261.63, 4.5, 0.125), f64::from(RATE)).expect("a grid");
    let mut contact = 0;
    while !site.let_go() {
        site.step().expect("a sample");
        contact += 1;
    }
    for points in [contact - 1, contact, contact + 1] {
        let at = tail(&p, RATE, 1, points).expect("a bound or none");
        let lets_go_inside = contact < points;
        assert_eq!(
            at.iter().all(|v| v.is_finite()),
            lets_go_inside,
            "{points} points"
        );
    }
    assert!(tail(&p, RATE, 0, 10).is_err());
}

fn released(p: ChaigneAskenfeltParams, at: f64) -> ChaigneAskenfeltParams {
    ChaigneAskenfeltParams { release: at, ..p }
}

#[test]
fn a_released_note_is_the_held_note_until_the_felt_lands() {
    for unison_count in [1.0, 3.0] {
        let held = ChaigneAskenfeltParams {
            unison_count,
            ..note(261.63, 4.5, 0.125)
        };
        let len = RATE as usize;
        let before = samples(&held, len);
        let after = samples(&released(held, 0.5), len);
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

#[test]
fn a_felted_string_never_gains_energy_once_pressed_and_its_bound_holds() {
    for (f0, damper_k) in [(65.406, 0.0), (261.63, 0.0), (261.63, 5e3), (2093.0, 2e3)] {
        let p = ChaigneAskenfeltParams {
            damper_k,
            ..released(note(f0, 4.5, 0.125), 0.1)
        };
        let fallen = dominates(&p, 1.5);
        assert!(
            fallen < -10.0,
            "f0 {f0}: the felted bound fell only {fallen} dB"
        );
        let mut site = ChaigneAskenfeltSite::new(&p, f64::from(RATE)).expect("a grid");
        let landing = (0.1 * f64::from(RATE)).ceil() as usize;
        for _ in 0..=landing {
            site.step().expect("a sample");
        }
        let mut held = site.energy().expect("one string");
        for k in 0..RATE as usize / 2 {
            site.step().expect("a sample");
            let now = site.energy().expect("one string");
            assert!(
                now <= held * (1.0 + 1e-10),
                "f0 {f0} step {k}: E rose {held} -> {now}"
            );
            held = now;
        }
    }
}
