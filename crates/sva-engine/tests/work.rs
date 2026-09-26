// Concern: proves a stream's and a render's work counters count what they did and price it alike | Non-concern: what any sample holds (stream.rs) | IO: (a composition) -> Work

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{RenderConfig, Silent, Stream, StreamConfig, Work, render, render_until_silent};

const RATE: u32 = 44_100;

const VOICE: &str = "lowpass(sample(0.3*vel*(saw(f0*8ct) + saw(f0/8ct))*(crop(min(t/0.005, 1)*(0.6 + \
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

fn streamed(g: &Graph, target: &str, block: usize, samples: usize, silent: Option<Silent>) -> Work {
    let config = StreamConfig {
        rate: RATE,
        block,
        silent,
    };
    let mut stream = Stream::open(g, target, &[], config).expect("a stream");
    while stream.position() < samples {
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
    let one = streamed(&g, "low", 441, samples, None);
    assert_eq!(one, streamed(&g, "low", 64, samples, None));
    assert_eq!(one.samples, samples as u64);
    assert_eq!(one.proofs, 0);
    assert_eq!(one.waves, Some(C2_LINES * samples as u128));
}

/// A stream reads its target through one node of its own, as `wrapped` reads `low`.
/// A windowed sum turns its atoms only inside the window; a sum of rows prices each addend.
#[test]
fn a_swept_or_added_row_counts_only_what_it_sums() {
    let g = composition();
    let clipped = streamed(&g, "clipped", 441, 8_820, None);
    let atoms = 2 * (20_000 / 220);
    assert_eq!(clipped.waves, Some(atoms * 4_410));
    assert_eq!(clipped, streamed(&g, "clipped", 1_260, 8_820, None));
    let added = streamed(&g, "added", 441, 8_820, None);
    assert_eq!(added.waves, Some(2 * 8_820));
    assert_eq!(added, streamed(&g, "added", 63, 8_820, None));
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
        streamed(&g, "low", 441, samples, None).priced_flops,
        work.priced_flops
    );
}

#[test]
fn a_proof_is_counted_at_each_block_end_and_over_a_whole_grid_once() {
    let g = composition();
    let silent = Silent {
        bits: 24,
        max_secs: 10.0,
    };
    let work = streamed(&g, "high", 4_410, usize::MAX, Some(silent));
    assert_eq!(work.proofs, work.samples.div_ceil(4_410));
    let whole = render_until_silent(
        &g,
        "high",
        RenderConfig::seconds(RATE, silent.max_secs),
        silent,
        None,
    )
    .expect("a silent render");
    assert!(whole.work().proofs >= 1);
    assert_eq!(whole.work().waves, None);
}
