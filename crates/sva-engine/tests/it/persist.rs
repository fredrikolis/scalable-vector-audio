// Concern: proves the persistent store answers a warm render, wipes on a version change, keeps its budget | Non-concern: any real medium | IO: (a composition, a fake backend) -> stats

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::fixtures::{graph_of, samples};
use sva_ast::Graph;
use sva_engine::{
    Backend, Cache, CacheStats, Hash, Outcome, Render, RenderConfig, STORE_VERSION, Store,
    VERSION_NAME, render_through,
};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

#[derive(Clone, Default)]
struct Memory(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

impl Memory {
    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }

    fn entries(&self) -> Vec<String> {
        let skip = [VERSION_NAME, "recency"];
        self.names()
            .into_iter()
            .filter(|n| !skip.contains(&n.as_str()))
            .collect()
    }

    fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(name).cloned()
    }

    fn set(&self, name: &str, bytes: Vec<u8>) {
        self.0.lock().unwrap().insert(name.to_string(), bytes);
    }
}

impl Backend for Memory {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.bytes(name))
    }

    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.set(name, bytes.to_vec());
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        let held = self.0.lock().unwrap();
        Ok(held
            .iter()
            .map(|(n, b)| (n.clone(), b.len() as u64))
            .collect())
    }
}

/// The fake never waits, so one poll finishes every future it makes.
fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("the in-memory backend never waits"),
    }
}

/// A fresh in-memory store over `memory`, as a new process opens it.
fn opened(memory: &Memory, max_bytes: u64) -> Store<Memory> {
    now(Store::open(memory.clone(), Cache::new(), max_bytes)).expect("the store opens")
}

fn rendered(graph: &Graph, store: &Store<Memory>) -> Render {
    now(render_through(
        graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        store,
    ))
    .expect("a render")
}

fn stats(render: &Render) -> &CacheStats {
    render
        .cache_stats
        .as_ref()
        .expect("a render through a store reports on it")
}

fn two_voices(name: &str, y: u32) -> Graph {
    graph_of(
        name,
        &[
            ("x", "sample(sin(2*pi*220*t))*0.5\n"),
            ("y", &format!("sample(sin(2*pi*{y}*t))*0.5\n")),
            ("master", "@x*0.5 + @y*0.25\n"),
        ],
    )
}

fn outcomes(stats: &CacheStats, node: &str) -> Vec<Outcome> {
    let found: Vec<Outcome> = stats
        .lookups
        .iter()
        .filter(|l| l.node == node)
        .map(|l| l.outcome)
        .collect();
    assert!(!found.is_empty(), "`{node}` was looked up: {stats:?}");
    found
}

#[test]
fn a_warm_store_answers_every_value_and_writes_nothing_until_persist() {
    let memory = Memory::default();
    let graph = two_voices("warm", 330);
    let store = opened(&memory, u64::MAX);
    let cold = rendered(&graph, &store);
    assert!(memory.entries().is_empty(), "a render writes nothing");
    assert!(now(store.persist()).expect("persisted").written > 0);
    assert!(!memory.entries().is_empty());

    let warm = rendered(&graph, &opened(&memory, u64::MAX));
    let stats = stats(&warm);
    assert_eq!(stats.computed(), 0, "every value is a hit: {stats:?}");
    assert!(stats.lookups.iter().all(|l| l.store != Some(false)));
    assert!(stats.lookups.iter().any(|l| l.store == Some(true)));
    assert_eq!(samples(&cold), samples(&warm), "a hit is the bits computed");
    let (cold, warm) = (
        cold.labels[&cold.root].clone(),
        warm.labels[&warm.root].clone(),
    );
    assert_eq!(cold, warm, "a hit carries its label");
}

#[test]
fn an_edit_recomputes_only_the_edited_node_and_its_readers() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("edit-before", 330), &store);
    now(store.persist()).expect("persisted");

    let edited = rendered(&two_voices("edit-after", 440), &opened(&memory, u64::MAX));
    let stats = stats(&edited);
    assert!(outcomes(stats, "x").iter().all(|o| *o == Outcome::Hit));
    for node in ["y", "master"] {
        assert!(outcomes(stats, node).iter().any(|o| *o != Outcome::Hit));
    }
}

#[test]
fn a_store_another_version_wrote_is_wiped_on_open() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("version", 330), &store);
    now(store.persist()).expect("persisted");
    assert!(!memory.entries().is_empty());

    memory.set(VERSION_NAME, b"sva-engine 0.0.0 format 0".to_vec());
    let reopened = opened(&memory, u64::MAX);
    assert_eq!(memory.names(), vec![VERSION_NAME.to_string()]);
    assert_eq!(
        memory.bytes(VERSION_NAME),
        Some(STORE_VERSION.as_bytes().to_vec())
    );
    assert_eq!(reopened.bytes(), 0);
}

#[test]
fn a_truncated_entry_is_a_miss_and_is_written_again() {
    let memory = Memory::default();
    let graph = two_voices("truncated", 330);
    let store = opened(&memory, u64::MAX);
    let cold = rendered(&graph, &store);
    now(store.persist()).expect("persisted");

    let root = stats(&cold)
        .lookups
        .last()
        .expect("the root was looked up")
        .key;
    let name = format!("{:016x}{:016x}", root.0, root.1);
    let whole = memory.bytes(&name).expect("an entry");
    memory.set(&name, whole[..whole.len() / 2].to_vec());
    let store = opened(&memory, u64::MAX);
    let warm = rendered(&graph, &store);
    let missed = stats(&warm).lookups.iter().find(|l| l.key == root);
    assert_eq!(
        missed.and_then(|l| l.store),
        Some(false),
        "the root was a miss"
    );
    assert_eq!(samples(&cold), samples(&warm));
    now(store.persist()).expect("persisted");
    assert_eq!(memory.bytes(&name), Some(whole), "the entry is whole again");
}

fn tone(name: &str, hz: u32) -> Graph {
    graph_of(name, &[("master", &format!("sample(sin(2*pi*{hz}*t))\n"))])
}

/// Each tone's own keys, and the bytes its render persisted.
fn persisted_tone(memory: &Memory, max_bytes: u64, hz: u32) -> (Vec<Hash>, u64) {
    let store = opened(memory, max_bytes);
    let before = store.bytes();
    let render = rendered(&tone(&format!("tone-{hz}"), hz), &store);
    now(store.persist()).expect("persisted");
    assert!(store.bytes() <= max_bytes, "the store keeps its budget");
    let keys = stats(&render).lookups.iter().map(|l| l.key).collect();
    (keys, store.bytes().saturating_sub(before))
}

fn all_held(memory: &Memory, keys: &[Hash]) -> bool {
    let store = opened(memory, u64::MAX);
    keys.iter().all(|k| store.holds(*k))
}

#[test]
fn past_its_budget_the_store_evicts_the_least_recently_used_first() {
    let (_, one) = persisted_tone(&Memory::default(), u64::MAX, 100);
    let budget = one * 5 / 2;
    let memory = Memory::default();
    let (a, _) = persisted_tone(&memory, budget, 100);
    let (b, _) = persisted_tone(&memory, budget, 200);
    persisted_tone(&memory, budget, 100);
    let (c, _) = persisted_tone(&memory, budget, 300);
    assert!(all_held(&memory, &a), "read since, so kept");
    assert!(!all_held(&memory, &b), "least recently used, so gone");
    assert!(all_held(&memory, &c), "just written, so kept");
}
