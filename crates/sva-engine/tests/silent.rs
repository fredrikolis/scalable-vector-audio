// Concern: proves an open range ends at the first frame every later one is proven quiet from, or refuses | Non-concern: the release it ends after | IO: (a composition, level) -> a Render or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Cache, EngineError, Render, RenderConfig, Until, render};

const RATE: u32 = 44_100;

/// Every frame's RMS under 24 bits of full scale.
fn deep() -> f64 {
    2f64.powi(-24)
}

fn quiet(level: f64, limit: f64) -> RenderConfig {
    RenderConfig {
        until: Some(Until::quiet(level)),
        proof_limit_secs: limit,
        ..RenderConfig::at(RATE)
    }
}

fn until_quiet(files: &[(&str, &str)], root: &str, level: f64) -> Result<Render, EngineError> {
    let g = graph_of(root, files);
    render(&g, root, quiet(level, 30.0), None)
}

fn heard(render: &Render) -> Vec<f64> {
    render
        .output(render.root)
        .expect("the root")
        .plane(0)
        .to_vec()
}

fn secs(samples: &[f64]) -> f64 {
    samples.len() as f64 / f64::from(RATE)
}

/// The RMS of each 50 ms frame, as the `envelope` representation frames it.
fn frames(samples: &[f64]) -> Vec<f64> {
    samples
        .chunks(2_205)
        .map(|f| (f.iter().map(|v| v * v).sum::<f64>() / 2_205.0).sqrt())
        .collect()
}

/// Echo `k` of a 50 ms burst is `0.35^k`: the fifteenth, `1.5e-7`, is the last over `2^-24`
/// in RMS, and it ends at `15 * 0.25 + 0.05 = 3.8` seconds, a frame boundary.
fn ends_after_the_fifteenth_echo(samples: &[f64], what: &str) {
    let end = secs(samples);
    assert!((3.79..=3.8).contains(&end), "{what} ends at {end}");
    let last = frames(samples).last().copied().expect("a frame");
    assert!(last >= deep(), "{what}: the last frame {last} is heard");
}

#[test]
fn an_echo_over_a_burst_falls_silent_after_its_fifteenth_echo() {
    let echo = "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n";
    let render = until_quiet(&[("echo", echo)], "echo", deep()).expect("silence is proven");
    ends_after_the_fifteenth_echo(&heard(&render), "the series");
}

#[test]
fn an_echo_over_a_rendered_node_falls_silent_after_its_fifteenth_echo() {
    let files = [
        ("blip", "sample(crop(sin(2*pi*440*t), 0s, 0.05s))\n"),
        ("echo", "@blip + 0.35*self(t - 0.25s)\n"),
    ];
    let render = until_quiet(&files, "echo", deep()).expect("silence is proven");
    ends_after_the_fifteenth_echo(&heard(&render), "the recurrence");
}

/// PLAN.md's envelope over one sine, released at half a second: the release starts at
/// `s + (1 - s)exp(-(0.5 - a)/d)` and falls as `exp(-(t - 0.5)/r)`, its RMS under `2^-24` from
/// `0.5 + r ln(level * 2^24 / sqrt 2)` on, and the first quiet frame starts within a frame of it.
#[test]
fn an_envelope_released_at_half_a_second_falls_silent_where_its_release_crosses_24_bits() {
    let synth = "a = 0.01\nd = 0.3\ns = 0.6\nr = 0.4\n\
        held = crop(min(t/a, 1)*(s + (1 - s)*exp(-max(t - a, 0)/d)), 0s, release)\n\
        at_release = min(release/a, 1)*(s + (1 - s)*exp(-max(release - a, 0)/d))\n\
        sin(2*pi*f0*t)*(held + crop(at_release*exp(-(t - release)/r), release, 60s))\n";
    let files = [
        ("synth", synth),
        ("note", "@synth(t, f0=220, release=0.5)\n"),
    ];
    let render = until_quiet(&files, "note", deep()).expect("silence is proven");
    let level = 0.6 + 0.4 * (-(0.5 - 0.01) / 0.3f64).exp();
    let silent_from = 0.5 + 0.4 * (level * 2f64.powi(24) / 2f64.sqrt()).ln();
    let end = secs(&heard(&render));
    assert!(
        (silent_from - 0.05..=silent_from + 0.05).contains(&end),
        "ends at {end}, silent from {silent_from}"
    );
}

#[test]
fn a_filtered_decay_falls_silent_where_its_input_does() {
    let files = [(
        "tone",
        "lowpass(sample(exp(-t/0.1)*sin(2*pi*220*t)), cutoff=880, q=0.707)\n",
    )];
    let render = until_quiet(&files, "tone", deep()).expect("silence is proven");
    let end = secs(&heard(&render));
    let input = 0.1 * (2f64.powi(24) / 2f64.sqrt()).ln();
    assert!((input - 0.1..=input + 0.1).contains(&end), "ends at {end}");
}

#[test]
fn a_held_oscillator_is_never_silent() {
    let refused = until_quiet(&[("osc", "sin(2*pi*220*t)\n")], "osc", deep())
        .err()
        .expect("a sine is never silent");
    assert_eq!(refused.code(), "engine.never_silent", "{refused}");
}

