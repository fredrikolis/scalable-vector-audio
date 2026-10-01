// Concern: proves a long live stream holds bounded state and reads each stored sound once, by counts | Non-concern: one block's samples (stream.rs) | IO: (a composition, minutes of taps) -> counts

use std::cell::RefCell;

use crate::disk::{Memory, opened as over_disk};
use crate::fixtures::{Now, added, graph_of, next};
use sva_ast::Graph;
use sva_engine::{Backend, Counters, Range, RenderConfig, Stream, StreamConfig, Tier, fetch};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

fn until(end: i64) -> RenderConfig {
    RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(end),
        },
        ..RenderConfig::at(RATE)
    }
}

fn opened<B: Backend>(graph: &Graph, end: i64, tier: &Tier<B>) -> RefCell<Stream> {
    let config = StreamConfig {
        block: BLOCK,
        channels: None,
        render: until(end),
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

const SOUNDS: usize = 15;

/// Fifteen one-second sounds, each rendered and persisted by a process of its own.
fn pad(name: &str) -> (Graph, Memory) {
    let mut files: Vec<(String, String)> = (0..SOUNDS)
        .map(|k| {
            let f = 110 + 20 * k;
            let sound = format!(
                "crop(lowpass(sample(sin(2*pi*{f}*t)*exp(-t/0.2)), cutoff=2000, q=0.7), 0s, 1s)\n"
            );
            (format!("s{k}"), sound)
        })
        .collect();
    files.push((ROOM.0.to_string(), ROOM.1.to_string()));
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let graph = graph_of(name, &files);
    let disk = Memory::default();
    for k in 0..SOUNDS {
        let tier = over_disk(&disk, u64::MAX);
        let end = i64::from(RATE);
        sva_engine::render_over(&graph, &format!("s{k}"), until(end), &tier)
            .now()
            .expect("a render");
        tier.persist().now().expect("persisted");
    }
    (graph, disk)
}

/// Two minutes of taps on the pad, a three-sound loop chunk every sixteenth, each landing a
/// block ahead, fetched each second as a page does: memory's counters after.
fn session(cap: u64) -> Counters {
    let (graph, disk) = pad("session");
    let tier = over_disk(&disk, u64::MAX);
    tier.set_max_bytes(cap);
    let end = i64::from(RATE) * 120;
    let stream = opened(&graph, end + i64::from(RATE), &tier);
    let mut tap = 0;
    while stream.borrow().position() < end {
        let at = stream.borrow().position() + BLOCK as i64;
        let s = tap % SOUNDS;
        let term = match tap % 16 {
            0 => format!(
                "@s{s}(t - {at}sp) + @s{}(t - {}sp) + @s{s}(t - {}sp)",
                (s + 3) % SOUNDS,
                at + 2_000,
                at + 4_000
            ),
            _ => format!("@s{s}(t - {at}sp)"),
        };
        added(&stream, &graph, &expr(&term), &tier)
            .now()
            .expect("added");
        played(&stream, 8);
        if tap % 4 == 0 {
            fetch(&stream, &tier).now();
        }
        tap += 1;
    }
    tier.counters()
}

/// Each sound is read off the disk once however often the session taps it, and a working set
/// within the cap evicts nothing.
#[test]
fn a_long_live_session_reads_each_sound_once_and_evicts_nothing_that_fits() {
    let counters = session(64 << 20);
    assert_eq!(counters.disk_reads, SOUNDS as u64, "{counters:?}");
    assert_eq!(counters.disk_lookups, SOUNDS as u64, "{counters:?}");
    assert_eq!(counters.evictions(), 0, "{counters:?}");
}

/// Under a cap the session's computed taps pass many times over, they evict only each other:
/// each sound is still read once.
#[test]
fn a_long_live_session_past_its_cap_still_reads_each_sound_once() {
    let counters = session(2 << 20);
    assert_eq!(counters.disk_reads, SOUNDS as u64, "{counters:?}");
    assert!(counters.probation_evictions > 0, "{counters:?}");
    assert_eq!(counters.protected_evictions, 0, "{counters:?}");
}
