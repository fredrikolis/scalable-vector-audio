// Concern: proves a stream's blocks are the samples a whole render writes, bit for bit, or it refuses | Non-concern: one machine's own blocks (sva-samples) | IO: (a composition, bindings) -> blocks

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{RenderConfig, Silent, Stream, StreamConfig, render, render_until_silent};

const RATE: u32 = 44_100;

const PIANO3: &str = "0.0014822 * chaigne_askenfelt(f0, vel=vel, b=max(1.4e-4, \
    4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, hammer_k=2e10*max(1, \
    pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) - \
    0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

const ECHO: &str = "feedback = 0.35\nx + feedback*self(t - 0.25s)\n";

fn composition() -> Graph {
    graph_of(
        "stream",
        &[
            ("piano3", PIANO3),
            ("echo", ECHO),
            ("note", "@piano3(t, f0=261.63, vel=4.5)\n"),
            ("echoed", "@echo(t, x=@note)\n"),
            (
                "chain",
                "lowpass(highpass(@note, cutoff=180, q=0.7), cutoff=2400, q=1.3)\n",
            ),
            ("saw", "0.3*saw(220*t)\n"),
            ("sawed", "lowpass(sample(@saw), cutoff=900, q=0.8)\n"),
            ("spectrum", "exp(0 - pow(f/300, 2))\n"),
            ("ahead", "@note(t + 0.01s)\n"),
            ("string", "chaigne_askenfelt(f0, release=release)\n"),
            ("damped", "@string(t, f0=523.25, release=0.05)\n"),
            ("held", "sin(2*pi*220*t)\n"),
        ],
    )
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = StreamConfig {
        rate: RATE,
        block,
        silent: None,
    };
    let mut stream = Stream::open(g, target, &[], config).unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::with_capacity(samples + block);
    while out.len() < samples {
        let block = stream.next_block().unwrap_or_else(|e| panic!("{e}"));
        out.extend_from_slice(block.expect("a stream with no end").plane(0));
    }
    out.truncate(samples);
    out
}

fn whole(g: &Graph, target: &str, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(RATE);
    let held = render(g, target, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.buffer(id).expect("a buffer").plane(0).to_vec()
}

fn sounds(samples: &[f64]) {
    assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
}

#[test]
fn a_streamed_piano_note_is_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = 9_000;
    let want = whole(&g, "note", samples);
    sounds(&want);
    for block in [128, 1_000] {
        assert_eq!(
            streamed(&g, "note", block, samples),
            want,
            "blocks of {block}"
        );
    }
}

/// The echo reads its own output a quarter second back, across many blocks.
#[test]
fn an_echo_over_a_streamed_note_is_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = 16_000;
    let want = whole(&g, "echoed", samples);
    assert_ne!(want[11_100..], whole(&g, "note", samples)[11_100..]);
    assert_eq!(streamed(&g, "echoed", 1_024, samples), want);
}

#[test]
fn a_biquad_chain_over_a_streamed_note_is_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = 6_000;
    let want = whole(&g, "chain", samples);
    sounds(&want);
    assert_eq!(streamed(&g, "chain", 777, samples), want);
}

/// A whole render places a saw's lines by one transform over its horizon; a stream sums
/// them at each instant, so its blocks agree with each other rather than with that render.
/// A form in `f`, read at `t`, is a form in `t` like any other.
#[test]
fn a_streamed_closed_form_is_the_same_in_blocks_of_any_size() {
    let g = composition();
    let samples = 5_000;
    for target in ["saw", "sawed", "spectrum"] {
        let one = streamed(&g, target, samples, samples);
        sounds(&one);
        for block in [1, 64, 1_023] {
            assert_eq!(
                streamed(&g, target, block, samples),
                one,
                "{target} in {block}"
            );
        }
    }
}

#[test]
fn a_bound_name_reaches_the_target_as_a_named_argument() {
    let g = composition();
    let config = StreamConfig {
        rate: RATE,
        block: 2_000,
        silent: None,
    };
    let mut stream = Stream::open(
        &g,
        "piano3",
        &[("f0".into(), 261.63), ("vel".into(), 4.5)],
        config,
    )
    .expect("a bound stream");
    let first = stream.next_block().expect("a block").expect("no end");
    assert_eq!(first.plane(0), &whole(&g, "note", 2_000)[..]);
}

#[test]
fn a_node_no_block_reads_alone_refuses_the_stream() {
    let g = composition();
    let config = StreamConfig {
        rate: RATE,
        block: 256,
        silent: None,
    };
    let refused = Stream::open(&g, "ahead", &[], config)
        .err()
        .expect("a read ahead refuses");
    assert_eq!(refused.code(), "engine.no_stream", "{refused}");
    for name in ["f0) + (1", "2x"] {
        let refused = Stream::open(&g, "piano3", &[(name.into(), 1.0)], config)
            .err()
            .expect("a name that is no word refuses");
        assert_eq!(refused.code(), "engine.no_stream", "{refused}");
    }
    let refused = Stream::open(&g, "piano3", &[("f0".into(), f64::NAN)], config)
        .err()
        .expect("a value that is no number refuses");
    assert_eq!(refused.code(), "engine.no_stream", "{refused}");
}

const SILENT: Silent = Silent {
    bits: 16,
    max_secs: 10.0,
};

/// The whole render cuts after its last sample over the floor; the stream ends where the
/// bound proved silence, and every sample between is under the floor.
#[test]
fn a_stream_until_silent_ends_where_the_bound_proves_it() {
    let g = composition();
    let config = StreamConfig {
        rate: RATE,
        block: 4_096,
        silent: Some(SILENT),
    };
    let mut stream = Stream::open(&g, "damped", &[], config).expect("a proven end");
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().expect("a block") {
        heard.extend_from_slice(block.plane(0));
    }
    assert_eq!(Some(heard.len()), stream.end());
    let whole = render_until_silent(
        &g,
        "damped",
        RenderConfig::seconds(RATE, SILENT.max_secs),
        SILENT,
        None,
        None,
    )
    .expect("a silent render");
    let want = whole.buffer(whole.root).expect("a buffer").plane(0);
    sounds(want);
    assert_eq!(heard[..want.len()], *want);
    assert!(
        heard[want.len()..]
            .iter()
            .all(|v| v.abs() < SILENT.threshold())
    );
}

#[test]
fn a_stream_whose_silence_is_never_proven_refuses_at_its_opening() {
    let g = composition();
    let config = StreamConfig {
        rate: RATE,
        block: 256,
        silent: Some(SILENT),
    };
    for (target, code) in [
        ("held", "engine.never_silent"),
        ("note", "engine.no_tail_bound"),
    ] {
        let refused = Stream::open(&g, target, &[], config)
            .err()
            .expect("no proof");
        assert_eq!(refused.code(), code, "{target}: {refused}");
    }
}
