// Concern: proves a store answers a render or a stream from its root down, stages until persist, keeps a version and a budget | Non-concern: any real medium | IO: (composition, fake backend) -> hits

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::fixtures::{graph_of, samples};
use sva_ast::Graph;
use sva_engine::{
    Backend, CacheStats, Hash, INDEX_NAME, NoStore, Outcome, Range, Render, RenderConfig,
    STORE_FORMAT, Store, Stream, StreamConfig, render, render_through,
};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// One map of names; a staging area's names sit under its prefix, where `list` never looks.
/// While `refusing` is up, every rename fails. `reads` logs each read: the name and the bytes it
/// answered.
#[derive(Clone, Default)]
struct Memory {
    held: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    prefix: String,
    refusing: Arc<AtomicBool>,
    reads: Arc<Mutex<Vec<(String, usize)>>>,
    lists: Arc<AtomicUsize>,
}

impl Memory {
    fn names(&self) -> Vec<String> {
        self.held
            .lock()
            .unwrap()
            .keys()
            .filter(|n| !n.contains('/'))
            .cloned()
            .collect()
    }

    fn staged(&self) -> Vec<String> {
        let held = self.held.lock().unwrap();
        held.keys().filter(|n| n.contains('/')).cloned().collect()
    }

    fn at(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    fn entries(&self) -> Vec<String> {
        let names = self.names().into_iter();
        names.filter(|n| n != INDEX_NAME).collect()
    }

    fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.held.lock().unwrap().get(&self.at(name)).cloned()
    }

    fn set(&self, name: &str, bytes: Vec<u8>) {
        self.held.lock().unwrap().insert(self.at(name), bytes);
    }
}

impl Backend for Memory {
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let found = self.bytes(name);
        let read = (self.at(name), found.as_ref().map_or(0, Vec::len));
        self.reads.lock().unwrap().push(read);
        Ok(found)
    }

    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.set(name, bytes.to_vec());
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<(), String> {
        self.held.lock().unwrap().remove(&self.at(name));
        Ok(())
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        self.lists.fetch_add(1, Ordering::Relaxed);
        let held = self.held.lock().unwrap();
        Ok(held
            .iter()
            .filter_map(|(n, b)| Some((n.strip_prefix(&self.prefix)?, b)))
            .filter(|(n, _)| !n.contains('/'))
            .map(|(n, b)| (n.to_string(), b.len() as u64))
            .collect())
    }

    async fn staging(&self) -> Result<Memory, String> {
        Ok(Memory {
            prefix: format!("{}staging/", self.prefix),
            ..self.clone()
        })
    }

    async fn rename(&self, name: &str, to: &Memory) -> Result<(), String> {
        if self.refusing.load(Ordering::Relaxed) {
            return Err("the medium refused".to_string());
        }
        let mut held = self.held.lock().unwrap();
        let bytes = held.remove(&self.at(name)).ok_or("nothing staged")?;
        held.insert(to.at(name), bytes);
        Ok(())
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
    now(Store::open(memory.clone(), max_bytes)).expect("the store opens")
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
fn a_store_another_format_wrote_is_wiped_on_open() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("version", 330), &store);
    now(store.persist()).expect("persisted");
    assert!(!memory.entries().is_empty());

    memory.set(INDEX_NAME, b"sva-engine 0.0.0 format 0\n".to_vec());
    let reopened = opened(&memory, u64::MAX);
    assert_eq!(memory.names(), vec![INDEX_NAME.to_string()]);
    assert_eq!(memory.bytes(INDEX_NAME), Some(format_only()));
    assert_eq!(reopened.bytes(), 0);
}

/// The index as every build at this format writes it for an empty store, whatever its engine
/// version or sources: the bytes a store in a user's browser keeps across releases.
fn format_only() -> Vec<u8> {
    format!("sva store format {STORE_FORMAT}\n").into_bytes()
}

