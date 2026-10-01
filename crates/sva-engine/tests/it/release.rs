// Concern: proves a note's release is an ordinary parameter inf never reaches, and how a solver's parameters may move | Non-concern: rendering until silent | IO: (a composition) -> samples or a refusal

use std::f64::consts::TAU;

use crate::fixtures::graph_of;
use sva_engine::{EngineError, RenderConfig, render};

const RATE: u32 = 44_100;

/// PLAN.md's envelope over one sine: attack, decay to a sustain held until release, then
/// an exponential release from wherever the envelope stood at key-up.
const SYNTH: &str = "a = 0.01\nd = 0.3\ns = 0.6\nr = 0.4\nrelease = inf\n\
    held = crop(min(t/a, 1)*(s + (1 - s)*exp(-max(t - a, 0)/d)), 0s, release)\n\
    at_release = min(release/a, 1)*(s + (1 - s)*exp(-max(release - a, 0)/d))\n\
    sin(2*pi*f0*t)*(held + crop(at_release*exp(-(t - release)/r), release, 60s))\n";

fn samples(files: &[(&str, &str)], root: &str, secs: f64) -> Vec<f64> {
    let g = graph_of(root, files);
    let held = render(&g, root, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{root}: {e}"));
    let id = held.id(root).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

fn refusal(body: &str) -> EngineError {
    let g = graph_of("probe", &[("probe", body)]);
    match render(&g, "probe", RenderConfig::seconds(RATE, 1.0), None) {
        Err(e) => e,
        Ok(_) => panic!("`{body}` rendered"),
    }
}

fn at(i: usize) -> f64 {
    i as f64 / f64::from(RATE)
}

fn sustain(t: f64) -> f64 {
    let (a, d, s) = (0.01, 0.3, 0.6);
    (t / a).min(1.0) * (s + (1.0 - s) * (-(t - a).max(0.0) / d).exp())
}

#[test]
fn a_note_released_at_half_a_second_is_the_held_note_until_then() {
    let files = [
        ("synth", SYNTH),
        ("released", "@synth(t, f0=220, release=0.5)\n"),
        ("held", "@synth(t, f0=220)\n"),
    ];
    let released = samples(&files, "released", 1.0);
    let held = samples(&files, "held", 1.0);
    let key_up = (0.5 * f64::from(RATE)) as usize;
    for i in 0..key_up {
        assert!(
            (released[i] - held[i]).abs() <= 1e-12,
            "sample {i}: released {} against held {}",
            released[i],
            held[i]
        );
    }
    for i in [key_up + 10, (0.7 * f64::from(RATE)) as usize, 44_000] {
        let t = at(i);
        let want = (TAU * 220.0 * t).sin() * sustain(0.5) * (-(t - 0.5) / 0.4).exp();
        assert!(
            (released[i] - want).abs() <= 1e-12,
            "sample {i} after key-up: {} against {want}",
            released[i]
        );
    }
}

#[test]
fn a_note_nobody_releases_holds_its_sustain() {
    let files = [("synth", SYNTH), ("held", "@synth(t, f0=220)\n")];
    let held = samples(&files, "held", 1.0);
    for i in [100, 22_000, 44_000] {
        let t = at(i);
        let want = (TAU * 220.0 * t).sin() * sustain(t);
        assert!(
            (held[i] - want).abs() <= 1e-12,
            "sample {i}: {} against {want}",
            held[i]
        );
    }
}

/// A stored-energy coefficient only jumps, and a varying parameter holds its range at every
/// sample; each is refused by its own code.
#[test]
fn a_solver_parameter_moves_only_as_its_model_allows() {
    for (body, code) in [
        (
            "chaigne_askenfelt(261.63, damper_k=1e3*t)\n",
            "engine.moving_energy_parameter",
        ),
        (
            "chaigne_askenfelt(261.63, damper_r=t - 0.5)\n",
            "samples.argument_out_of_range",
        ),
        (
            "willemsen_bilbao_serafin(196, bow_force=crop(2, 0s, 0.5s))\n",
            "samples.argument_out_of_range",
        ),
    ] {
        let e = refusal(body);
        assert_eq!(e.code(), code, "{body}: {e}");
    }
    let jumps = "chaigne_askenfelt(261.63, damper_k=1e3*step(t - 0.1))\n";
    let g = graph_of("jumps", &[("jumps", jumps)]);
    render(&g, "jumps", RenderConfig::seconds(RATE, 0.2), None).expect("a spring that jumps");
}

/// `t - inf` is `-inf` at every instant, as IEEE arithmetic has it, so a release nobody sets
/// leaves `max(t - release, 0)` zero and its decay at one; a release set decays from there.
#[test]
fn a_decay_past_a_release_at_inf_stays_at_one() {
    let env = "release = inf\nexp(-max(t - release, 0)/0.15)*crop(sin(2*pi*220*t), 0s, 1s)\n";
    let files = [
        ("env", env),
        ("held", "@env\n"),
        ("released", "@env(t, release=0.5)\n"),
    ];
    let held = samples(&files, "held", 1.0);
    let released = samples(&files, "released", 1.0);
    for i in [100, 22_000, 44_000] {
        let t = at(i);
        let tone = (TAU * 220.0 * t).sin();
        assert!((held[i] - tone).abs() <= 1e-12, "sample {i}: {}", held[i]);
        let decay = (-(t - 0.5).max(0.0) / 0.15).exp();
        let want = tone * decay;
        assert!(
            (released[i] - want).abs() <= 1e-12,
            "sample {i}: {} against {want}",
            released[i]
        );
    }
}
