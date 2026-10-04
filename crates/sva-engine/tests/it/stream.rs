// Concern: proves a stream's blocks are the samples a whole render writes, bit for bit, or it refuses | Non-concern: one machine's own blocks (sva-samples) | IO: (a composition, bindings) -> blocks

use crate::fixtures::{Now, graph_of, next};
use sva_ast::Graph;
use sva_engine::{Range, RenderConfig, Stream, StreamConfig, Tier, render};

const RATE: u32 = 8_000;

const PIANO3: &str = "0.0014822 * chaigne_askenfelt(f0, vel=vel, b=max(1.4e-4, \
    4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, hammer_k=2e10*max(1, \
    pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) - \
    0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

/// The demo synth voice: two detuned saws through a lowpass under an ADSR.
const VOICE: &str = "release = inf\nlowpass(sample(0.3*vel*(saw(f0*8ct) + saw(f0/8ct))*(crop(min(t/0.005, 1)*(0.6 + \
    0.4*exp(-t/0.25)), 0s, release) + crop(min(release/0.005, 1)*(0.6 + \
    0.4*exp(-release/0.25))*exp(-(t - release)/0.3), release, 3600s))), cutoff=min(f0*(2 + \
    10*vel), 18000), q=0.9)\n";

/// A string whose felt, lifted until `release`, ramps its dashpot in over 0.03 s.
const STRING: &str = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*pow(262/f0, 2)\
    *crop(min(1, (t - release)/0.03), release, inf))\n";

const ECHO: &str = "feedback = 0.35\nsample(x) + feedback*self[idx(t - 0.25s)]\n";

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
            ("string", STRING),
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
    let whole = render(g, &format!("at_{target}"), config, &Tier::default()).expect("a render");
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
        channels: None,
        render: RenderConfig {
            range,
            ..RenderConfig::at(RATE)
        },
    }
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = config(block, four());
    let mut stream = Stream::open(g, &at(target), config, &Tier::default())
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::with_capacity(samples + block);
    while out.len() < samples {
        let block = next(&mut stream).unwrap_or_else(|e| panic!("{e}"));
        out.extend_from_slice(block.expect("a stream with no end").plane(0));
    }
    out.truncate(samples);
    out
}

fn whole(g: &Graph, target: &str, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(RATE);
    let held = render(
        g,
        target,
        RenderConfig::seconds(RATE, secs),
        &Tier::default(),
    )
    .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

/// `s` seconds of samples at the stream's rate.
fn secs(s: f64) -> usize {
    (s * f64::from(RATE)) as usize
}

fn sounds(samples: &[f64]) {
    assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
}

#[test]
fn a_streamed_piano_note_is_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = secs(0.2);
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
    let (samples, echoing) = (secs(0.36), secs(0.252));
    let want = whole(&g, "echoed", samples);
    assert_ne!(want[echoing..], whole(&g, "note", samples)[echoing..]);
    assert_eq!(streamed(&g, "echoed", 1_024, samples), want);
}

