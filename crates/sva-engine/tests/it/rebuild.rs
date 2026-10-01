// Concern: proves a stream change takes in, names, walks, types and copies only its own share | Non-concern: the samples it plays (stream.rs, edit.rs) | IO: (a stream, changes) -> Built per change

use std::cell::RefCell;

use sva_ast::{Expr, Graph};
use sva_engine::{Built, Handle, RenderConfig, Stream, StreamConfig, Tier};

use crate::fixtures::{Now, added, edited, graph_of, next, removed, replaced};

const RATE: u32 = 8_000;

/// The note sum through a room, beside a mix of filters no term reads.
const MASTER: &str = "@room(t, x=@notes) + 0.5*@mix";

fn composition(branches: usize) -> Graph {
    let mix: Vec<String> = (0..branches).map(|k| format!("0.01*@b{k}")).collect();
    let mut files = vec![
        (
            "pluck".to_string(),
            "lowpass(sample(crop(sin(2*pi*f0*t)*exp(-t/0.05), 0s, 0.2s)), cutoff=3000)\n"
                .to_string(),
        ),
        (
            "hold".to_string(),
            "lowpass(sample(0.1*sin(2*pi*f0*t)), cutoff=3000)\n".to_string(),
        ),
        (
            "room".to_string(),
            "lowpass(x, cutoff=4000) + 0.3*x(t - 0.01s)\n".to_string(),
        ),
        ("mix".to_string(), format!("0 + {}\n", mix.join(" + "))),
    ];
    for k in 0..branches {
        files.push((
            format!("b{k}"),
            format!(
                "lowpass(sample(0.1*saw({}*t)), cutoff=900, q=0.7)\n",
                55 + k
            ),
        ));
    }
    let named: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    graph_of("rebuild", &named)
}

fn expr(text: &str) -> Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {e:?}"))
}

fn opened(graph: &Graph, target: &str) -> RefCell<Stream> {
    let config = StreamConfig {
        block: 64,
        channels: None,
        render: RenderConfig::at(RATE),
    };
    let stream = Stream::open(graph, &expr(target), config, &Tier::default()).now();
    RefCell::new(stream.unwrap_or_else(|e| panic!("{e}")))
}

fn built(stream: &RefCell<Stream>) -> Built {
    stream.borrow().counts().built
}

fn add(stream: &RefCell<Stream>, graph: &Graph, term: &str) -> Handle {
    let handle = added(stream, graph, &expr(term), &Tier::default()).now();
    handle.unwrap_or_else(|e| panic!("`{term}`: {e}"))
}

/// A held note, which never retires.
fn note(k: usize, f0: usize) -> String {
    format!("@hold(t - {k}sp, f0={f0})")
}

/// A stream holding `terms` terms, a block played.
fn holding(branches: usize, terms: usize) -> (Graph, RefCell<Stream>) {
    let graph = composition(branches);
    let stream = opened(&graph, MASTER);
    for k in 0..terms {
        add(&stream, &graph, &note(k, 100 + k % 7));
    }
    next(&mut stream.borrow_mut()).expect("a block");
    (graph, stream)
}

/// The whole master, the store asked of the mix and its branches alone.
#[test]
fn an_open_names_and_walks_its_whole_master() {
    for branches in [3, 40] {
        let built = built(&opened(&composition(branches), MASTER));
        let whole = 4 + branches;
        let want = Built {
            parsed: 1,
            instances: whole,
            visited: whole,
            typed: whole,
            values: built.values,
            copied: 0,
            lookups: 1 + branches,
        };
        assert_eq!(built, want, "{branches} branches");
    }
}

/// An add takes in its text, names its term and note, and walks, types and copies those and
/// the chain reading the note sum, whatever the master and the terms.
#[test]
fn an_add_names_walks_and_copies_only_its_share() {
    let mut seen = Vec::new();
    for (branches, terms) in [(2, 3), (60, 200)] {
        let (graph, stream) = holding(branches, terms);
        add(&stream, &graph, &note(terms, 300));
        seen.push(built(&stream));
    }
    let share = Built {
        parsed: 1,
        instances: 2,
        visited: 5,
        typed: 5,
        values: seen[0].values,
        copied: 1,
        lookups: 1,
    };
    assert_eq!(seen, [share, share]);
}

/// A replace or a remove walks, types and copies only its term and the chain above it.
#[test]
fn a_replace_or_remove_names_walks_and_copies_only_its_share() {
    let mut seen = Vec::new();
    for (branches, terms) in [(2, 3), (60, 200)] {
        let (graph, stream) = holding(branches, terms);
        let held = add(&stream, &graph, &note(0, 640));
        next(&mut stream.borrow_mut()).expect("a block");
        let again = (held, &expr(&note(0, 645)));
        assert_eq!(
            replaced(&stream, &graph, again, &Tier::default())
                .now()
                .ok(),
            Some(true)
        );
        let replace = built(&stream);
        next(&mut stream.borrow_mut()).expect("a block");
        assert_eq!(
            removed(&stream, held, &Tier::default()).now().ok(),
            Some(true)
        );
        seen.push((replace, built(&stream)));
    }
    assert_eq!(seen[0], seen[1], "independent of the master and the terms");
    let (replace, remove) = seen[0];
    let chain = Built {
        parsed: 1,
        instances: 1,
        visited: 5,
        typed: 5,
        values: replace.values,
        copied: 2,
        lookups: 1,
    };
    assert_eq!(replace, chain);
    let cropped = Built {
        parsed: 0,
        instances: 0,
        visited: 4,
        typed: 4,
        values: remove.values,
        copied: 1,
        lookups: 0,
    };
    assert_eq!(remove, cropped);
}

/// An edit of one parameter builds only the target; one changing nothing, nothing.
#[test]
fn an_edit_of_one_parameter_builds_only_the_target() {
    let mut seen = Vec::new();
    for (branches, terms) in [(2, 3), (60, 200)] {
        let (graph, stream) = holding(branches, terms);
        let edit = |target: &str| {
            let done = edited(&stream, &graph, &expr(target), &Tier::default()).now();
            done.unwrap_or_else(|e| panic!("`{target}`: {e}"));
            built(&stream)
        };
        seen.push((edit(MASTER), edit("@room(t, x=@notes) + 0.4*@mix")));
    }
    assert_eq!(seen[0], seen[1], "independent of the master and the terms");
    let (same, gain) = seen[0];
    assert_eq!(
        same,
        Built {
            parsed: 1,
            ..Built::default()
        }
    );
    assert_eq!(
        gain,
        Built {
            parsed: 1,
            visited: 1,
            typed: 1,
            values: gain.values,
            ..Built::default()
        }
    );
}

/// Reads ask nothing of the note sum until a term's support can have ended.
#[test]
fn a_read_works_out_the_note_sum_only_once_a_term_can_have_ended() {
    let graph = composition(2);
    let stream = opened(&graph, MASTER);
    add(&stream, &graph, "@pluck(t - 640sp, f0=200)");
    for _ in 0..20 {
        next(&mut stream.borrow_mut()).expect("a block");
    }
    assert_eq!(stream.borrow().counts().demands, 0, "the pluck sounds on");
    while stream.borrow().counts().terms == 1 {
        next(&mut stream.borrow_mut()).expect("a block");
    }
    let demands = stream.borrow().counts().demands;
    assert!(demands >= 1, "the pluck ended and retired");
    for _ in 0..20 {
        next(&mut stream.borrow_mut()).expect("a block");
    }
    assert_eq!(
        stream.borrow().counts().demands,
        demands,
        "no term is left to end"
    );
}
