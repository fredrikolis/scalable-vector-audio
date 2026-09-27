// Concern: proves a stream's blocks are the samples a whole render writes, bit for bit, or it refuses | Non-concern: one machine's own blocks (sva-samples) | IO: (a composition, bindings) -> blocks

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Range, RenderConfig, Stream, StreamConfig, render};

const RATE: u32 = 44_100;

const PIANO3: &str = "0.0014822 * chaigne_askenfelt(f0, vel=vel, b=max(1.4e-4, \
    4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, hammer_k=2e10*max(1, \
    pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) - \
    0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

/// The demo synth voice: two detuned saws through a lowpass under an ADSR.
const VOICE: &str = "lowpass(sample(0.3*vel*(saw(f0*8ct) + saw(f0/8ct))*(crop(min(t/0.005, 1)*(0.6 + \
    0.4*exp(-t/0.25)), 0s, release) + crop(min(release/0.005, 1)*(0.6 + \
    0.4*exp(-release/0.25))*exp(-(t - release)/0.3), release, 3600s))), cutoff=min(f0*(2 + \
    10*vel), 18000), q=0.9)\n";

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
            ("bar", "chaigne_doutaut(440)\n"),
            ("slow", "sin(2*pi*220*t)*exp(0 - 0.5*t)\n"),
            ("voice", VOICE),
            ("low", "@voice(t, f0=65.406, vel=0.8, release=0.1)\n"),
            ("high", "@voice(t, f0=1046.502, vel=0.8, release=0.1)\n"),
            ("tone", "0.3*saw(220)\n"),
            ("clipped", "crop(0.3*sin(2*pi*220*t), 0s, 0.1s)\n"),
            ("decayed", "sin(2*pi*220*t)*exp(0 - t/0.005)\n"),
            ("at_clipped", "@clipped\n"),
            ("at_decayed", "@decayed\n"),
            ("at_damped", "@damped\n"),
        ],
    )
}

/// A render of `@target`, the node a stream of it reads through, over the same interval.
fn read_through(g: &Graph, target: &str, config: RenderConfig) -> Vec<f64> {
    let whole = render(g, &format!("at_{target}"), config, None).expect("a render");
    whole
        .output(whole.root)
        .expect("a buffer")
        .plane(0)
        .to_vec()
}

/// `@target`, as a caller names what it streams.
fn at(target: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(&format!("@{target}")).expect("a ref")
}

/// Four seconds from t = 0, which no test reaches the end of.
fn four() -> Range {
    Range {
        start: Some(0),
        end: Some(4 * i64::from(RATE)),
    }
}

fn config(block: usize, range: Range) -> StreamConfig {
    StreamConfig {
        block,
        render: RenderConfig {
            range,
            ..RenderConfig::at(RATE)
        },
    }
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = config(block, four());
    let mut stream = Stream::open(g, &at(target), config, None).unwrap_or_else(|e| panic!("{e}"));
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
    held.output(id).expect("a buffer").plane(0).to_vec()
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

/// Each saw's harmonics are one run, summed at each instant by the same evaluator both read.
#[test]
fn a_streamed_synth_voice_is_the_whole_render_bit_for_bit_in_blocks_of_any_size() {
    let g = composition();
    let samples = 10_000;
    for target in ["low", "high"] {
        let want = whole(&g, target, samples);
        sounds(&want);
        for block in [1, 64, 441, 5_000] {
            assert_eq!(
                streamed(&g, target, block, samples),
                want,
                "{target} in blocks of {block}"
            );
        }
    }
}

/// An open stream ends where its root's support does, at a crop or where `exp` underflows,
/// the render's own end.
#[test]
fn an_open_stream_ends_where_its_support_does() {
    let g = composition();
    for target in ["clipped", "decayed"] {
        let mut stream =
            Stream::open(&g, &at(target), config(441, Range::default()), None).expect("opens");
        let mut heard = Vec::new();
        while let Some(block) = stream.next_block().expect("a block") {
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(Some(heard.len() as i64), stream.end(), "{target}");
        let want = read_through(&g, target, RenderConfig::at(RATE));
        assert_eq!(heard, want, "{target}");
    }
}

/// History before a late start is computed all the same: a stream from sample `a` on is the
/// whole render over the same range, bit for bit, echo and filters and all.
#[test]
fn a_stream_from_a_later_start_is_the_whole_render_over_the_same_range() {
    let g = composition();
    let (start, samples) = (11_000, 6_000);
    let range = Range {
        start: Some(start),
        end: Some(start + samples),
    };
    for target in ["echoed", "chain", "low"] {
        let config = RenderConfig {
            range,
            ..RenderConfig::at(RATE)
        };
        let held = render(&g, target, config, None).unwrap_or_else(|e| panic!("{target}: {e}"));
        let want = held.output(held.root).expect("a buffer").plane(0).to_vec();
        sounds(&want);
        assert_eq!(
            want[..],
            whole(&g, target, (start + samples) as usize)[start as usize..],
            "{target}: a late start trims the output alone"
        );
        let mut stream = Stream::open(&g, &at(target), self::config(777, range), None)
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        let mut heard = Vec::new();
        while let Some(block) = stream.next_block().expect("a block") {
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(heard, want, "{target}");
    }
}

#[test]
fn a_node_no_block_reads_alone_refuses_the_stream() {
    let g = composition();
    let refused = Stream::open(&g, &at("ahead"), config(256, four()), None)
        .err()
        .expect("a read ahead refuses");
    assert_eq!(refused.code(), "engine.no_stream", "{refused}");
}

/// A root whose support never ends streams on for as long as it is pulled, where a whole
/// render of it refuses.
#[test]
fn an_open_stream_whose_support_never_ends_streams_on_while_pulled() {
    let g = composition();
    for target in ["held", "tone", "bar", "damped"] {
        let mut stream = Stream::open(&g, &at(target), config(4_410, Range::default()), None)
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        let mut heard = Vec::new();
        for _ in 0..30 {
            let block = stream.next_block().expect("a block").expect("no end");
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(stream.end(), None, "{target}");
        sounds(&heard[heard.len() - 4_410..]);
    }
    let refused = render(&g, "at_damped", RenderConfig::at(RATE), None)
        .err()
        .expect("an open render with no end");
    assert_eq!(refused.code(), "render.no_end", "{refused}");
}

/// A range with an end ends there whatever holds under it.
#[test]
fn a_closed_stream_ends_at_its_range() {
    let g = composition();
    let range = Range {
        start: Some(0),
        end: Some(1_000),
    };
    let mut stream =
        Stream::open(&g, &at("tone"), config(256, range), None).expect("a closed range opens");
    let mut heard = 0;
    while let Some(block) = stream.next_block().expect("a block") {
        heard += block.len();
    }
    assert_eq!((heard, stream.end()), (1_000, Some(1_000)));
}
