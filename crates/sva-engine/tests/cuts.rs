// Concern: proves each node is cut where bound times gain crosses the decay floor, all within it, or refuses | Non-concern: the release it follows | IO: (a composition) -> a Render or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Cmp, EngineError, Range, Render, RenderConfig, Term, Until, render};

const RATE: u32 = 44_100;

/// 24 bits of full scale, the resolution a render writes by default.
fn deep() -> f64 {
    2f64.powi(-24)
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

/// An open render at 24 bits ends where its root is cut, and against a render at 52 bits,
/// cut far later, it is within the floor before its end and the finer one is under the floor
/// after it. Returns where it ends, in seconds.
fn cut_within_the_floor(files: &[(&str, &str)], root: &str) -> f64 {
    let g = graph_of(root, files);
    let cut = heard(&render(&g, root, RenderConfig::at(RATE), None).expect("an end is cut"));
    let fine = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(cut.len() as i64 + i64::from(RATE)),
        },
        ..RenderConfig::at(RATE)
    };
    let mut fine = fine;
    fine.profile.precision_bits = 52;
    let exact = heard(&render(&g, root, fine, None).expect("a finer render"));
    for (n, (a, b)) in cut.iter().zip(&exact).enumerate() {
        assert!((a - b).abs() <= deep(), "{root} at {n}: {a} against {b}");
    }
    let after = exact[cut.len()..]
        .iter()
        .fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(after <= deep(), "{root}: {after} after its end");
    secs(&cut)
}

#[test]
fn an_echo_over_a_burst_is_cut_within_the_floor() {
    let echo = "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n";
    let end = cut_within_the_floor(&[("echo", echo)], "echo");
    assert!(end < 6.0, "the sixteenth echo is under the floor: {end}");
}

#[test]
fn an_echo_over_a_rendered_node_is_cut_within_the_floor() {
    let files = [
        ("blip", "sample(crop(sin(2*pi*440*t), 0s, 0.05s))\n"),
        ("echo", "@blip + 0.35*self(t - 0.25s)\n"),
    ];
    cut_within_the_floor(&files, "echo");
}

/// PLAN.md's envelope over one sine, released at half a second: its release falls as
/// `exp(-(t - 0.5)/r)` and crosses 24 bits at `0.5 + r ln(level * 2^24)`.
#[test]
fn an_envelope_released_at_half_a_second_ends_where_its_release_crosses_the_floor() {
    let synth = "a = 0.01\nd = 0.3\ns = 0.6\nr = 0.4\n\
        held = crop(min(t/a, 1)*(s + (1 - s)*exp(-max(t - a, 0)/d)), 0s, release)\n\
        at_release = min(release/a, 1)*(s + (1 - s)*exp(-max(release - a, 0)/d))\n\
        sin(2*pi*f0*t)*(held + crop(at_release*exp(-(t - release)/r), release, 60s))\n";
    let files = [
        ("synth", synth),
        ("note", "@synth(t, f0=220, release=0.5)\n"),
    ];
    let end = cut_within_the_floor(&files, "note");
    let level = 0.6 + 0.4 * (-(0.5 - 0.01) / 0.3f64).exp();
    let crosses = 0.5 + 0.4 * (level * 2f64.powi(24)).ln();
    assert!(
        (crosses - 0.05..=crosses + 0.5).contains(&end),
        "ends at {end}, under the floor from {crosses}"
    );
}

/// The release nearly every envelope is written with, uncropped: its argument grows without
/// bound, yet its rounding stays relative to its tiny value, so it ends where it crosses.
#[test]
fn an_uncropped_release_after_a_duration_ends_where_it_crosses_the_floor() {
    let files = [("note", "sin(2*pi*220*t)*exp(-max(0, t - 0.5)/0.1)\n")];
    let end = cut_within_the_floor(&files, "note");
    let crosses = 0.5 + 0.1 * 2f64.powi(24).ln();
    assert!(
        (crosses - 0.05..=crosses + 0.1).contains(&end),
        "ends at {end}, under the floor from {crosses}"
    );
}

#[test]
fn a_filtered_decay_is_cut_within_the_floor() {
    let files = [(
        "tone",
        "lowpass(sample(exp(-t/0.1)*sin(2*pi*220*t)), cutoff=880, q=0.707)\n",
    )];
    cut_within_the_floor(&files, "tone");
}