#[test]
fn a_biquad_chain_over_a_streamed_note_is_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = secs(0.14);
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
    let samples = secs(0.12);
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
    let samples = secs(0.23);
    for target in ["low", "high"] {
        let want = whole(&g, target, samples);
        sounds(&want);
        for block in [1, 64, 441, samples / 2] {
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
        let mut stream = Stream::open(
            &g,
            &at(target),
            config(441, Range::default()),
            &Tier::default(),
        )
        .now()
        .expect("opens");
        let mut heard = Vec::new();
        while let Some(block) = next(&mut stream).expect("a block") {
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
    let (start, samples) = (secs(0.25) as i64, secs(0.14) as i64);
    let range = Range {
        start: Some(start),
        end: Some(start + samples),
    };
    for target in ["echoed", "chain", "low"] {
        let config = RenderConfig {
            range,
            ..RenderConfig::at(RATE)
        };
        let held = render(&g, target, config, &Tier::default())
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        let want = held.output(held.root).expect("a buffer").plane(0).to_vec();
        sounds(&want);
        assert_eq!(
            want[..],
            whole(&g, target, (start + samples) as usize)[start as usize..],
            "{target}: a late start trims the output alone"
        );
        let mut stream = Stream::open(&g, &at(target), self::config(777, range), &Tier::default())
            .now()
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        let mut heard = Vec::new();
        while let Some(block) = next(&mut stream).expect("a block") {
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(heard, want, "{target}");
    }
}

/// A read ahead of its reader streams: the value it reads is computed ahead of the block.
#[test]
fn a_read_ahead_streams_the_whole_render_bit_for_bit() {
    let g = composition();
    let samples = secs(0.2);
    let want = whole(&g, "ahead", samples);
    sounds(&want);
    for block in [1, 64, 777] {
        assert_eq!(
            streamed(&g, "ahead", block, samples),
            want,
            "in blocks of {block}"
        );
    }
}

/// A root whose support never ends streams on for as long as it is pulled, where a whole
/// render of it refuses.
#[test]
fn an_open_stream_whose_support_never_ends_streams_on_while_pulled() {
    let g = composition();
    for target in ["held", "tone", "bar", "damped"] {
        let block = secs(0.1);
        let mut stream = Stream::open(
            &g,
            &at(target),
            config(block, Range::default()),
            &Tier::default(),
        )
        .now()
        .unwrap_or_else(|e| panic!("{target}: {e}"));
        let mut heard = Vec::new();
        for _ in 0..30 {
            let block = next(&mut stream).expect("a block").expect("no end");
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(stream.end(), None, "{target}");
        sounds(&heard[heard.len() - block..]);
    }
    let refused = render(&g, "at_damped", RenderConfig::at(RATE), &Tier::default())
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
    let mut stream = Stream::open(&g, &at("tone"), config(256, range), &Tier::default())
        .now()
        .expect("a closed range opens");
    let mut heard = 0;
    while let Some(block) = next(&mut stream).expect("a block") {
        heard += block.len();
    }
    assert_eq!((heard, stream.end()), (1_000, Some(1_000)));
}

fn reads() -> Graph {
    graph_of(
        "reads",
        &[
            (
                "filtered",
                "lowpass(sample(0.3*saw(2000 + 20*t)), cutoff=900, q=0.8)\n",
            ),
            ("shifted", "@filtered(t - 3sp)\n"),
            ("between", "@filtered(t - 0.0123456s)\n"),
            ("scaled", "@filtered(0.75*t)\n"),
            ("warped", "@filtered(t - t*t/4)\n"),
            ("reversed", "@filtered(1s - t)\n"),
            ("tone", "crop(0.3*saw(2000 + 20*t), 0s, 0.1s)\n"),
            ("formula", "@tone(0.75*t - 0.0123456s) + @filtered\n"),
            (
                "looped",
                "crop(sample(0.3*saw(2000 + 20*t)), 0s, 0.05s) + 0.5*self[idx(t - 2ms) - 1]\n",
            ),
            (
                "wobbled",
                "crop(sample(0.3*saw(2000 + 20*t)), 0s, 0.05s) + \
                 0.5*lp(self[idx(t - 0.005s - 0.002s*sin(2*pi*0.5*t))], cutoff=2000)\n",
            ),
            (
                "nearest",
                "@filtered[idx(t - 0.005s - 0.002s*sin(2*pi*0.5*t))]\n",
            ),
            ("held", "rand(t - t % 0.01s, seed=3)*@filtered\n"),
        ],
    )
}

fn streamed_at(g: &Graph, target: &str, rate: u32, block: usize, samples: usize) -> Vec<f64> {
    let config = StreamConfig {
        block,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(4 * i64::from(rate)),
            },
            ..RenderConfig::at(rate)
        },
    };
    let mut stream = Stream::open(g, &at(target), config, &Tier::default())
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::with_capacity(samples + block);
    while out.len() < samples {
        let block = next(&mut stream).unwrap_or_else(|e| panic!("{target}: {e}"));
        out.extend_from_slice(block.expect("a stream with no end").plane(0));
    }
    out.truncate(samples);
    out
}

fn whole_at(g: &Graph, target: &str, rate: u32, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(rate);
    let held = render(
        g,
        target,
        RenderConfig::seconds(rate, secs),
        &Tier::default(),
    )
    .unwrap_or_else(|e| panic!("{target}: {e}"));
    held.output(held.root).expect("a buffer").plane(0).to_vec()
}

/// Every node steps on the grid its reader asks for, so a stream writes what the whole render
/// writes at any rate: a filter, a whole shift of it, the filter between its steps and at a
/// scaled time, a closed form at any instant, indexed loops at a fixed and a moving delay, the
/// nearest steps of a filter under a moving delay, and noise held over a moving time.
#[test]
fn a_stream_is_the_whole_render_bit_for_bit_at_every_rate() {
    let g = reads();
    for rate in [8_000, 44_100, 48_000, 96_000] {
        for target in [
            "filtered", "shifted", "between", "scaled", "formula", "looped", "wobbled", "nearest",
            "held",
        ] {
            let samples = rate as usize * 14 / 100;
            let want = whole_at(&g, target, rate, samples);
            sounds(&want);
            for block in [256, 1_000] {
                assert_eq!(
                    streamed_at(&g, target, rate, block, samples),
                    want,
                    "{target} at {rate} in blocks of {block}"
                );
            }
        }
    }
}

/// A filter has a value only at its own steps, so a read at a moving time refuses at typing,
/// whole or streamed.
#[test]
fn a_filter_read_at_a_moving_time_refuses_whole_and_streamed() {
    let g = reads();
    let Err(whole) = render(
        &g,
        "warped",
        RenderConfig::seconds(RATE, 0.1),
        &Tier::default(),
    ) else {
        panic!("a filter has no value at a time that moves");
    };
    assert_eq!(whole.code(), "type.stateful_warp", "{whole}");
    let Err(streamed) =
        Stream::open(&g, &at("warped"), config(256, four()), &Tier::default()).now()
    else {
        panic!("a filter streams no value at a time that moves");
    };
    assert_eq!(streamed.code(), "type.stateful_warp", "{streamed}");
}

/// A read backwards in time streams as the whole render reads it: the value it reads is held
/// from where the stream first asks for it back to where it last does.
#[test]
fn a_streamed_read_backwards_in_time_is_the_whole_render() {
    let g = reads();
    let samples = secs(0.5);
    let want = whole_at(&g, "reversed", RATE, samples);
    sounds(&want);
    assert_eq!(streamed_at(&g, "reversed", RATE, 512, samples), want);
}

/// A composition's own `notes` is an ordinary node: two that define it otherwise each stream
/// their own over one tier.
#[test]
fn a_composition_s_own_notes_streams_as_it_is_written() {
    let tier = Tier::default();
    for hz in [220, 330] {
        let g = graph_of(
            &format!("own-notes-{hz}"),
            &[
                ("notes", &format!("sample(0.1*sin(2*pi*{hz}*t))\n")),
                ("master", "@notes*0.5\n"),
            ],
        );
        let mut stream = Stream::open(&g, &at("master"), config(64, four()), &tier)
            .now()
            .unwrap_or_else(|e| panic!("{e}"));
        let block = next(&mut stream).expect("a block").expect("samples");
        assert_eq!(block.plane(0), &whole(&g, "master", 64)[..], "{hz} Hz");
    }
}
