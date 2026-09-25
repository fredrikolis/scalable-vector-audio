// Concern: proves a key-up time changes no sample before it, and is never where nobody binds it | Non-concern: rendering until silent after it | IO: (a composition) -> a Buffer or a refusal

mod fixtures;

use std::f64::consts::TAU;

use fixtures::graph_of;
use sva_engine::{EngineError, RenderConfig, render};

const RATE: u32 = 44_100;

/// PLAN.md's envelope over one sine: attack, decay to a sustain held until release, then
/// an exponential release from wherever the envelope stood at key-up.
const SYNTH: &str = "a = 0.01\nd = 0.3\ns = 0.6\nr = 0.4\n\
    held = crop(min(t/a, 1)*(s + (1 - s)*exp(-max(t - a, 0)/d)), 0s, release)\n\
    at_release = min(release/a, 1)*(s + (1 - s)*exp(-max(release - a, 0)/d))\n\
    sin(2*pi*f0*t)*(held + crop(at_release*exp(-(t - release)/r), release, 60s))\n";

fn samples(files: &[(&str, &str)], root: &str, secs: f64) -> Vec<f64> {
    let g = graph_of(root, files);
    let held = render(&g, root, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{root}: {e}"));
    let id = held.id(root).expect("the root");
    held.buffer(id).expect("a buffer").plane(0).to_vec()
}

fn refusal(body: &str) -> EngineError {
    let files = [("env", "crop(1, 0s, release)\n"), ("probe", body)];
    let g = graph_of("probe", &files);
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

#[test]
fn a_sampled_note_released_at_half_a_second_is_the_held_note_until_then() {
    let filtered = "lowpass(sample(@synth(t, f0=220, release=release)), cutoff=880, q=0.707)\n";
    let files = [
        ("synth", SYNTH),
        ("tone", filtered),
        ("released", "@tone(t, release=0.5)\n"),
        ("held", "@tone(t)\n"),
    ];
    let released = samples(&files, "released", 1.0);
    let held = samples(&files, "held", 1.0);
    let key_up = (0.5 * f64::from(RATE)) as usize;
    assert_eq!(released[..key_up], held[..key_up]);
    assert_ne!(released[key_up + 4410], held[key_up + 4410]);
}

#[test]
fn a_sample_before_release_that_reads_it_is_refused_by_name() {
    for (body, term) in [
        ("sin(2*pi*release*t)\n", "2*pi*release"),
        (
            "crop(sin(2*pi*t), 0s, release, fall=0.1s)\n",
            "crop(sin(2*pi*t), 0s, release, fall=0.1)",
        ),
        ("@env(t + 0.1s)\n", "@env(t + 0.1s)"),
        ("crop(1, 0s, 2*release)\n", "2*release"),
        (
            "fourier(crop(sin(2*pi*t), 0s, release))\n",
            "fourier(crop(sin(2*pi*t), 0s, release))",
        ),
    ] {
        let e = refusal(body);
        assert_eq!(e.code(), "engine.release_not_causal", "{body}");
        assert!(e.to_string().contains(&format!("`{term}`")), "{body}: {e}");
    }
}

#[test]
fn a_past_read_of_a_releasing_node_is_causal() {
    let files = [
        ("env", "crop(1, 0s, release)\n"),
        ("late", "@env(t - 0.1s, release=0.5)\n"),
    ];
    let late = samples(&files, "late", 1.0);
    assert_eq!(late[(0.05 * f64::from(RATE)) as usize], 0.0);
    assert_eq!(late[(0.3 * f64::from(RATE)) as usize], 1.0);
    assert_eq!(late[(0.7 * f64::from(RATE)) as usize], 0.0);
}

const STRING: &str = "chaigne_askenfelt(f0, release=release)\n";

#[test]
fn a_felted_string_is_the_held_string_until_its_felt_lands() {
    let files = [
        ("string", STRING),
        ("released", "@string(t, f0=261.63, release=0.5)\n"),
        ("held", "@string(t, f0=261.63)\n"),
        ("plain", "chaigne_askenfelt(261.63)\n"),
    ];
    let released = samples(&files, "released", 1.0);
    let held = samples(&files, "held", 1.0);
    assert_eq!(
        held,
        samples(&files, "plain", 1.0),
        "an unbound release never lands"
    );
    let landing = (0.5 * f64::from(RATE)).ceil() as usize;
    assert_eq!(released[..=landing], held[..=landing]);
    assert_ne!(released[landing + 1..], held[landing + 1..]);
}

#[test]
fn a_solver_reads_release_only_as_its_own_landing() {
    for body in [
        "chaigne_askenfelt(261.63, damper_r=release)\n",
        "chaigne_askenfelt(261.63, release=release + 0.1)\n",
        "willemsen_bilbao_serafin(261.63, bow_vel=release)\n",
    ] {
        let refused = refusal(body);
        assert_eq!(
            refused.code(),
            "engine.release_not_causal",
            "{body}: {refused}"
        );
    }
}
