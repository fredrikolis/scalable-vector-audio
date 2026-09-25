// Concern: proves a render until silent ends where every later sample is under half an LSB, or refuses | Non-concern: the release it ends after | IO: (a composition, bits, max) -> a Render or a refusal

mod fixtures;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fixtures::graph_of;
use sva_engine::{
    Cache, EngineError, Entry, Expected, Hash, Label, MemoryCache, Payload, PayloadKind, Render,
    RenderConfig, Silent, render_until_silent,
};
use sva_samples::FilterTrace;

const RATE: u32 = 44_100;

const DEEP: Silent = Silent {
    bits: 24,
    max_secs: 30.0,
};

fn until_silent(files: &[(&str, &str)], root: &str, silent: Silent) -> Result<Render, EngineError> {
    let g = graph_of(root, files);
    render_until_silent(
        &g,
        root,
        RenderConfig::seconds(RATE, 1.0),
        silent,
        None,
        None,
    )
}

fn heard(render: &Render) -> Vec<f64> {
    render
        .buffer(render.root)
        .expect("the root")
        .plane(0)
        .to_vec()
}

fn secs(samples: &[f64]) -> f64 {
    samples.len() as f64 / f64::from(RATE)
}

/// Echo `k` of a 50 ms burst is `0.35^k`: the fifteenth, `1.5e-7`, is the last over `2^-24`,
/// and it ends at `15 * 0.25 + 0.05 = 3.8` seconds.
fn ends_after_the_fifteenth_echo(samples: &[f64], what: &str) {
    let end = secs(samples);
    assert!((3.79..=3.8).contains(&end), "{what} ends at {end}");
    let last = samples.last().expect("a sample").abs();
    assert!(
        last >= DEEP.threshold(),
        "{what}: the last sample {last} is heard"
    );
}

#[test]
fn an_echo_over_a_burst_falls_silent_after_its_fifteenth_echo() {
    let echo = "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n";
    let render = until_silent(&[("echo", echo)], "echo", DEEP).expect("silence is proven");
    ends_after_the_fifteenth_echo(&heard(&render), "the series");
}

#[test]
fn an_echo_over_a_rendered_node_falls_silent_after_its_fifteenth_echo() {
    let files = [
        ("blip", "sample(crop(sin(2*pi*440*t), 0s, 0.05s))\n"),
        ("echo", "@blip + 0.35*self(t - 0.25s)\n"),
    ];
    let render = until_silent(&files, "echo", DEEP).expect("silence is proven");
    ends_after_the_fifteenth_echo(&heard(&render), "the recurrence");
}

/// PLAN.md's envelope over one sine, released at half a second: the release starts at
/// `s + (1 - s)exp(-(0.5 - a)/d)` and falls as `exp(-(t - 0.5)/r)`, under `2^-24` from
/// `0.5 + r ln(level * 2^24)` on, and the sine peaks within one period before that.
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
    let render = until_silent(&files, "note", DEEP).expect("silence is proven");
    let level = 0.6 + 0.4 * (-(0.5 - 0.01) / 0.3f64).exp();
    let silent_from = 0.5 + 0.4 * (level * 2f64.powi(24)).ln();
    let end = secs(&heard(&render));
    assert!(
        (silent_from - 1.0 / 220.0..=silent_from).contains(&end),
        "ends at {end}, silent from {silent_from}"
    );
}

#[test]
fn a_filtered_decay_falls_silent_where_its_input_does() {
    let files = [(
        "tone",
        "lowpass(sample(exp(-t/0.1)*sin(2*pi*220*t)), cutoff=880, q=0.707)\n",
    )];
    let render = until_silent(&files, "tone", DEEP).expect("silence is proven");
    let end = secs(&heard(&render));
    let input = 0.1 * 2f64.powi(24).ln();
    assert!(
        (input - 0.05..=input + 0.05).contains(&end),
        "ends at {end}"
    );
}

