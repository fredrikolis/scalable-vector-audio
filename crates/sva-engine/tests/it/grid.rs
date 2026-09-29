// Concern: proves each node is sampled at the render's rate, at the instants its reader asks for, or refuses | Non-concern: a stream's blocks (stream.rs) | IO: (a composition, rate) -> samples
use std::f64::consts::TAU;

use crate::fixtures::graph_of;
use sva_engine::{Cache, CachePolicy, RenderConfig, render};

const RATES: [u32; 4] = [8_000, 44_100, 48_000, 96_000];

fn composition() -> sva_ast::Graph {
    graph_of(
        "grid",
        &[
            ("tone", "sin(2*pi*220*t)\n"),
            ("lfo", "lowpass(sample(0.001*sin(2*pi*3*t)), cutoff=100)\n"),
            ("vibrato", "@tone(t + @lfo)\n"),
            ("scaled", "@tone(0.37*t - 0.0123456s)\n"),
            ("decay", "crop(1, 0s, 1sp) + 0.5*self[idx(t) - 1]\n"),
            (
                "mix",
                "lowpass(sample(0.3*saw(220*t)), cutoff=900) + @vibrato + rand(t - t % 0.01s, \
                 seed=3) + @decay\n",
            ),
        ],
    )
}

fn plane(held: &sva_engine::Render, node: &str) -> Vec<f64> {
    let id = held.id(node).unwrap_or_else(|| panic!("{node} is held"));
    held.output(id).expect("a buffer").plane(0).to_vec()
}