/// A decaying node read at a hundredth is cut where a hundredth of its bound crosses its
/// share of the floor: earlier than where the node alone does, by `0.1 ln 100` less what a
/// second node's share takes back, `0.1 ln 2`.
#[test]
fn a_node_under_a_downstream_gain_is_cut_where_its_share_at_the_output_crosses_the_floor() {
    let decay = "sample(exp(-t/0.1)*sin(2*pi*220*t))\n";
    let cut_at = |root_body: &str| {
        let g = graph_of("gain", &[("decay", decay), ("root", root_body)]);
        let held = render(&g, "root", RenderConfig::at(RATE), None).expect("both are cut");
        let at = held
            .cuts
            .cut
            .iter()
            .find(|c| c.node == "decay")
            .unwrap_or_else(|| panic!("`decay` is cut under `{root_body}`: {:?}", held.cuts))
            .at;
        at as f64 / f64::from(RATE)
    };
    let alone = cut_at("@decay\n");
    let quiet = cut_at("0.01*@decay\n");
    let earlier = alone - quiet;
    assert!(
        (0.1 * 50f64.ln() - 0.02..=0.1 * 100f64.ln() + 0.02).contains(&earlier),
        "cut {earlier} s earlier at a hundredth"
    );
    cut_within_the_floor(&[("decay", decay), ("root", "0.01*@decay\n")], "root");
}

#[test]
fn a_held_oscillator_never_ends() {
    let g = graph_of("osc", &[("osc", "sin(2*pi*220*t)\n")]);
    let refused = render(&g, "osc", RenderConfig::at(RATE), None)
        .err()
        .expect("a sine is never cut");
    assert_eq!(refused.code(), "render.never_ends", "{refused}");
}

/// The bound looks further, doubling, only while the budget holds.
#[test]
fn a_slow_decay_past_the_budget_has_no_end() {
    let g = graph_of(
        "slow",
        &[(
            "slow",
            "lowpass(sample(exp(-t/1000)*sin(2*pi*220*t)), cutoff=8000, q=0.707)\n",
        )],
    );
    let config = RenderConfig {
        flop_budget: 1_000_000,
        ..RenderConfig::at(RATE)
    };
    let refused = render(&g, "slow", config, None)
        .err()
        .expect("a thousand-second decay is not under the floor where the budget ends");
    assert_eq!(refused.code(), "render.no_end", "{refused}");
}

/// A damped high string, so the ring-down fits a short render.
const STRING: &str = "chaigne_askenfelt(1046.5, damp_dc=20)\n";

/// Ends where the root is cut at a 16-bit floor, the fixed render's prefix.
fn rings_down(body: &str) -> usize {
    let loud = 2f64.powi(-16);
    let g = graph_of("body", &[("body", body)]);
    let config = RenderConfig {
        decay_floor: Some(loud),
        ..RenderConfig::at(RATE)
    };
    let samples = heard(&render(&g, "body", config.clone(), None).expect("a string rings down"));
    let end = samples.len();
    let longer = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(end as i64 + i64::from(RATE)),
        },
        ..config
    };
    let longer = heard(&render(&g, "body", longer, None).expect("a fixed render"));
    assert_eq!(
        &longer[..end],
        &samples[..],
        "{body}: not the fixed render's prefix"
    );
    end
}

#[test]
fn a_struck_string_ends_where_its_modes_bound_it_under_the_floor() {
    rings_down(STRING);
}