/// A store another build wrote at this format, as a release that changes no stored value
/// finds it, opens with every entry.
#[test]
fn a_store_survives_a_build_change_that_keeps_its_format() {
    let memory = Memory::default();
    let graph = two_voices("build", 330);
    let store = opened(&memory, u64::MAX);
    let cold = rendered(&graph, &store);
    now(store.persist()).expect("persisted");
    let index = memory.bytes(INDEX_NAME).expect("an index");
    assert!(
        index.starts_with(&format_only()),
        "the version is the format alone"
    );
    let entries = memory.entries();

    let warm = rendered(&graph, &opened(&memory, u64::MAX));
    assert_eq!(memory.entries(), entries, "nothing was wiped");
    assert_eq!(stats(&warm).computed(), 0, "{:?}", stats(&warm));
    assert_eq!(bits(&cold), bits(&warm));
}

/// Opening a store reads one file, its index, however many entries it holds, and lists none.
#[test]
fn opening_a_store_reads_its_index_alone() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("index", 330), &store);
    now(store.persist()).expect("persisted");
    assert!(memory.entries().len() > 1);
    memory.reads.lock().unwrap().clear();
    memory.lists.store(0, Ordering::Relaxed);

    let reopened = opened(&memory, u64::MAX);
    let reads = memory.reads.lock().unwrap().clone();
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert_eq!(reads[0].0, INDEX_NAME);
    assert_eq!(memory.lists.load(Ordering::Relaxed), 0);
    assert!(reopened.bytes() > 0, "the index names every entry");
}

/// The format a digest of these values' stored bytes was pinned under, and the digest.
const PINNED: (u32, u64) = (3, 6023739010022230036);

