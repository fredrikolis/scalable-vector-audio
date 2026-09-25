// Concern: proves a stream resumed from a checkpoint is the whole render under the bindings then in force | Non-concern: the blocks before it | IO: (a checkpoint, bindings) -> blocks or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{RenderConfig, Silent, Stream, StreamConfig, render, render_until_silent};

const RATE: u32 = 44_100;
const BLOCK: usize = 1_024;

/// piano3 with its damper wired to `release`.
const PIANO: &str = "0.0014822 * chaigne_askenfelt(f0, vel=vel, release=release, \
    b=max(1.4e-4, 4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, \
    hammer_k=2e10*max(1, pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) \
    - 0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

const ECHO: &str = "feedback = 0.35\nx + feedback*self(t - 0.25s)\n";

fn composition(released: f64) -> Graph {
    graph_of(
        "checkpoint",
        &[
            ("piano", PIANO),
            ("echo", ECHO),
            (
                "key",
                "@echo(t, x=@piano(t, f0=261.63, vel=4.5, release=release))\n",
            ),
            ("released", &format!("@key(t, release={released})\n")),
            ("envelope", "sin(2*pi*220*t)*crop(1, 0s, release)\n"),
            (
                "fading",
                "sin(2*pi*220*t)*(crop(1, 0s, release) + crop(exp(0 - (t - release)/0.01), \
                 release, 60s))\n",
            ),
            ("string", "chaigne_askenfelt(523.25, release=release)\n"),
            ("struck", &format!("@string(t, release={released})\n")),
        ],
    )
}

fn config() -> StreamConfig {
    StreamConfig {
        rate: RATE,
        block: BLOCK,
        silent: None,
    }
}

fn opened(g: &Graph, target: &str) -> Stream {
    Stream::open(g, target, &[], config()).unwrap_or_else(|e| panic!("{e}"))
}

fn blocks(stream: &mut Stream, count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count * BLOCK);
    for _ in 0..count {
        let block = stream.next_block().unwrap_or_else(|e| panic!("{e}"));
        out.extend_from_slice(block.expect("a stream with no end").plane(0));
    }
    out
}

fn whole(g: &Graph, target: &str, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(RATE);
    let held = render(g, target, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.buffer(id).expect("a buffer").plane(0).to_vec()
}

/// Key-up lands on the block grid at sample `k`: the held blocks up to it and the released
/// ones after are one whole render with `release = k/rate`, echo and all.
#[test]
fn a_note_released_at_a_checkpoint_is_the_whole_render_released_there() {
    let k = 12 * BLOCK;
    let release = k as f64 / f64::from(RATE);
    let g = composition(release);
    let mut held = opened(&g, "key");
    let mut heard = blocks(&mut held, 12);
    let checkpoint = held.checkpoint();
    assert_eq!(checkpoint.position(), k);
    let mut released = held
        .resume(&checkpoint, &[("release".into(), release)], None)
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&mut released, 8));

    let want = whole(&g, "released", heard.len());
    assert_ne!(
        want[k + 2_000..],
        blocks(&mut held, 8)[2_000..],
        "the damper did nothing"
    );
    assert_eq!(heard, want);
}

#[test]
fn a_checkpoint_resumed_with_the_same_bindings_is_the_stream_it_was_taken_of() {
    let g = composition(1.0);
    let mut stream = opened(&g, "key");
    blocks(&mut stream, 3);
    let checkpoint = stream.checkpoint();
    let mut again = stream
        .resume(&checkpoint, &[], None)
        .expect("the same bindings");
    let (on, resumed) = (blocks(&mut stream, 14), blocks(&mut again, 14));
    assert!(on.iter().any(|v| *v != 0.0), "silence tests nothing");
    assert_eq!(resumed, on);
}