#[test]
fn a_felted_string_ends_where_its_energy_bounds_it_under_the_floor() {
    rings_down("chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
}

#[test]
fn a_loud_felted_string_follows_its_bound_down_past_the_floor() {
    let quiet = rings_down("chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
    let loud = rings_down("1000*chaigne_askenfelt(1046.5, damp_dc=20, release=0.1)\n");
    assert!(loud > quiet, "loud at {loud}, quiet at {quiet}");
}

#[test]
fn a_felted_unison_on_its_bridge_ends_where_its_energy_bounds_it_under_the_floor() {
    rings_down(
        "chaigne_askenfelt(261.63, unison_count=3, bridge_mass=1, bridge_coupling=100, \
         release=0.1)\n",
    );
}

#[test]
fn a_solver_with_no_derived_bound_refuses_rather_than_truncates() {
    let g = graph_of("body", &[("body", "chaigne_doutaut(440)\n")]);
    let refused = render(&g, "body", RenderConfig::at(RATE), None)
        .err()
        .expect("no bar bound is derived");
    assert_eq!(refused.code(), "render.no_bound", "{refused}");
    assert!(refused.to_string().contains("chaigne_doutaut"), "{refused}");
}

#[test]
fn a_cropped_solver_ends_at_its_window() {
    let g = graph_of(
        "body",
        &[("body", "crop(chaigne_askenfelt(261.63), 0s, 0.2s)\n")],
    );
    let held = render(&g, "body", RenderConfig::at(RATE), None).expect("the crop ends it");
    assert!(secs(&heard(&held)) <= 0.2);
}

/// Past a crop's end the root is exactly zero, a crop around a loop's feedback included, so
/// an open range ends there; an endless root refuses.
#[test]
fn an_open_range_ends_where_the_root_support_does() {
    let files = [
        ("cut", "crop(sin(2*pi*log(max(2, 1))*100*t), 0s, 1s)\n"),
        (
            "fed",
            "crop(0.5*(sample(sin(2*pi*100*t)) + 0.3*self(t - 1sp)), 0s, 1s)\n",
        ),
        ("held", "sin(2*pi*100*t)\n"),
    ];
    let g = graph_of("cut", &files);
    for root in ["cut", "fed"] {
        let cut = render(&g, root, RenderConfig::at(RATE), None)
            .unwrap_or_else(|e| panic!("{root}: {e}"));
        let end = secs(&heard(&cut));
        assert!((1.0..=1.0001).contains(&end), "{root} ends at {end}");
    }
    let refused = render(&g, "held", RenderConfig::at(RATE), None)
        .err()
        .expect("nothing ends an endless root");
    assert_eq!(refused.code(), "render.never_ends", "{refused}");
}

/// A level held forever is proven through what keeps one: a rectifier, a saturation, a
/// gain, a buffer, a sum with something that decays, and a bound it never crosses.
#[test]
fn a_level_held_through_a_map_a_gain_or_a_sum_never_ends() {
    for held in [
        "abs(sin(2*pi*220*t))",
        "tanh(4*sample(sin(2*pi*220*t)))",
        "0.5*sample(sin(2*pi*220*t))",
        "sample(sin(2*pi*220*t)) + sample(exp(-t)*sin(2*pi*330*t))",
        "max(sin(2*pi*440*t), 0.5)",
        "min(sample(exp(-t)*sin(2*pi*440*t)), -0.25)",
    ] {
        let g = graph_of("held", &[("held", &format!("{held}\n"))]);
        let refused = render(&g, "held", RenderConfig::at(RATE), None)
            .err()
            .unwrap_or_else(|| panic!("`{held}` rendered"));
        assert_eq!(refused.code(), "render.never_ends", "{held}: {refused}");
    }
}

/// `until` stops a render at the first frame whose level holds it, and computes only as far
/// as the pass that found it: less than the whole interval.
#[test]
fn until_stops_a_render_at_the_first_frame_it_holds_at() {
    let g = graph_of("fade", &[("fade", "sample(exp(-t/0.2)*sin(2*pi*220*t))\n")]);
    let interval = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(10 * i64::from(RATE)),
        },
        ..RenderConfig::at(RATE)
    };
    let whole = render(&g, "fade", interval.clone(), None).expect("the whole interval");
    let frame = 2_205;
    let level = 10f64.powf(-40.0 / 20.0);
    let first = heard(&whole)
        .chunks(frame)
        .position(|f| (f.iter().map(|v| v * v).sum::<f64>() / f.len() as f64).sqrt() < level)
        .expect("a frame under -40 dB");
    let until = Until::Holds(Term::Envelope, Cmp::Lt, Term::Number(level));
    let stopped = RenderConfig {
        until: Some(until),
        ..interval
    };
    let stopped = render(&g, "fade", stopped, None).expect("a stopped render");
    assert_eq!(heard(&stopped).len(), first * frame);
    assert_eq!(heard(&stopped)[..], heard(&whole)[..first * frame]);
    assert!(stopped.work().priced_flops < whole.work().priced_flops);
}

/// A floor under the resolution the render writes is no floor it can hold.
#[test]
fn a_decay_floor_under_the_resolution_refuses() {
    let g = graph_of("osc", &[("osc", "crop(sin(2*pi*220*t), 0s, 1s)\n")]);
    let config = RenderConfig {
        decay_floor: Some(2f64.powi(-30)),
        ..RenderConfig::at(RATE)
    };
    let refused: EngineError = render(&g, "osc", config, None)
        .err()
        .expect("under 24 bits");
    assert_eq!(refused.code(), "render.floor_below_resolution", "{refused}");
}