/// A change to how a value is encoded, or to what the engine computes for any construct here,
/// fails this until `STORE_FORMAT` is bumped and the digest pinned again: rows, a filter, a
/// shifted read, a loop, a solver under a min-and-max ramp, a pointwise form and noise.
#[test]
fn what_a_store_writes_changes_only_with_its_format() {
    let damped = "chaigne_askenfelt(261.63, damper_r=0.1*crop(max(0, min(1, (t - 0.02s)/0.03s)), \
        0.02s, inf))\n";
    let graph = graph_of(
        "pinned",
        &[
            ("x", "sample(sin(2*pi*220*t))*0.5\n"),
            (
                "f",
                "lowpass(sample(sin(2*pi*220*t)) + sample(sin(2*pi*3000*t))*0.3, cutoff=800)\n",
            ),
            ("echo", "@x + 0.5*self[idx(t - 0.01s)]\n"),
            ("s", damped),
            (
                "chirp",
                "sample(sin(2*pi*(200 + 900*t)*t))*step(t - 0.01s)\n",
            ),
            ("hiss", "lowpass(noise(7, period=0.25), cutoff=1000)*0.1\n"),
            (
                "master",
                "@echo*0.5 + @f(t - 0.01s) + @s*0.25 + @chirp*0.2 + @hiss\n",
            ),
        ],
    );
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&graph, &store);
    now(store.persist()).expect("persisted");
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    for name in memory.entries() {
        let bytes = memory.bytes(&name).expect("an entry");
        for b in name.bytes().chain(bytes) {
            digest = (digest ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(
        (STORE_FORMAT, digest),
        PINNED,
        "stored bytes changed: bump STORE_FORMAT and pin the new digest"
    );
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

/// Two stores over one directory, as two workers of a page open it: what one persists, the
/// other, opened before, answers from.
#[test]
fn a_store_answers_what_another_over_its_directory_persisted_after_it_opened() {
    let memory = Memory::default();
    let graph = two_voices("shared", 330);
    let (first, second) = (opened(&memory, u64::MAX), opened(&memory, u64::MAX));
    rendered(&graph, &first);
    now(first.persist()).expect("persisted");
    let warm = rendered(&graph, &second);
    assert_eq!(stats(&warm).computed(), 0, "{:?}", stats(&warm));
}

#[test]
fn stores_sharing_a_directory_keep_one_budget_over_what_both_persisted() {
    let (_, one) = persisted_tone(&Memory::default(), u64::MAX, 100);
    let memory = Memory::default();
    let (first, second) = (opened(&memory, one * 3 / 2), opened(&memory, one * 3 / 2));
    rendered(&tone("tone-100", 100), &first);
    now(first.persist()).expect("persisted");
    rendered(&tone("tone-200", 200), &second);
    now(second.persist()).expect("persisted");
    let held: u64 = memory
        .entries()
        .iter()
        .map(|n| memory.bytes(n).map_or(0, |b| b.len() as u64))
        .sum();
    assert!(
        held <= one * 3 / 2,
        "{held} bytes held past a budget of {}",
        one * 3 / 2
    );
}

fn rendered_over(graph: &Graph, store: &Store<Memory>, seconds: f64) -> Render {
    now(render_through(
        graph,
        "master",
        RenderConfig::seconds(RATE, seconds),
        store,
    ))
    .expect("a render")
}

fn bits(render: &Render) -> Vec<Vec<u64>> {
    let root = render.output(render.root).expect("the root's samples");
    root.planes
        .iter()
        .map(|plane| plane.iter().map(|v| v.to_bits()).collect())
        .collect()
}

fn sorted(names: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = names.iter().map(String::as_str).collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[test]
fn a_warm_render_whose_root_hits_visits_one_key_and_types_and_plans_nothing() {
    let memory = Memory::default();
    let graph = two_voices("root-hit", 330);
    let store = opened(&memory, u64::MAX);
    let cold = rendered(&graph, &store);
    now(store.persist()).expect("persisted");

    let warm = rendered(&graph, &opened(&memory, u64::MAX));
    let stats = stats(&warm);
    assert_eq!(stats.lookups.len(), 1, "one key: {stats:?}");
    assert_eq!(stats.lookups[0].node, "master");
    assert_eq!(stats.lookups[0].outcome, Outcome::Hit);
    assert!(stats.typed.is_empty(), "typed {:?}", stats.typed);
    assert!(stats.planned.is_empty(), "planned {:?}", stats.planned);
    assert_eq!(bits(&cold), bits(&warm));
}

/// `master` reads `e` through `p`; `s` and `q` are the siblings on the way down.
fn nested(name: &str, hz: u32) -> Graph {
    graph_of(
        name,
        &[
            ("e", &format!("sample(sin(2*pi*{hz}*t))*0.5\n")),
            ("q", "sample(sin(2*pi*550*t))*0.25\n"),
            ("s", "sample(sin(2*pi*330*t))*0.5\n"),
            ("p", "@e*0.5 + @q\n"),
            ("master", "@p + @s*0.5\n"),
        ],
    )
}

#[test]
fn an_edit_visits_the_missed_path_and_its_hit_siblings_and_types_only_the_missed() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&nested("path-before", 220), &store);
    now(store.persist()).expect("persisted");

    let edited = nested("path-after", 440);
    let warm = rendered(&edited, &opened(&memory, u64::MAX));
    let stats = stats(&warm);
    let visited: Vec<String> = stats.lookups.iter().map(|l| l.node.clone()).collect();
    assert_eq!(
        sorted(&visited),
        ["e", "master", "p", "q", "s"],
        "{stats:?}"
    );
    assert_eq!(visited.len(), 5, "each node is visited once");
    for lookup in &stats.lookups {
        let hit = ["q", "s"].contains(&lookup.node.as_str());
        assert_eq!(lookup.outcome == Outcome::Hit, hit, "{lookup:?}");
    }
    assert_eq!(sorted(&stats.typed), ["e", "master", "p"]);
    assert_eq!(sorted(&stats.planned), ["e", "master", "p"]);
    let fresh = render(
        &edited,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        None,
    );
    assert_eq!(
        bits(&warm),
        bits(&fresh.expect("a render")),
        "hits read as computed"
    );
}

#[test]
fn a_cold_render_through_a_store_writes_the_bits_a_render_without_one_does() {
    let graph = graph_of(
        "cold-bits",
        &[
            (
                "x",
                "lowpass(sample(sin(2*pi*220*t)) + sample(sin(2*pi*3000*t))*0.3, cutoff=800)\n",
            ),
            ("y", "@x(t - 0.01s)*0.5\n"),
            ("z", "sample(sin(2*pi*440*t))*0.5\n"),
            ("master", "@x*0.5 + @y + @z*0.25\n"),
        ],
    );
    let config = RenderConfig::seconds(RATE, 1.2);
    let fresh = render(&graph, "master", config.clone(), None).expect("a render");
    let store = opened(&Memory::default(), u64::MAX);
    let cold = now(render_through(&graph, "master", config, &store)).expect("a render");
    assert!(
        bits(&fresh)[0].iter().any(|b| *b != 0),
        "silence tests nothing"
    );
    assert_eq!(bits(&cold), bits(&fresh));
}

#[test]
fn a_render_stages_what_it_drops_and_only_persist_moves_it_into_the_store() {
    let memory = Memory::default();
    let graph = two_voices("staging", 330);
    let store = opened(&memory, u64::MAX);
    let cold = rendered_over(&graph, &store, 2.0);
    assert!(memory.entries().is_empty(), "the store is unchanged");
    let staged: u64 = memory
        .staged()
        .iter()
        .map(|name| memory.held.lock().unwrap()[name].len() as u64)
        .sum();
    assert!(staged > 0, "the render staged what it computed");
    assert!(
        (cold.held_bytes as u64) < staged,
        "the render held {} bytes at most, less than the {staged} it staged",
        cold.held_bytes
    );
    let done = now(store.persist()).expect("persisted");
    assert_eq!(done.written, memory.entries().len());
    assert!(memory.staged().is_empty(), "every staged value moved");
}

#[test]
fn a_comment_changes_a_nodes_identity_and_a_file_nothing_reads_changes_none() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let before = rendered(&two_voices("comment-before", 330), &store);
    now(store.persist()).expect("persisted");
    let key = |render: &Render, node: &str| {
        stats(render)
            .lookups
            .iter()
            .find(|l| l.node == node)
            .map(|l| l.key)
    };

    let commented = graph_of(
        "comment-after",
        &[
            ("x", "sample(sin(2*pi*220*t))*0.5\n"),
            ("y", "sample(sin(2*pi*330*t))*0.5\n"),
            ("master", "; the mix\n@x*0.5 + @y*0.25\n"),
        ],
    );
    let after = rendered(&commented, &opened(&memory, u64::MAX));
    assert_ne!(key(&before, "master"), key(&after, "master"));
    assert!(
        outcomes(stats(&after), "master")
            .iter()
            .all(|o| *o != Outcome::Hit)
    );
    assert!(
        outcomes(stats(&after), "x")
            .iter()
            .all(|o| *o == Outcome::Hit)
    );

    let beside = graph_of(
        "unread-after",
        &[
            ("x", "sample(sin(2*pi*220*t))*0.5\n"),
            ("y", "sample(sin(2*pi*330*t))*0.5\n"),
            ("master", "@x*0.5 + @y*0.25\n"),
            ("unread", "sample(sin(2*pi*990*t))\n"),
        ],
    );
    let unread = rendered(&beside, &opened(&memory, u64::MAX));
    assert_eq!(key(&before, "master"), key(&unread, "master"));
    assert_eq!(stats(&unread).lookups.len(), 1, "the root hits");
}

#[test]
fn a_persist_that_fails_leaves_every_value_it_did_not_commit_staged() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("refused", 330), &store);
    memory.refusing.store(true, Ordering::Relaxed);
    assert!(now(store.persist()).is_err());
    assert!(memory.entries().is_empty());

    memory.refusing.store(false, Ordering::Relaxed);
    let done = now(store.persist()).expect("persisted");
    assert_eq!(done.written, 3, "x, y and master were still staged");
    assert_eq!(memory.entries().len(), 3);
}

