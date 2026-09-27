// Concern: proves a stream's blocks are the samples a whole render writes, bit for bit, or it refuses | Non-concern: one machine's own blocks (sva-samples) | IO: (a composition, bindings) -> blocks

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Range, RenderConfig, Stream, StreamConfig, Until, render};

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
            (
                "keyed",
                "lowpass(sample(@saw*(crop(1, 0s, release) + crop(exp(0 - (t - release)/0.01), \
                 release, 60s))), cutoff=900, q=0.8)\n",
            ),
            ("keyed_up", "@keyed(t, release=0.05)\n"),
            ("voice", VOICE),
            ("low", "@voice(t, f0=65.406, vel=0.8, release=0.1)\n"),
            ("high", "@voice(t, f0=1046.502, vel=0.8, release=0.1)\n"),
            (
                "released",
                "@voice(t, f0=1046.502, vel=0.8, release=0.05)\n",
            ),
            ("tone", "0.3*saw(220)\n"),
            ("clipped", "crop(0.3*saw(220), 0s, 0.1s)\n"),
        ],
    )
}

/// `@target`, as a caller names what it streams.
fn at(target: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(&format!("@{target}")).expect("a ref")
}

/// An hour from t = 0, which no test reaches the end of.
fn hour() -> Range {
    Range {
        start: Some(0),
        end: Some(3_600 * i64::from(RATE)),
    }
}

