// Concern: proves a stream change builds only what it changed | Non-concern: the samples it plays (stream.rs, edit.rs) | IO: (a stream, changes) -> Built per change

use std::cell::RefCell;

use sva_ast::{Expr, Graph};
use sva_engine::{Built, NoStore, RenderConfig, Stream, StreamConfig};

use crate::fixtures::{Now, added, edited, graph_of, next};

const RATE: u32 = 8_000;

fn expr(text: &str) -> Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {e:?}"))
}

/// Many branches, one reading the note sum.
const MASTER: &str = "@room(t, x=@notes) + 0.5*@bed";

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
                "@pad(t, f0=55) + @pad(t, f0=82.5) + @pad(t, f0=110) + @air\n",
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

fn add(stream: &RefCell<Stream>, graph: &Graph, term: &str) -> Built {
    added(stream, graph, &expr(term), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("`{term}`: {e}"));
    next(&mut stream.borrow_mut()).expect("a block");
    built(stream)
}

fn pluck(k: usize, f0: usize) -> String {
    format!("@pluck(t - {}sp, f0={f0})", 64 * k)
}

/// An open builds each of the master's eight instances: target, notes, room, bed, pads, air.
#[test]
fn an_open_builds_its_whole_master() {
    let built = built(&opened(&composition(), MASTER));
    assert_eq!((built.instances, built.typed, built.lookups), (8, 8, 8));
}

/// An add asks only for the pluck its term newly reads, met before or not.
#[test]
fn an_add_asks_its_store_only_for_what_its_term_newly_reads() {
    let graph = composition();
    let stream = opened(&graph, MASTER);
    for k in 0..6 {
        assert_eq!(add(&stream, &graph, &pluck(k, 200 + 50 * k)).lookups, 1);
    }
    assert_eq!(add(&stream, &graph, &pluck(6, 200)).lookups, 1);
    let quieter = expr("@room(t, x=@notes) + 0.4*@bed");
    edited(&stream, &graph, &quieter, &NoStore)
        .now()
        .expect("edited");
    assert_eq!(built(&stream).lookups, 0, "the edit reads nothing anew");
}