#[test]
fn every_node_is_held_at_the_rate_asked_for() {
    let g = composition();
    for rate in RATES {
        let held = render(&g, "mix", RenderConfig::seconds(rate, 0.05), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        assert_eq!(plane(&held, "mix").len(), rate as usize / 20, "{rate}");
        for (id, buffer) in &held.buffers {
            assert_eq!(buffer.rate, rate, "{rate}: {}", held.tys.name(*id));
        }
    }
}

/// A crop's end is exact at every rate: `[0, 0.8s)` holds `0.8*rate` samples, however
/// `0.8*48000` rounds in floating point.
#[test]
fn an_open_range_ends_at_the_crops_exact_sample_count() {
    let g = graph_of(
        "grid-crop-end",
        &[("cut", "crop(sample(sin(2*pi*100*t)), 0s, 0.8s)\n")],
    );
    for rate in RATES {
        let held = render(&g, "cut", RenderConfig::at(rate), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        assert_eq!(plane(&held, "cut").len(), rate as usize * 4 / 5, "{rate}");
    }
}

/// `self[idx(t) - 1]` is the sample before, whatever the rate: an impulse halves each step.
#[test]
fn an_indexed_loop_steps_at_the_render_rate() {
    let g = composition();
    for rate in RATES {
        let held = render(&g, "decay", RenderConfig::seconds(rate, 0.001), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        let decay = plane(&held, "decay");
        for (n, v) in decay.iter().enumerate().take(8) {
            assert_eq!(*v, 0.5f64.powi(n as i32), "{rate}: sample {n}");
        }
    }
}

/// At a scaled, shifted time, and at a time a filter moves.
#[test]
fn a_formula_read_at_any_instant_is_exact() {
    let g = composition();
    for rate in RATES {
        let config = RenderConfig::seconds(rate, 0.02);
        let scaled = render(&g, "scaled", config.clone(), None).expect("a scaled read");
        for (n, v) in plane(&scaled, "scaled").iter().enumerate() {
            let at = 0.37 * n as f64 / f64::from(rate) - 0.012_345_6;
            assert!((v - (TAU * 220.0 * at).sin()).abs() < 1e-9, "{rate}: {n}");
        }
        let held = render(&g, "vibrato", config, None).expect("a moving read");
        let lfo = held
            .buffers
            .get(&held.id("lfo").expect("the lfo"))
            .expect("a filter is held")
            .plane(0)
            .to_vec();
        for (n, v) in plane(&held, "vibrato").iter().enumerate() {
            let at = n as f64 / f64::from(rate) + lfo[n];
            assert!((v - (TAU * 220.0 * at).sin()).abs() < 1e-12, "{rate}: {n}");
        }
    }
}

fn close(got: &[f64], want: &[f64], at: &str) {
    assert!(got.iter().any(|v| *v != 0.0), "{at}: silence tests nothing");
    assert_eq!(got.len(), want.len(), "{at}");
    for (n, (a, b)) in got.iter().zip(want).enumerate() {
        assert!((a - b).abs() < 1e-9, "{at}: sample {n} is {a}, not {b}");
    }
}

/// `@kick(t - d)` steps the kick on the grid `d` shifts it to: the same as writing the shift
/// into what it filters, from where that starts.
#[test]
fn a_stateful_node_read_between_its_steps_runs_on_the_grid_the_read_shifts_it_to() {
    let g = graph_of(
        "shifted",
        &[
            (
                "kick",
                "lowpass(crop(sample(sin(2*pi*55*t)), 0s, 0.05s), cutoff=900, q=0.8)\n",
            ),
            ("late", "@kick(t - 0.1234s)\n"),
            (
                "written",
                "lowpass(crop(sample(sin(2*pi*55*(t - 0.1234s))), 0.1234s, 0.1734s), \
                 cutoff=900, q=0.8)\n",
            ),
        ],
    );
    for rate in RATES {
        let config = RenderConfig::seconds(rate, 0.3);
        let at = |target: &str| {
            let held = render(&g, target, config.clone(), None)
                .unwrap_or_else(|e| panic!("{target} at {rate}: {e}"));
            plane(&held, target)
        };
        close(&at("late"), &at("written"), &format!("{rate}"));
    }
}

/// `@x(0.5*t)` steps x at half the step: x rendered at twice the rate, sample for sample.
#[test]
fn a_stateful_node_read_at_half_speed_steps_at_half_the_step() {
    let g = graph_of(
        "slowed",
        &[
            (
                "filtered",
                "lowpass(crop(sample(0.3*saw(220*t)), 0s, 0.05s), cutoff=900, q=0.8)\n",
            ),
            ("slow", "@filtered(0.5*t)\n"),
        ],
    );
    for rate in [8_000, 22_050, 24_000, 48_000] {
        let slow = render(&g, "slow", RenderConfig::seconds(rate, 0.05), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        let fast = render(&g, "filtered", RenderConfig::seconds(2 * rate, 0.025), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        close(
            &plane(&slow, "slow"),
            &plane(&fast, "filtered"),
            &format!("{rate}"),
        );
    }
}

/// A stateful node at a time that moves has no step to read there; typing says which
/// construct holds the state and offers the nearest step.
#[test]
fn a_stateful_node_read_at_a_moving_time_refuses_at_typing() {
    let g = graph_of(
        "warped",
        &[
            ("pad", "lowpass(sample(0.3*saw(220*t)), cutoff=900)\n"),
            ("d", "0.001*sin(2*pi*3*t)\n"),
            ("warped", "@pad(t - @d)\n"),
        ],
    );
    let Err(refused) = sva_engine::types(&g, "warped") else {
        panic!("a filter has no value at a time that moves");
    };
    assert_eq!(refused.code(), "type.stateful_warp", "{refused}");
    let said = refused.to_string();
    for part in ["`pad`", "`lowpass(…)`", "idx("] {
        assert!(said.contains(part), "{part} in {said}");
    }
}

#[test]
fn a_store_answers_a_stepped_node_only_at_its_own_rate() {
    let g = composition();
    let cache = Cache::new();
    let at = |rate: u32, cache: Option<&Cache>| {
        let config = RenderConfig {
            cache_policy: Some(CachePolicy::All),
            ..RenderConfig::seconds(rate, 0.05)
        };
        plane(&render(&g, "mix", config, cache).expect("a render"), "mix")
    };
    at(44_100, Some(&cache));
    assert_eq!(at(48_000, Some(&cache)), at(48_000, None));
}
