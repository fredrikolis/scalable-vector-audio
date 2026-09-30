// Concern: names each instance by its source: its file's text, its bindings and what it reads | Non-concern: typing, a typed node's identity (refs/) | IO: (&Graph, Instances, Order) -> Hash per instance

use std::collections::BTreeMap;

use sva_ast::Graph;
use sva_formula::{Hash, Lanes};

use crate::instantiate::Instances;
use crate::schedule::Order;

const SOURCE_ROTATE: u32 = 29;

struct Sink(Lanes<SOURCE_ROTATE>);

impl Sink {
    fn new(what: &str) -> Sink {
        let mut sink = Sink(Lanes::default());
        sink.text(what);
        sink
    }

    fn text(&mut self, what: &str) {
        self.0.word(what.len() as u64);
        for chunk in what.as_bytes().chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0.word(u64::from_le_bytes(word));
        }
    }

    fn hash(&mut self, held: Hash) {
        self.0.word(held.0);
        self.0.word(held.1);
    }
}

/// Each instance's identity, dependencies first: a loop's members are named together, so
/// each names every other.
pub(crate) fn identities(graph: &Graph, inst: &Instances, order: &Order) -> BTreeMap<String, Hash> {
    let mut named: BTreeMap<String, Hash> = BTreeMap::new();
    for group in &order.groups {
        let outside = |path: &String| !group.contains(path);
        let mut whole = Sink::new(match order.is_loop(group) {
            true => "loop",
            false => "node",
        });
        for path in group {
            own(&mut whole, graph, inst, path);
            for read in order.deps(path).iter().filter(|d| outside(d)) {
                whole.text(read);
                whole.hash(named[read]);
            }
        }
        let whole = whole.0.finish();
        for path in group {
            let mut one = Sink::new("member");
            one.hash(whole);
            one.text(path);
            named.insert(path.clone(), one.0.finish());
        }
    }
    named
}

/// The file's whole text, unparsed, and each binding as the instance resolved it.
fn own(sink: &mut Sink, graph: &Graph, inst: &Instances, path: &str) {
    let file = inst.origin(path).expect("an instance of a file");
    sink.text(file);
    sink.text(graph.text(file).expect("every node holds its text"));
    sink.0.word(graph.seconds_per_bar().map_or(0, f64::to_bits));
    for (name, expr, cx) in inst.bindings(path).expect("an instance") {
        sink.text(name);
        sink.text(&inst.render(expr, cx));
    }
}