#[test]
fn a_slow_decay_is_not_silent_by_the_latest_time_asked() {
    let g = graph_of("slow", &[("slow", "exp(-t/10)*sin(2*pi*220*t)\n")]);
    let refused = render(&g, "slow", quiet(2f64.powi(-16), 5.0), None)
        .err()
        .expect("a ten-second decay is not silent by five");
    assert_eq!(refused.code(), "engine.not_silent_by", "{refused}");
    assert!(refused.to_string().contains("-4.3 dBFS"), "{refused}");
}

/// A damped high string, so the ring-down fits a short render.
const STRING: &str = "chaigne_askenfelt(1046.5, damp_dc=20)\n";

/// Ends where silence at 16 bits is proven, the fixed render's prefix, and every frame after
/// quiet.
fn rings_down(body: &str) -> usize {
    let loud = 2f64.powi(-16);
    let g = graph_of("body", &[("body", body)]);
    let render = render(&g, "body", quiet(loud, 10.0), None).expect("a single string rings down");
    let samples = heard(&render);
    let end = samples.len();
    let last = frames(&samples).last().copied().expect("a frame");
    assert!(last >= loud, "{body}: the last frame {last} is not heard");
    let config = RenderConfig::seconds(RATE, secs(&samples) + 1.0);
    let longer = heard(&sva_engine::render(&g, "body", config, None).expect("a fixed render"));
    assert_eq!(
        &longer[..end],
        &samples[..],
        "{body}: not the fixed render's prefix"
    );
    let after = frames(&longer[end..]).into_iter().fold(0.0f64, f64::max);
    assert!(after < loud, "{body}: {after} heard after the end");
    end
}

#[test]
fn a_struck_string_ends_where_its_modes_prove_every_later_sample_silent() {
    rings_down(STRING);
}

#[test]
fn a_felted_string_ends_where_its_energy_proves_every_later_sample_silent() {
    let held = rings_down(STRING);
    let felted = rings_down("chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
    assert!(felted < held, "felted at {felted}, held at {held}");
}

#[test]
fn a_loud_felted_string_follows_its_bound_down_past_the_floor() {
    let quiet = rings_down("chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
    let loud = rings_down("1000*chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
    assert!(loud > quiet, "loud at {loud}, quiet at {quiet}");
}

#[test]
fn a_felted_unison_on_its_bridge_ends_where_its_energy_proves_every_later_sample_silent() {
    rings_down(
        "chaigne_askenfelt(261.63, unison_count=3, bridge_mass=1, bridge_coupling=100, \
         release=0.1)\n",
    );
}

#[test]
fn a_solver_with_no_derived_bound_refuses_rather_than_truncates() {
    let files = [("body", "chaigne_doutaut(440)\n")];
    let refused = until_quiet(&files, "body", deep())
        .err()
        .expect("no bar bound is derived");
    assert_eq!(refused.code(), "engine.no_tail_bound", "{refused}");
    assert!(refused.to_string().contains("chaigne_doutaut"), "{refused}");
}

#[test]
fn a_cropped_solver_is_bounded_by_the_window_it_is_heard_in() {
    let files = [("body", "crop(chaigne_askenfelt(261.63), 0s, 0.2s)\n")];
    let render = until_quiet(&files, "body", deep()).expect("the crop ends it");
    assert!(secs(&heard(&render)) <= 0.2);
}

/// A warm render runs no proof: where it stopped is stored beside the samples, under the
/// condition that stopped it.
#[test]
fn where_a_render_stopped_is_remembered_by_its_condition() {
    let g = graph_of(
        "echo",
        &[(
            "echo",
            "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n",
        )],
    );
    let cache = Cache::holding(64 << 20);
    let config = quiet(deep(), 30.0);
    let cold = render(&g, "echo", config.clone(), Some(&cache)).expect("cold");
    assert!(cold.proofs > 0, "the cold render proves where it ends");
    let warm = render(&g, "echo", config, Some(&cache)).expect("warm");
    assert_eq!(warm.proofs, 0, "the warm one reads where it ended");
    assert_eq!(heard(&cold), heard(&warm));
    let other = render(&g, "echo", quiet(2f64.powi(-16), 30.0), Some(&cache)).expect("16 bits");
    assert!(heard(&other).len() < heard(&warm).len());
}

/// A level held forever is proven through what keeps one: a rectifier, a saturation, a
/// gain, a buffer, a sum with something that decays, and a bound it never crosses.
#[test]
fn a_level_held_through_a_map_a_gain_or_a_sum_is_never_silent() {
    for held in [
        "abs(sin(2*pi*220*t))",
        "tanh(4*sample(sin(2*pi*220*t)))",
        "0.5*sample(sin(2*pi*220*t))",
        "sample(sin(2*pi*220*t)) + sample(exp(-t)*sin(2*pi*330*t))",
        "max(sin(2*pi*440*t), 0.5)",
        "min(sample(exp(-t)*sin(2*pi*440*t)), -0.25)",
    ] {
        let refused = until_quiet(&[("held", &format!("{held}\n"))], "held", deep())
            .err()
            .unwrap_or_else(|| panic!("`{held}` rendered"));
        assert_eq!(refused.code(), "engine.never_silent", "{held}: {refused}");
    }
}
