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
            ("between", "@lfo(t - 0.0123456s)\n"),
            (
                "comb",
                "crop(sample(sin(2*pi*220*t)), 0s, 0.01s) + 0.5*self(t - 0.0123456s)\n",
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

#[test]
fn a_read_between_a_stateful_nodes_samples_refuses() {
    let g = composition();
    for rate in RATES {
        for target in ["between", "comb"] {
            let Err(refused) = render(&g, target, RenderConfig::seconds(rate, 0.05), None) else {
                panic!("{target} at {rate} reads between samples");
            };
            assert_eq!(
                refused.code(),
                "render.off_grid_read",
                "{target}: {refused}"
            );
        }
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