/// A note one worker rendered and persisted, a stream in another answers from the store: the
/// term stands as its samples under the loop that reads it, bit for bit the cold stream.
#[test]
fn a_stream_reads_a_note_another_store_over_its_directory_persisted() {
    let graph = graph_of(
        "streamed",
        &[
            (
                "blip",
                "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.1s)\n",
            ),
            ("echo", "x + 0.5*self[idx(t - 0.05s)]\n"),
            ("warm", "@blip(t, f0=200)\n"),
        ],
    );
    let memory = Memory::default();
    let (renderer, player) = (opened(&memory, u64::MAX), opened(&memory, u64::MAX));
    now(render_through(
        &graph,
        "warm",
        RenderConfig::seconds(RATE, 0.1),
        &renderer,
    ))
    .expect("a render");
    now(renderer.persist()).expect("persisted");

    let expr = |text: &str| sva_ast::parse_expr(text).expect("an expression");
    let config = StreamConfig {
        block: 256,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(RATE.into()),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let (master, term) = (expr("@echo(t, x=@notes)"), expr("@blip(t - 512sp, f0=200)"));
    let cold = now(Stream::open(
        &graph,
        &master,
        config.clone(),
        None,
        &NoStore,
    ));
    let mut cold = cold.expect("a stream");
    let mut warm = now(Stream::open(&graph, &master, config, None, &player)).expect("a stream");
    now(cold.add(&graph, &term, &NoStore)).expect("added");
    now(warm.add(&graph, &term, &player)).expect("added");
    for _ in 0..(RATE / 256) {
        let (cold, warm) = (
            cold.next_block().expect("a block"),
            warm.next_block().expect("a block"),
        );
        assert_eq!(
            warm.map(|b| b.plane(0).to_vec()),
            cold.map(|b| b.plane(0).to_vec())
        );
    }
    let hits = warm
        .stats()
        .lookups
        .into_iter()
        .filter(|l| l.store == Some(true));
    let hits: Vec<String> = hits.map(|l| l.node).collect();
    assert_eq!(hits, ["blip(f0=200)"], "{:?}", warm.stats());
}

/// A string held until `release`, never released here.
const HELD: &str = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*crop(min(1, \
    (t - release)/0.03s), release, inf))\n";