/// A crop's end at `release` is the other causal use: the envelope closes at key-up.
#[test]
fn a_closed_form_released_at_a_checkpoint_closes_there() {
    let g = composition(1.0);
    let mut stream = opened(&g, "envelope");
    let mut heard = blocks(&mut stream, 4);
    let k = 4 * BLOCK;
    let release = k as f64 / f64::from(RATE);
    let mut released = stream
        .resume(&stream.checkpoint(), &[("release".into(), release)], None)
        .expect("a release at the checkpoint");
    heard.extend(blocks(&mut released, 2));
    assert!(heard[..k].iter().any(|v| *v != 0.0));
    assert!(heard[k..].iter().all(|v| *v == 0.0));
}

#[test]
fn a_binding_a_sample_before_the_checkpoint_could_hear_refuses() {
    let g = composition(1.0);
    let mut stream = opened(&g, "key");
    blocks(&mut stream, 4);
    let checkpoint = stream.checkpoint();
    let early = (4 * BLOCK - 2) as f64 / f64::from(RATE);
    for bindings in [
        vec![("release".to_string(), early)],
        vec![("f0".to_string(), 440.0)],
    ] {
        let refused = stream
            .resume(&checkpoint, &bindings, None)
            .err()
            .expect("a binding that reaches back refuses");
        assert_eq!(refused.code(), "engine.binding_not_causal", "{refused}");
    }
}

#[test]
fn a_checkpoint_resumed_on_another_stream_refuses() {
    let g = composition(1.0);
    let mut key = opened(&g, "key");
    blocks(&mut key, 1);
    let envelope = opened(&g, "envelope");
    let refused = envelope
        .resume(&key.checkpoint(), &[], None)
        .err()
        .expect("another target's checkpoint refuses");
    assert_eq!(refused.code(), "engine.checkpoint_mismatch", "{refused}");
}

/// Held, the string rings on unproven; released, its silence is proven and the stream ends.
#[test]
fn a_string_released_at_a_checkpoint_streams_until_its_silence_is_proven() {
    let silent = Silent {
        bits: 16,
        max_secs: 10.0,
    };
    let k = 3 * BLOCK;
    let release = k as f64 / f64::from(RATE);
    let g = composition(release);
    let mut held = opened(&g, "string");
    let mut heard = blocks(&mut held, 3);
    let mut released = held
        .resume(
            &held.checkpoint(),
            &[("release".into(), release)],
            Some(silent),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    while let Some(block) = released.next_block().expect("a block") {
        heard.extend_from_slice(block.plane(0));
    }
    assert_eq!(Some(heard.len()), released.end());
    let whole = render_until_silent(
        &g,
        "struck",
        RenderConfig::seconds(RATE, silent.max_secs),
        silent,
        None,
        None,
    )
    .expect("a silent render");
    let want = whole.buffer(whole.root).expect("a buffer").plane(0);
    assert!(want.len() > k, "silence before the release tests nothing");
    assert_eq!(heard[..want.len()], *want);
    assert!(
        heard[want.len()..]
            .iter()
            .all(|v| v.abs() < silent.threshold())
    );
}

/// `max_secs` counts from the checkpoint, so a note held past it still proves its release.
#[test]
fn a_release_held_past_max_secs_proves_its_silence_from_the_checkpoint() {
    let silent = Silent {
        bits: 16,
        max_secs: 0.3,
    };
    let g = composition(1.0);
    let mut held = opened(&g, "fading");
    blocks(&mut held, 22);
    let k = 22 * BLOCK;
    assert!(k as f64 / f64::from(RATE) > silent.max_secs);
    let release = k as f64 / f64::from(RATE);
    let mut released = held
        .resume(
            &held.checkpoint(),
            &[("release".into(), release)],
            Some(silent),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let end = released.end().expect("a proven end");
    assert!(end > k && (end - k) as f64 / f64::from(RATE) <= silent.max_secs);
    let mut tail = Vec::new();
    while let Some(block) = released.next_block().expect("a block") {
        tail.extend_from_slice(block.plane(0));
    }
    assert_eq!(k + tail.len(), end);
    assert!(tail.iter().any(|v| v.abs() >= silent.threshold()));
}