fn config(block: usize, range: Range, until: Option<Until>) -> StreamConfig {
    StreamConfig {
        rate: RATE,
        block,
        range,
        until,
    }
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = config(block, hour(), None);
    let mut stream = Stream::open(g, &at(target), &[], config).unwrap_or_else(|e| panic!("{e}"));
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

/// Past the proven end, a render twice as long hears nothing at the threshold.
#[test]
fn a_released_voice_or_a_clipped_saw_ends_before_any_sample_brute_force_hears() {
    for target in ["released", "clipped"] {
        ends_before_brute_force_hears(target);
    }
}

fn ends_before_brute_force_hears(target: &str) {
    let g = composition();
    let deep = 2f64.powi(-24);
    let config = config(441, Range::default(), Some(Until::quiet(deep)));
    let mut stream = Stream::open(&g, &at(target), &[], config).expect("a stream opens");
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().expect("a block") {
        heard.extend_from_slice(block.plane(0));
    }
    let end = stream.end().expect("a proven end") as usize;
    assert_eq!(heard.len(), end);
    let brute = whole(&g, target, 2 * end);
    assert_eq!(heard[..], brute[..end], "{target}");
    let last = brute.iter().rposition(|v| v.abs() >= deep);
    assert!(
        last.is_some_and(|at| at < end),
        "{target} heard at {last:?}, past {end}"
    );
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
        let mut stream = Stream::open(&g, &at(target), &[], self::config(777, range, None))
            .unwrap_or_else(|e| panic!("{target}: {e}"));
        let mut heard = Vec::new();
        while let Some(block) = stream.next_block().expect("a block") {
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(heard, want, "{target}");
    }
}

#[test]
fn a_bound_name_reaches_the_target_as_a_named_argument() {
    let g = composition();
    let mut stream = Stream::open(
        &g,
        &at("piano3"),
        &[("f0".into(), 261.63), ("vel".into(), 4.5)],
        config(2_000, hour(), None),
    )
    .expect("a bound stream");
    let first = stream.next_block().expect("a block").expect("no end");
    assert_eq!(first.plane(0), &whole(&g, "note", 2_000)[..]);
}

#[test]
fn a_node_no_block_reads_alone_refuses_the_stream() {
    let g = composition();
    let config = config(256, hour(), None);
    let refused = Stream::open(&g, &at("ahead"), &[], config.clone())
        .err()
        .expect("a read ahead refuses");
    assert_eq!(refused.code(), "engine.no_stream", "{refused}");
    for name in ["f0) + (1", "2x"] {
        let refused = Stream::open(&g, &at("piano3"), &[(name.into(), 1.0)], config.clone())
            .err()
            .expect("a name that is no word refuses");
        assert_eq!(refused.code(), "engine.no_stream", "{refused}");
    }
    let refused = Stream::open(&g, &at("piano3"), &[("f0".into(), f64::NAN)], config)
        .err()
        .expect("a value that is no number refuses");
    assert_eq!(refused.code(), "engine.no_stream", "{refused}");
}

fn sixteen() -> Option<Until> {
    Some(Until::quiet(2f64.powi(-16)))
}

/// The whole render ends at a frame the proof reaches; the stream ends where the bound
/// proved it from a block's end, and every frame between is under the level.
#[test]
fn a_stream_until_quiet_ends_where_the_bound_proves_it() {
    let g = composition();
    let mut stream = Stream::open(
        &g,
        &at("damped"),
        &[],
        config(4_096, Range::default(), sixteen()),
    )
    .expect("a proven end");
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().expect("a block") {
        heard.extend_from_slice(block.plane(0));
    }
    assert_eq!(Some(heard.len() as i64), stream.end());
    let quiet = RenderConfig {
        until: sixteen(),
        ..RenderConfig::at(RATE)
    };
    let whole = render(&g, "damped", quiet, None).expect("a quiet render");
    let want = whole
        .output(whole.root)
        .expect("a buffer")
        .plane(0)
        .to_vec();
    sounds(&want);
    assert_eq!(heard[..want.len()], *want);
    assert!(
        heard[want.len()..]
            .chunks(2_205)
            .all(|f| (f.iter().map(|v| v * v).sum::<f64>() / 2_205.0).sqrt() < 2f64.powi(-16))
    );
}

/// A stream proves nothing at its opening, where no block has left a state yet; its first
/// block's end refuses what no state proves.
#[test]
fn a_stream_whose_quiet_is_never_proven_refuses_at_its_first_block() {
    let g = composition();
    let config = config(256, Range::default(), sixteen());
    for (target, code) in [
        ("held", "engine.never_silent"),
        ("tone", "engine.never_silent"),
        ("bar", "engine.no_tail_bound"),
    ] {
        let mut stream =
            Stream::open(&g, &at(target), &[], config.clone()).expect("no proof at the opening");
        let refused = stream.next_block().err().expect("no proof");
        assert_eq!(refused.code(), code, "{target}: {refused}");
        assert_eq!(stream.position(), config.block as i64, "{target}");
    }
}

/// A range with an end ends there whatever the condition: no proof failing is a refusal.
#[test]
fn a_closed_stream_ends_at_its_range_whatever_its_condition() {
    let g = composition();
    let range = Range {
        start: Some(0),
        end: Some(1_000),
    };
    let mut stream = Stream::open(&g, &at("tone"), &[], config(256, range, sixteen()))
        .expect("a held tone over a closed range opens");
    let mut heard = 0;
    while let Some(block) = stream
        .next_block()
        .expect("no proof refuses a closed range")
    {
        heard += block.len();
    }
    assert_eq!((heard, stream.end()), (1_000, Some(1_000)));
}

/// An open range needs a condition some proof can bring about.
#[test]
fn an_open_stream_no_condition_can_end_refuses_at_its_opening() {
    let g = composition();
    let refused = Stream::open(&g, &at("tone"), &[], config(256, Range::default(), None))
        .err()
        .expect("nothing ends it");
    assert_eq!(refused.code(), "render.no_stop", "{refused}");
}

/// A filter's state exists once a block has run through it: a released filtered voice ends
/// past its key-up where its bound proves it, every sample the whole render's.
#[test]
fn a_filtered_stream_until_quiet_is_proven_from_its_blocks() {
    let g = composition();
    let mut stream = Stream::open(
        &g,
        &at("keyed_up"),
        &[],
        config(441, Range::default(), sixteen()),
    )
    .expect("a filter opens");
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().expect("a block") {
        heard.extend_from_slice(block.plane(0));
    }
    assert_eq!(Some(heard.len() as i64), stream.end());
    assert!(
        heard.len() > 2_205,
        "an end before the key-up at {}",
        heard.len()
    );
    let want = whole(&g, "keyed_up", heard.len());
    sounds(&want);
    assert_eq!(heard, want);
}