/// A composition whose `string` a worker warmed over its first `stored` samples and persisted.
fn warmed(name: &str, stored: i64) -> (Graph, Memory) {
    let graph = graph_of(
        name,
        &[("string", HELD), ("warm", "@string(t, f0=261.63)\n")],
    );
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let config = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(stored),
        },
        ..RenderConfig::at(RATE)
    };
    now(render_through(&graph, "warm", config, &store)).expect("a render");
    now(store.persist()).expect("persisted");
    (graph, memory)
}

const STRIKE: &str = "@string(t - 512sp, f0=261.63)";

fn notes(graph: &Graph, end: i64, store: &impl sva_engine::Through) -> Stream {
    let config = StreamConfig {
        block: 256,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(end),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let master = sva_ast::parse_expr("@notes").expect("an expression");
    now(Stream::open(graph, &master, config, None, store)).expect("a stream")
}

fn term(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

fn played(stream: &mut Stream) -> Vec<u64> {
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().expect("a block") {
        heard.extend(block.plane(0).iter().map(|v| v.to_bits()));
    }
    heard
}

fn store_hits(stream: &Stream) -> Vec<String> {
    let hits = stream.stats().lookups.into_iter();
    hits.filter(|l| l.store == Some(true))
        .map(|l| l.node)
        .collect()
}

/// A held note the store holds over all the stream plays of it, key-up and fade included, is
/// played from its samples: it computes nothing, and the blocks are a storeless stream's.
#[test]
fn a_stream_plays_a_stored_held_note_from_its_samples() {
    let (graph, memory) = warmed("held-stored", 2_000);
    let fade = format!("{STRIKE} * (1 - step(t - 1200sp)*(1 - exp(-(t - 1200sp)/0.15s)))");
    let player = opened(&memory, u64::MAX);
    let mut heard = Vec::new();
    for store in [None, Some(&player)] {
        let mut stream = match store {
            Some(store) => notes(&graph, 2_000, store),
            None => notes(&graph, 2_000, &NoStore),
        };
        let handle = match store {
            Some(store) => now(stream.add(&graph, &term(STRIKE), store)),
            None => now(stream.add(&graph, &term(STRIKE), &NoStore)),
        };
        let handle = handle.expect("added");
        let mut blocks = Vec::new();
        for _ in 0..4 {
            let block = stream.next_block().expect("a block").expect("a block");
            blocks.extend(block.plane(0).iter().map(|v| v.to_bits()));
        }
        let keyed = (handle, &term(&fade));
        let replaced = match store {
            Some(store) => now(stream.replace(&graph, keyed, store)),
            None => now(stream.replace(&graph, keyed, &NoStore)),
        };
        assert!(replaced.expect("replaced"));
        blocks.extend(played(&mut stream));
        heard.push((blocks, stream.stats().lookups));
    }
    let (cold, warm) = (&heard[0], &heard[1]);
    assert!(cold.0.iter().any(|b| *b != 0), "silence tests nothing");
    assert_eq!(
        warm.0, cold.0,
        "the stored note is the live one, bit for bit"
    );
    let note = |lookups: &[sva_engine::Lookup]| -> Vec<(Outcome, Option<bool>)> {
        let of = lookups
            .iter()
            .filter(|l| l.node == "string(f0=261.63, release=inf)");
        of.map(|l| (l.outcome, l.store)).collect()
    };
    assert!(
        !note(&cold.1).is_empty(),
        "the storeless stream computes the note"
    );
    assert!(!note(&warm.1).is_empty());
    assert!(
        note(&warm.1)
            .iter()
            .all(|l| *l == (Outcome::Hit, Some(true))),
        "the store answers the note, which computes nothing: {:?}",
        note(&warm.1)
    );
}

/// Held past what the store holds, the note continues live, seamlessly: an exact stream and a
/// live one both play a storeless stream's blocks, and the live one starts nothing silent.
#[test]
fn a_held_note_longer_than_the_store_continues_live_seamlessly() {
    let (graph, memory) = warmed("held-longer", 2_000);
    let mut cold = notes(&graph, 8_000, &NoStore);
    now(cold.add(&graph, &term(STRIKE), &NoStore)).expect("added");
    let cold = played(&mut cold);
    for live in [false, true] {
        let player = opened(&memory, u64::MAX);
        let mut warm = notes(&graph, 8_000, &player);
        if live {
            warm.go_live();
        }
        now(warm.add(&graph, &term(STRIKE), &player)).expect("added");
        let first = warm.next_block().expect("a block").expect("a block");
        assert_eq!(
            warm.work().priced_flops,
            0,
            "live {live}: the first block is stored"
        );
        let mut heard: Vec<u64> = first.plane(0).iter().map(|v| v.to_bits()).collect();
        heard.extend(played(&mut warm));
        let first = heard.iter().zip(&cold).position(|(a, b)| a != b);
        eprintln!(
            "live {live} len {} {} first diff {first:?} dropped {:?}",
            heard.len(),
            cold.len(),
            warm.dropped()
        );
        assert!(heard == cold, "live {live}");
        assert!(
            warm.dropped().is_empty(),
            "live {live}: {:?}",
            warm.dropped()
        );
        assert!(!store_hits(&warm).is_empty(), "live {live}");
    }
}
