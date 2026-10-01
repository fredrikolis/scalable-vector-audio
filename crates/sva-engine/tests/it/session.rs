// Concern: proves a long live stream holds bounded state and reads each stored sound once, by counts | Non-concern: one block's samples (stream.rs) | IO: (a composition, minutes of taps) -> counts

use std::cell::RefCell;

use crate::fixtures::{Now, added, graph_of, next};
use sva_ast::Graph;
use sva_engine::{Backend, Range, RenderConfig, Stream, StreamConfig, Tier};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

fn opened<B: Backend>(graph: &Graph, end: i64, tier: &Tier<B>) -> RefCell<Stream> {
    let config = StreamConfig {
        block: BLOCK,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(end),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let master = expr("@room(t, x=@notes)");
    RefCell::new(
        Stream::open(graph, &master, config, tier)
            .now()
            .expect("a stream"),
    )
}

fn played(stream: &RefCell<Stream>, blocks: usize) {
    for _ in 0..blocks {
        next(&mut stream.borrow_mut()).expect("a block");
    }
}

const ROOM: (&str, &str) = (
    "room",
    "lowpass(x, cutoff=3000, q=0.7) + 0.3*x(t - 0.03s)\n",
);

const BLIP: (&str, &str) = (
    "blip",
    "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.1s)\n",
);

/// A held pad and notes that retire: each value keeps one segment per unbroken run, as many
/// after half a minute as after four seconds.
#[test]
fn a_long_session_keeps_one_segment_per_unbroken_run_and_no_retired_term() {
    let graph = graph_of(
        "segments",
        &[
            (
                "pad",
                "lowpass(sample(0.1*saw(110*t)), cutoff=900, q=0.7)\n",
            ),
            BLIP,
            ROOM,
        ],
    );
    let tier = Tier::default();
    let stream = opened(&graph, i64::from(RATE) * 600, &tier);
    let pad = added(&stream, &graph, &expr("@pad(t)"), &tier).now();
    let pad = format!("notes#{}", pad.expect("added").0);
    let mut seen = Vec::new();
    for second in (0..32).step_by(4) {
        let at = stream.borrow().position();
        for k in 0..2 {
            let onset = at + 256 + 700 * k;
            let note = format!("@blip(t - {onset}sp, f0={})", 200 + 10 * k);
            added(&stream, &graph, &expr(&note), &tier)
                .now()
                .expect("added");
        }
        played(&stream, 4 * RATE as usize / BLOCK);
        let held = stream.borrow();
        let segments = ["notes", "room", "streamed", pad.as_str()].map(|n| held.evaluated(n).len());
        seen.push((second, segments, held.counts().terms, held.held_bytes()));
    }
    let (first, last) = (seen[0], seen[seen.len() - 1]);
    assert!(first.1.iter().all(|n| *n <= 2), "{first:?}");
    assert_eq!(first.1, last.1, "{seen:?}");
    assert!(last.2 <= first.2, "only sounding terms stay: {seen:?}");
    assert!(last.3 <= first.3, "{seen:?}");
}
