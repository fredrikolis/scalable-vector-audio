// Concern: proves a stream's and a render's work counters count what they did and price it alike | Non-concern: what any sample holds (stream.rs) | IO: (a composition) -> Work

use crate::fixtures::{Now, graph_of};
use sva_ast::Graph;
use sva_engine::{NoStore, Range, RenderConfig, Stream, StreamConfig, Work, render};

const RATE: u32 = 44_100;

const VOICE: &str = "release = inf\nlowpass(sample(0.3*vel*(saw(f0*8ct) + saw(f0/8ct))*(crop(min(t/0.005, 1)*(0.6 + \
    0.4*exp(-t/0.25)), 0s, release) + crop(min(release/0.005, 1)*(0.6 + \
    0.4*exp(-release/0.25))*exp(-(t - release)/0.3), release, 3600s))), cutoff=min(f0*(2 + \
    10*vel), 18000), q=0.9)\n";

fn composition() -> Graph {
    graph_of(
        "work",
        &[
            ("voice", VOICE),
            ("low", "@voice(t, f0=65.406, vel=0.8, release=0.1)\n"),
            ("wrapped", "@low(t)\n"),
            ("clipped", "crop(0.3*saw(220), 0s, 0.1s)\n"),
            ("added", "sin(2*pi*110*t) + tanh(sin(2*pi*220*t))\n"),
            ("high", "@voice(t, f0=1046.502, vel=0.8, release=0.05)\n"),
        ],
    )
}

/// Four seconds of range, which no test reaches the end of.
fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Work {
    let config = StreamConfig {
        block,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(4 * i64::from(RATE)),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let target = sva_ast::parse_expr(&format!("@{target}")).expect("a ref");
    let mut stream = Stream::open(g, &target, config, None, &NoStore)
        .now()
        .expect("a stream");
    while stream.position() < samples as i64 {
        if stream.next_block().expect("a block").is_none() {
            break;
        }
    }
    stream.work()
}

/// Two saws' harmonics under 20 kHz at C2, 8 cents either side, each line and its mirror.
const C2_LINES: u128 = 2 * (304 + 307);

#[test]
fn a_stream_counts_its_samples_and_the_lines_its_runs_turn_whatever_its_blocks() {
    let g = composition();
    let samples = 441 * 64;
    let one = streamed(&g, "low", 441, samples);
    assert_eq!(one, streamed(&g, "low", 64, samples));
    assert_eq!(one.samples, samples as u64);
    assert_eq!(one.waves, Some(C2_LINES * samples as u128));
}

/// A stream reads its target through one node of its own, as `wrapped` reads `low`.
/// A windowed sum turns its atoms only inside the window; a sum of rows prices each addend.
#[test]
fn a_swept_or_added_row_counts_only_what_it_sums() {
    let g = composition();
    let clipped = streamed(&g, "clipped", 441, 8_820);
    let atoms = 2 * (20_000 / 220);
    assert_eq!(clipped.waves, Some(atoms * 4_410));
    assert_eq!(clipped, streamed(&g, "clipped", 1_260, 8_820));
    let added = streamed(&g, "added", 441, 8_820);
    assert_eq!(added.waves, Some(2 * 8_820));
    assert_eq!(added, streamed(&g, "added", 63, 8_820));
}

#[test]
fn a_stream_prices_each_sample_as_a_whole_render_prices_it() {
    let g = composition();
    let samples = 4_410;
    let secs = samples as f64 / f64::from(RATE);
    let config = RenderConfig::seconds(RATE, secs);
    let whole = render(&g, "wrapped", config, None).expect("a render");
    let work = whole.work();
    assert_eq!(work.samples, samples as u64);
    assert!(work.priced_flops > 0);
    assert_eq!(
        streamed(&g, "low", 441, samples).priced_flops,
        work.priced_flops
    );
}

/// Beyond its output, a line of sixteen notes holds what two do, give or take one note.
#[test]
fn a_render_holds_what_its_readers_still_reach_and_its_output() {
    let beyond = |n: usize| {
        let line: Vec<String> = (0..n)
            .map(|k| format!("@note(t - {k}s, f0={})", 220 + k))
            .collect();
        let g = graph_of(
            "held-line",
            &[
                (
                    "note",
                    "f0 = 220\ncrop(lowpass(sample(sin(2*pi*f0*t)), cutoff=1000, q=0.7), 0s, 1s)\n",
                ),
                ("line", &format!("{}\n", line.join(" + "))),
            ],
        );
        let held = render(&g, "line", RenderConfig::at(RATE), None).expect("a render");
        let output = held.output(held.root).expect("the root");
        held.held_bytes - size_of_val(output.plane(0))
    };
    let note = RATE as usize * size_of::<f64>();
    let (two, sixteen) = (beyond(2), beyond(16));
    assert!(
        sixteen <= two + note,
        "{sixteen} bytes beyond sixteen notes, {two} beyond two"
    );
}

/// A late window streams through the history its filter needs and drops it: it writes the
/// samples a render from 0 writes there, and holds no more the later it starts.
#[test]
fn a_late_window_streams_its_history_and_holds_no_more_the_later_it_starts() {
    let mut g = graph_of(
        "held-late",
        &[
            (
                "hit-0.25s",
                "sample(rand(t, seed=3))*sample(exp(-t/0.05))\n",
            ),
            ("rest-0.25s", "sample(0*t)\n"),
            (
                "bar-1s",
                "concat(@hit-0.25s, @rest-0.25s, @hit-0.25s, @hit-0.25s)\n",
            ),
            (
                "track-6s",
                "concat(@bar-1s, @bar-1s, @bar-1s, @bar-1s, @bar-1s, @bar-1s)\n",
            ),
            (
                "dry",
                "lowpass(sat(@track-6s*1.3 + @track-6s(t - 0.125s)*0.5), cutoff=900, q=0.7)\n",
            ),
        ],
    );
    g.desugar_arrangement().expect("concat expands");
    let over = |from: f64, to: f64| {
        let at = |secs: f64| (secs * f64::from(RATE)) as i64;
        let config = RenderConfig {
            range: Range {
                start: Some(at(from)),
                end: Some(at(to)),
            },
            ..RenderConfig::at(RATE)
        };
        let held = render(&g, "dry", config, None).expect("a render");
        let output = held.output(held.root).expect("the root").plane(0).to_vec();
        (held.held_bytes - output.len() * size_of::<f64>(), output)
    };
    let ((late, _), (later, samples)) = (over(2.0, 3.0), over(4.0, 5.0));
    let (_, whole) = over(0.0, 5.0);
    assert!(
        samples == whole[whole.len() - samples.len()..],
        "the late window's samples"
    );
    assert!(
        later <= late,
        "{later} bytes beyond a window at 4 s, {late} beyond one at 2 s: history is held"
    );
}
