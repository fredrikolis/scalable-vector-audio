// Concern: proves a stream change builds only what it changed | Non-concern: the samples it plays (stream.rs, edit.rs) | IO: (a stream, changes) -> Built per change

use std::cell::RefCell;

use sva_ast::{Expr, Graph};
use sva_engine::{Built, Handle, NoStore, RenderConfig, Stream, StreamConfig};

use crate::fixtures::{Now, added, edited, graph_of, next, removed, replaced};

const RATE: u32 = 8_000;

/// Many branches, one reading the note sum.
const MASTER: &str = "@room(t, x=@notes) + 0.5*@bed";

const WIDER: &str = "@room(t, x=@notes) + 0.5*@bed + 0.3*@bed(t, top=220) + @air";

fn expr(text: &str) -> Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {e:?}"))
}

fn composition() -> Graph {
    graph_of(
        "rebuild",
        &[
            (
                "pluck",
                "lowpass(sample(crop(sin(2*pi*f0*t)*exp(-t/0.05), 0s, 0.2s)), cutoff=3000)\n",
            ),
            ("pad", "lowpass(sample(0.1*saw(f0*t)), cutoff=900, q=0.7)\n"),
            (
                "air",
                "highpass(sample(0.05*sin(2*pi*3000*t)), cutoff=2000)\n",
            ),
            ("room", "lowpass(x, cutoff=4000) + 0.3*x(t - 0.01s)\n"),
            (
                "bed",
                "top = 110\n@pad(t, f0=55) + @pad(t, f0=82.5) + @pad(t, f0=top) + @air\n",
            ),
        ],
    )
}

fn opened(graph: &Graph, target: &str) -> RefCell<Stream> {
    let config = StreamConfig {
        block: 64,
        channels: None,
        render: RenderConfig::at(RATE),
    };
    let stream = Stream::open(graph, &expr(target), config, None, &NoStore).now();
    RefCell::new(stream.unwrap_or_else(|e| panic!("{e}")))
}

fn built(stream: &RefCell<Stream>) -> Built {
    stream.borrow().counts().built
}

fn add(stream: &RefCell<Stream>, graph: &Graph, term: &str) -> Handle {
    let handle = added(stream, graph, &expr(term), &NoStore).now();
    let handle = handle.unwrap_or_else(|e| panic!("`{term}`: {e}"));
    next(&mut stream.borrow_mut()).expect("a block");
    handle
}

fn edit(stream: &RefCell<Stream>, graph: &Graph, target: &str) -> Built {
    let edit = edited(stream, graph, &expr(target), &NoStore).now();
    edit.unwrap_or_else(|e| panic!("`{target}`: {e}"));
    built(stream)
}

fn pluck(k: usize, f0: usize) -> String {
    format!("@pluck(t - {}sp, f0={f0})", 64 * k)
}

fn instances(master: usize, terms: usize) -> usize {
    master + 2 * terms
}

/// Target, notes, room, bed, three pads and air.
#[test]
fn an_open_builds_its_whole_master() {
    let built = built(&opened(&composition(), MASTER));
    assert_eq!((built.instances, built.typed, built.lookups), (8, 8, 8));
}

/// The new pluck, its term, the note sum, the room and the target, however many terms and
/// branches it joins; naming instances alone walks them all.
#[test]
fn an_add_builds_its_term_and_the_chain_reading_the_note_sum() {
    let graph = composition();
    for (master, size) in [(MASTER, 8), (WIDER, 10)] {
        let stream = opened(&graph, master);
        for k in 0..8 {
            add(&stream, &graph, &pluck(k, 200 + 50 * k));
            let share = Built {
                instances: instances(size, k + 1),
                typed: 5,
                values: 6,
                lookups: 1,
            };
            assert_eq!(built(&stream), share, "{master}, term {k}");
        }
    }
}

/// A replace builds what an add does; a remove, the term it crops and the chain above it.
#[test]
fn a_replace_or_remove_builds_only_its_term_and_the_chain() {
    let graph = composition();
    let stream = opened(&graph, MASTER);
    for k in 0..4 {
        add(&stream, &graph, &pluck(k, 300 + 50 * k));
    }
    let held = add(&stream, &graph, &pluck(4, 640));
    let again = (held, &expr(&pluck(4, 645)));
    assert_eq!(
        replaced(&stream, &graph, again, &NoStore).now().ok(),
        Some(true)
    );
    let share = Built {
        instances: instances(8, 5),
        typed: 5,
        values: 6,
        lookups: 1,
    };
    assert_eq!(built(&stream), share);
    assert_eq!(removed(&stream, held, &NoStore).now().ok(), Some(true));
    let cropped = Built {
        typed: 4,
        values: 4,
        lookups: 0,
        ..share
    };
    assert_eq!(built(&stream), cropped);
}

/// The chain from what it moved up to the target; a new bed asks again for what it reads.
#[test]
fn an_edit_builds_only_the_chain_it_moved() {
    let graph = composition();
    let stream = opened(&graph, MASTER);
    for k in 0..4 {
        add(&stream, &graph, &pluck(k, 300 + 50 * k));
    }
    let whole = instances(8, 4);
    let nothing = Built {
        instances: whole,
        ..Built::default()
    };
    assert_eq!(edit(&stream, &graph, MASTER), nothing);
    let gain = edit(&stream, &graph, "@room(t, x=@notes) + 0.4*@bed");
    let target = Built {
        typed: 1,
        values: 1,
        ..nothing
    };
    assert_eq!(gain, target);
    let top = edit(&stream, &graph, "@room(t, x=@notes) + 0.4*@bed(t, top=111)");
    let chain = Built {
        typed: 3,
        values: 4,
        lookups: 5,
        ..nothing
    };
    assert_eq!(top, chain);
}
