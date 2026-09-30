// Concern: proves a read past a stream's position skips there, live or exact | Non-concern: a read at the position (stream.rs) | IO: (a stream, a sample) -> blocks, dropped, work

use std::cell::RefCell;

use crate::fixtures::{Now, added, graph_of};
use sva_ast::Graph;
use sva_engine::{
    Change, Changed, EngineError, NoStore, Placed, Range, RenderConfig, Stream, StreamConfig,
    change, render,
};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

fn composition() -> Graph {
    graph_of(
        "skip",
        &[
            ("held", "0.3*sin(2*pi*220*t)\n"),
            ("slow", "sin(2*pi*330*t)*exp(0 - 0.5*t)\n"),
            (
                "sawed",
                "lowpass(sample(0.3*saw(220*t)), cutoff=900, q=0.8)\n",
            ),
            ("blip", "crop(sin(2*pi*440*t)*exp(0 - 8*t), 0s, 0.25s)\n"),
        ],
    )
}

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {}", e.message))
}

/// A stream over `target` for four seconds, live where asked.
fn opened(g: &Graph, target: &str, live: bool) -> RefCell<Stream> {
    let config = StreamConfig {
        block: BLOCK,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(4 * i64::from(RATE)),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let mut stream = Stream::open(g, &expr(target), config, None, &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    if live {
        stream.go_live();
    }
    RefCell::new(stream)
}

/// A live stream over `target` with no end asked.
fn endless(g: &Graph, target: &str) -> RefCell<Stream> {
    let config = StreamConfig {
        block: BLOCK,
        render: RenderConfig::at(RATE),
    };
    let mut stream = Stream::open(g, &expr(target), config, None, &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    stream.go_live();
    RefCell::new(stream)
}

/// A term added at its landing, answered its handle.
fn landing(stream: &RefCell<Stream>, g: &Graph, term: &str) -> sva_engine::Handle {
    let build =
        |_: &Stream| Ok::<_, EngineError>(Change::Add(g.clone(), expr(term), Placed::Landing));
    let Ok(Changed::Added(note)) = change(stream, build, &NoStore).now() else {
        panic!("an add answers its handle");
    };
    note
}

fn add(stream: &RefCell<Stream>, g: &Graph, term: &str) {
    added(stream, g, &expr(term), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("`{term}`: {e}"));
}

fn read(stream: &RefCell<Stream>, at: usize) -> Vec<f64> {
    let block = stream.borrow_mut().read(at as i64, BLOCK);
    let block = block
        .unwrap_or_else(|e| panic!("{e}"))
        .expect("before the end");
    assert_eq!(
        block.start(),
        at as i64,
        "the block starts where it was read"
    );
    block.plane(0).to_vec()
}

/// `text` rendered whole over its first `samples`.
fn whole(g: &Graph, text: &str, samples: usize) -> Vec<f64> {
    let mut g = g.clone();
    assert!(g.define("whole", expr(text)));
    let secs = samples as f64 / f64::from(RATE);
    let held = render(&g, "whole", RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("`{text}`: {e}"));
    let id = held.id("whole").expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

/// A live stream behind the clock skips to now: formulas read on exactly there, and nothing
/// of the span between is computed.
#[test]
fn a_live_skip_reads_a_formula_on_exactly_and_computes_nothing_between() {
    let g = composition();
    let stream = opened(&g, "@notes", true);
    add(&stream, &g, "@held");
    add(&stream, &g, "@slow");
    read(&stream, 0);
    read(&stream, BLOCK);
    let at = 40 * BLOCK + 17;
    let heard = read(&stream, at);
    let want = whole(&g, "@held + @slow", at + BLOCK);
    assert_eq!(heard, want[at..], "the whole render there");
    assert_eq!(stream.borrow().position(), (at + BLOCK) as i64);
    assert_eq!(stream.borrow().work().samples, 3 * BLOCK as u64);
    assert!(stream.borrow().dropped().is_empty());
}

/// A stateful term cannot jump: it starts silent where the stream skips to, as a live edit
/// starts one, is named in `dropped`, and runs nothing of the span between.
#[test]
fn a_live_skip_starts_a_filter_silent_there_and_names_it() {
    let g = composition();
    let stream = opened(&g, "@notes", true);
    add(&stream, &g, "@sawed");
    read(&stream, 0);
    read(&stream, BLOCK);
    let at = 40 * BLOCK + 17;
    let heard = read(&stream, at);
    let fresh = "lowpass(sample(crop(0.3*saw(220*t), {at}sp, inf)), cutoff=900, q=0.8)";
    let want = whole(&g, &fresh.replace("{at}", &at.to_string()), at + BLOCK);
    assert_eq!(heard, want[at..], "silent at the skip, stepping from there");
    assert_eq!(stream.borrow().counts().dropped, 1);
    assert_eq!(stream.borrow().dropped(), ["sawed"]);
    let gap = (2 * BLOCK) as i64..at as i64;
    for segment in stream.borrow().evaluated("sawed") {
        assert!(
            segment.end <= gap.start || segment.start >= gap.end,
            "{segment:?} is inside the skipped span"
        );
    }
}

/// An exact stream computes through the span it skips, so every sample it plays is the
/// whole render's, state and all.
#[test]
fn an_exact_skip_computes_through_and_plays_the_whole_render() {
    let g = composition();
    let stream = opened(&g, "@sawed", false);
    read(&stream, 0);
    let at = 40 * BLOCK + 17;
    let heard = read(&stream, at);
    assert_eq!(heard, whole(&g, "@sawed", at + BLOCK)[at..]);
    assert!(stream.borrow().dropped().is_empty());
}

/// A read before where the stream stands, or of no samples, is refused.
#[test]
fn a_read_behind_the_stream_or_of_nothing_is_refused() {
    let g = composition();
    let stream = opened(&g, "@held", true);
    read(&stream, 0);
    let behind = stream.borrow_mut().read(BLOCK as i64 - 1, BLOCK).err();
    assert_eq!(
        behind.map(|e| e.code().to_string()).as_deref(),
        Some("engine.stream_behind")
    );
    let empty = stream.borrow_mut().read(BLOCK as i64, 0).err();
    assert_eq!(
        empty.map(|e| e.code().to_string()).as_deref(),
        Some("engine.empty_read")
    );
    assert_eq!(read(&stream, BLOCK), whole(&g, "@held", 2 * BLOCK)[BLOCK..]);
}

/// A term placed at its landing keeps its sample 0 where it landed across a skip.
#[test]
fn a_term_placed_at_its_landing_keeps_its_time_across_a_skip() {
    let g = composition();
    let stream = opened(&g, "@notes", true);
    read(&stream, 0);
    let build =
        |_: &Stream| Ok::<_, EngineError>(Change::Add(g.clone(), expr("@slow"), Placed::Landing));
    let Ok(Changed::Added(note)) = change(&stream, build, &NoStore).now() else {
        panic!("an add answers its handle");
    };
    assert_eq!(stream.borrow().landed(note), Some(BLOCK as i64));
    let at = 40 * BLOCK + 17;
    let heard = read(&stream, at);
    let want = whole(&g, &format!("@slow(t - {BLOCK}sp)"), at + BLOCK);
    assert_eq!(heard, want[at..]);
    assert_eq!(stream.borrow().landed(note), Some(BLOCK as i64));
}

/// A live stream with nothing sounding never ends: a read anywhere ahead plays full frames of
/// silence there.
#[test]
fn a_silent_live_stream_reads_full_frames_of_zeros() {
    let g = composition();
    let stream = endless(&g, "@notes");
    assert_eq!(read(&stream, 0), vec![0.0; BLOCK]);
    let far = 30 * RATE as usize + 17;
    assert_eq!(read(&stream, far), vec![0.0; BLOCK]);
    assert_eq!(stream.borrow().position(), (far + BLOCK) as i64);
    assert_eq!(stream.borrow().end(), None);
}

/// Once its last note ended, a live stream read far ahead moves there, and a note added then
/// lands where it stands and plays from its attack.
#[test]
fn a_note_added_after_a_silent_skip_lands_there_with_its_attack() {
    let g = composition();
    let stream = endless(&g, "@notes");
    landing(&stream, &g, "@blip");
    for block in 0..9 {
        read(&stream, block * BLOCK);
    }
    assert_eq!(stream.borrow().counts().terms, 0, "the note ended");
    let far = 30 * RATE as usize + 17;
    assert_eq!(read(&stream, far), vec![0.0; BLOCK]);
    let note = landing(&stream, &g, "@blip");
    let at = far + BLOCK;
    assert_eq!(stream.borrow().landed(note), Some(at as i64));
    let attack = whole(&g, "@blip", BLOCK);
    assert!(attack.iter().any(|v| *v != 0.0));
    assert_eq!(read(&stream, at), attack, "the attack is whole");
}