#[test]
fn a_held_oscillator_is_never_silent() {
    let refused = until_silent(&[("osc", "sin(2*pi*220*t)\n")], "osc", DEEP)
        .err()
        .expect("a sine is never silent");
    assert_eq!(refused.code(), "engine.never_silent", "{refused}");
}

#[test]
fn a_slow_decay_is_not_silent_by_the_latest_time_asked() {
    let early = Silent {
        bits: 16,
        max_secs: 5.0,
    };
    let refused = until_silent(&[("slow", "exp(-t/10)*sin(2*pi*220*t)\n")], "slow", early)
        .err()
        .expect("a ten-second decay is not silent by five");
    assert_eq!(refused.code(), "engine.not_silent_by", "{refused}");
    assert!(refused.to_string().contains("-4.3 dBFS"), "{refused}");
}

#[test]
fn a_solver_with_no_derived_bound_refuses_rather_than_truncates() {
    let files = [("body", "chaigne_askenfelt(261.63)\n")];
    let refused = until_silent(&files, "body", DEEP)
        .err()
        .expect("no solver bound is derived");
    assert_eq!(refused.code(), "engine.no_tail_bound", "{refused}");
    assert!(
        refused.to_string().contains("chaigne_askenfelt"),
        "{refused}"
    );
}

#[test]
fn a_cropped_solver_is_bounded_by_the_window_it_is_heard_in() {
    let files = [("body", "crop(chaigne_askenfelt(261.63), 0s, 0.2s)\n")];
    let render = until_silent(&files, "body", DEEP).expect("the crop ends it");
    assert!(secs(&heard(&render)) <= 0.2);
}

/// A store that counts the hits it answers, so a warm render is seen to be one.
struct Counted {
    inner: MemoryCache,
    hits: AtomicUsize,
}

impl Cache for Counted {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let found = self.inner.load(key, node, expected);
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
        found
    }
    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        self.inner.peek(key, node, expected)
    }
    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        self.inner.store(key, payload, traces, label);
    }
    fn holds(&self, key: Hash) -> bool {
        self.inner.holds(key)
    }
    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        self.inner.worth_storing(cost, bytes, kind)
    }
    fn sweep(&self) {
        self.inner.sweep();
    }
    fn held_bytes(&self) -> u64 {
        self.inner.held_bytes()
    }
    fn evicted_bytes(&self) -> u64 {
        self.inner.evicted_bytes()
    }
    fn max_bytes(&self) -> u64 {
        self.inner.max_bytes()
    }
}

#[test]
fn a_silent_render_is_remembered_by_its_bits_and_latest_time() {
    let g = graph_of(
        "echo",
        &[(
            "echo",
            "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n",
        )],
    );
    let cache = Counted {
        inner: MemoryCache::holding(64 << 20),
        hits: AtomicUsize::new(0),
    };
    let config = RenderConfig::seconds(RATE, 1.0);
    let cold =
        render_until_silent(&g, "echo", config.clone(), DEEP, Some(&cache), None).expect("cold");
    assert_eq!(cache.hits.load(Ordering::Relaxed), 0);
    let warm =
        render_until_silent(&g, "echo", config.clone(), DEEP, Some(&cache), None).expect("warm");
    assert_eq!(
        cache.hits.load(Ordering::Relaxed),
        2,
        "the length, then the samples"
    );
    assert_eq!(heard(&cold), heard(&warm));
    let shallow = Silent { bits: 16, ..DEEP };
    let other =
        render_until_silent(&g, "echo", config, shallow, Some(&cache), None).expect("sixteen bits");
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
        let refused = until_silent(&[("held", &format!("{held}\n"))], "held", DEEP)
            .err()
            .unwrap_or_else(|| panic!("`{held}` rendered"));
        assert_eq!(refused.code(), "engine.never_silent", "{held}: {refused}");
    }
}
