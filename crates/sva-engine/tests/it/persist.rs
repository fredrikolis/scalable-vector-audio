// Concern: proves a store answers a render or a stream from its root down, stages until persist, keeps a version and a budget | Non-concern: any real medium | IO: (composition, fake backend) -> hits

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::fixtures::{added, graph_of, next, replaced, samples};
use sva_ast::Graph;
use sva_engine::{
    Backend, CacheStats, Change, Changed, EngineError, Handle, Hash, INDEX_NAME, NoStore, Outcome,
    Placed, Range, Render, RenderConfig, STORE_FORMAT, Store, Stream, StreamConfig, change, render,
    render_through, warm,
};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// One map of names; each staging area's names sit under its own prefix, where `list` never looks.
/// While `refusing` is up, every write and rename fails. `reads` logs each read: the name and the bytes it
/// answered. A name in `open` is another holder's, so, as in OPFS, it is neither removed nor
/// moved onto.
#[derive(Clone, Default)]
struct Memory {
    held: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    prefix: String,
    refusing: Arc<AtomicBool>,
    reads: Arc<Mutex<Vec<(String, usize)>>>,
    lists: Arc<AtomicUsize>,
    locked: Arc<AtomicBool>,
    open: Arc<Mutex<BTreeSet<String>>>,
}

/// The fake's lock, released when dropped.
struct Held(Arc<AtomicBool>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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

    fn opened(&self, name: &str) -> bool {
        self.open.lock().unwrap().contains(&self.at(name))
    }
}

impl Backend for Memory {
    type Lock = Held;

    async fn lock(&self) -> Result<Held, String> {
        std::future::poll_fn(|_| {
            let free =
                self.locked
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed);
            match free {
                Ok(_) => Poll::Ready(Ok(Held(self.locked.clone()))),
                Err(_) => Poll::Pending,
            }
        })
        .await
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let found = self.bytes(name);
        let read = (self.at(name), found.as_ref().map_or(0, Vec::len));
        self.reads.lock().unwrap().push(read);
        Ok(found)
    }

    async fn get_range(&self, name: &str, from: u64, len: u64) -> Result<Option<Vec<u8>>, String> {
        let found = self.bytes(name).map(|bytes| {
            let from = (from as usize).min(bytes.len());
            let to = from.saturating_add(len as usize).min(bytes.len());
            bytes[from..to].to_vec()
        });
        let read = (self.at(name), found.as_ref().map_or(0, Vec::len));
        self.reads.lock().unwrap().push(read);
        Ok(found)
    }

    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if self.refusing.load(Ordering::Relaxed) {
            return Err("the medium refused".to_string());
        }
        self.set(name, bytes.to_vec());
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<bool, String> {
        if self.opened(name) {
            return Ok(false);
        }
        self.held.lock().unwrap().remove(&self.at(name));
        Ok(true)
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

    /// Each store its own area, as the backend promises: none writes another's.
    async fn staging(&self) -> Result<Memory, String> {
        static AREAS: AtomicUsize = AtomicUsize::new(0);
        let area = AREAS.fetch_add(1, Ordering::Relaxed);
        Ok(Memory {
            prefix: format!("{}staging-{area}/", self.prefix),
            ..self.clone()
        })
    }

    async fn rename(&self, name: &str, to: &Memory) -> Result<bool, String> {
        if self.refusing.load(Ordering::Relaxed) {
            return Err("the medium refused".to_string());
        }
        if self.opened(name) || to.opened(name) {
            return Ok(false);
        }
        let mut held = self.held.lock().unwrap();
        let bytes = held.remove(&self.at(name)).ok_or("nothing staged")?;
        held.insert(to.at(name), bytes);
        Ok(true)
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
const PINNED: (u32, u64) = (10, 7451051165443767813);

/// A change to how a value is encoded, or to what the engine computes for any construct here,
/// fails this until `STORE_FORMAT` is bumped and the digest pinned again: rows, a filter, a
/// shifted read, a loop, a solver under a min-and-max ramp, a pointwise form, noise and a
/// filter's pruned ringing.
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
                "rung",
                "lowpass(sample(crop(sin(2*pi*220*t), 0s, 0.02s)), cutoff=300)\n",
            ),
            (
                "master",
                "@echo*0.5 + @f(t - 0.01s) + @s*0.25 + @chirp*0.2 + @hiss + @rung\n",
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

/// A medium that refuses every write fails no render and no warm: the render's samples are a
/// storeless render's, and its stats say why it staged nothing.
#[test]
fn a_store_refusing_every_write_fails_no_render_and_no_warm() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    memory.refusing.store(true, Ordering::Relaxed);
    let graph = two_voices("refusing", 330);
    let through = rendered(&graph, &store);
    let config = RenderConfig::seconds(RATE, SECONDS);
    let fresh = render(&graph, "master", config.clone(), None).expect("a render");
    assert_eq!(samples(&through), samples(&fresh));
    assert!(stats(&through).unstaged.is_some(), "{:?}", stats(&through));
    let warmed = now(warm(&graph, "master", config, &store)).expect("a warm");
    assert!(warmed.unstaged.is_some(), "{warmed:?}");
    memory.refusing.store(false, Ordering::Relaxed);
    assert_eq!(now(store.persist()).expect("persisted").written, 0);
}

/// Entries another holder has open, as another worker's OPFS handle holds them: a persist
/// neither commits over nor evicts them, and fails at nothing; once they close, the next one does
/// both.
#[test]
fn a_persist_skips_entries_another_holder_has_open_and_the_next_retries_them() {
    let names = {
        let memory = Memory::default();
        let store = opened(&memory, u64::MAX);
        rendered(&two_voices("held", 330), &store);
        now(store.persist()).expect("persisted");
        memory.entries()
    };
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("held", 330), &store);
    memory.open.lock().unwrap().extend(names.iter().cloned());
    let done = now(store.persist()).expect("an open entry is no failure");
    assert_eq!((done.written, memory.entries().len()), (0, 0));
    memory.open.lock().unwrap().clear();
    let done = now(store.persist()).expect("persisted");
    assert_eq!(done.written, names.len(), "each stayed staged");

    let memory = Memory::default();
    let (_, one) = persisted_tone(&memory, u64::MAX, 100);
    let old = memory.entries();
    memory.open.lock().unwrap().extend(old.iter().cloned());
    let store = opened(&memory, one / 2);
    rendered(&tone("tone-200", 200), &store);
    now(store.persist()).expect("an open entry is no failure");
    let kept = |name: &String| memory.bytes(name).is_some();
    assert!(old.iter().all(kept), "open, so kept past the budget");
    memory.open.lock().unwrap().clear();
    now(store.persist()).expect("persisted");
    assert!(
        !old.iter().any(kept),
        "closed, so the next persist evicts it"
    );
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
        channels: None,
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
    let cold = RefCell::new(cold.expect("a stream"));
    let warm = now(Stream::open(&graph, &master, config, None, &player));
    let warm = RefCell::new(warm.expect("a stream"));
    now(added(&cold, &graph, &term, &NoStore)).expect("added");
    now(added(&warm, &graph, &term, &player)).expect("added");
    for _ in 0..(RATE / 256) {
        let (cold, warm) = (
            next(&mut cold.borrow_mut()).expect("a block"),
            next(&mut warm.borrow_mut()).expect("a block"),
        );
        assert_eq!(
            warm.map(|b| b.plane(0).to_vec()),
            cold.map(|b| b.plane(0).to_vec())
        );
    }
    let hits = warm
        .borrow()
        .stats()
        .lookups
        .into_iter()
        .filter(|l| l.store == Some(true));
    let hits: Vec<String> = hits.map(|l| l.node).collect();
    assert_eq!(hits, ["blip(f0=200)"], "{:?}", warm.borrow().stats());
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

fn notes(graph: &Graph, end: i64, store: &impl sva_engine::Through) -> RefCell<Stream> {
    let config = StreamConfig {
        block: 256,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(end),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let master = sva_ast::parse_expr("@notes").expect("an expression");
    RefCell::new(now(Stream::open(graph, &master, config, None, store)).expect("a stream"))
}

fn term(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

fn played(stream: &RefCell<Stream>) -> Vec<u64> {
    let mut heard = Vec::new();
    while let Some(block) = next(&mut stream.borrow_mut()).expect("a block") {
        heard.extend(block.plane(0).iter().map(|v| v.to_bits()));
    }
    heard
}

fn store_hits(stream: &RefCell<Stream>) -> Vec<String> {
    let hits = stream.borrow().stats().lookups.into_iter();
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
        let stream = match store {
            Some(store) => notes(&graph, 2_000, store),
            None => notes(&graph, 2_000, &NoStore),
        };
        let handle = match store {
            Some(store) => now(added(&stream, &graph, &term(STRIKE), store)),
            None => now(added(&stream, &graph, &term(STRIKE), &NoStore)),
        };
        let handle = handle.expect("added");
        let mut blocks = Vec::new();
        for _ in 0..4 {
            let block = next(&mut stream.borrow_mut())
                .expect("a block")
                .expect("a block");
            blocks.extend(block.plane(0).iter().map(|v| v.to_bits()));
        }
        let keyed = (handle, &term(&fade));
        let replaced = match store {
            Some(store) => now(replaced(&stream, &graph, keyed, store)),
            None => now(replaced(&stream, &graph, keyed, &NoStore)),
        };
        assert!(replaced.expect("replaced"));
        blocks.extend(played(&stream));
        heard.push((blocks, stream.borrow().stats().lookups));
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

/// Held past what the store holds, an exact stream computes the rest of the note as it would
/// any value: its blocks are a storeless stream's, bit for bit.
#[test]
fn an_exact_stream_computes_a_held_note_past_what_the_store_holds() {
    let (graph, memory) = warmed("held-longer", 2_000);
    let cold = notes(&graph, 8_000, &NoStore);
    now(added(&cold, &graph, &term(STRIKE), &NoStore)).expect("added");
    let cold = played(&cold);
    let player = opened(&memory, u64::MAX);
    let warm = notes(&graph, 8_000, &player);
    now(added(&warm, &graph, &term(STRIKE), &player)).expect("added");
    let first = next(&mut warm.borrow_mut())
        .expect("a block")
        .expect("a block");
    assert_eq!(
        warm.borrow().work().priced_flops,
        0,
        "the first block is stored"
    );
    let mut heard: Vec<u64> = first.plane(0).iter().map(|v| v.to_bits()).collect();
    heard.extend(played(&warm));
    assert!(heard == cold, "the stored samples, then the computed ones");
    assert!(!store_hits(&warm).is_empty());
}

const LATE: &str = "@string(t - 512sp, f0=261.63)";

/// Four blocks in, adds the late note and plays on.
fn late(graph: &Graph, live: bool, store: &impl sva_engine::Through) -> (Vec<u64>, Vec<String>) {
    let stream = notes(graph, 8_000, store);
    if live {
        stream.borrow_mut().go_live();
    }
    for _ in 0..4 {
        next(&mut stream.borrow_mut())
            .expect("a block")
            .expect("a block");
    }
    now(added(&stream, graph, &term(LATE), store)).expect("added");
    (
        played(&stream),
        stream
            .borrow()
            .dropped()
            .iter()
            .map(|n| n.to_string())
            .collect(),
    )
}

/// A live stream adding a held note after it began plays the stored samples, then drops what
/// is not ready, the note's own value past them, started where the note stands, as a
/// storeless live stream drops the whole note.
#[test]
fn a_live_stream_drops_a_held_note_that_is_not_ready() {
    let (graph, memory) = warmed("held-live", 2_000);
    let (cold, _) = late(&graph, false, &NoStore);
    let (unready, _) = late(&graph, true, &NoStore);
    let player = opened(&memory, u64::MAX);
    let (heard, dropped) = late(&graph, true, &player);
    let stored = (512 + 2_000 - 1_024) as usize;
    assert!(
        cold[..stored].iter().any(|b| *b != 0),
        "silence tests nothing"
    );
    assert_eq!(heard[..stored], cold[..stored], "the stored samples");
    assert_eq!(
        heard[stored..],
        unready[stored..],
        "the note dropped past them"
    );
    assert_eq!(dropped, ["string(f0=261.63, release=inf)"]);
}

/// An entry's header span, off its first eight bytes: all a lookup may read of it.
fn head_of(entry: &[u8]) -> usize {
    8 + u64::from_le_bytes(entry[..8].try_into().expect("eight bytes")) as usize
}

/// A composition whose `string` a worker warmed at each of `hz` over the whole stream and
/// persisted, and each warm's new entries.
fn warmed_notes(name: &str, hz: &[&str]) -> (Graph, Memory, Vec<BTreeSet<String>>) {
    let mut nodes = vec![("string".to_string(), HELD.to_string())];
    nodes.extend(
        hz.iter()
            .map(|f| (format!("warm{f}"), format!("@string(t, f0={f})\n"))),
    );
    let nodes: Vec<(&str, &str)> = nodes
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let graph = graph_of(name, &nodes);
    let memory = Memory::default();
    let mut entries = Vec::new();
    for f in hz {
        let store = opened(&memory, u64::MAX);
        let config = RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(8_000),
            },
            ..RenderConfig::at(RATE)
        };
        let before: BTreeSet<String> = memory.entries().into_iter().collect();
        now(render_through(&graph, &format!("warm{f}"), config, &store)).expect("a render");
        now(store.persist()).expect("persisted");
        let after = memory.entries().into_iter();
        entries.push(after.filter(|e| !before.contains(e)).collect());
    }
    (graph, memory, entries)
}

fn taken(memory: &Memory) -> Vec<(String, usize)> {
    std::mem::take(&mut *memory.reads.lock().unwrap())
}

/// A stream looks each stored note up once: a note it has not met reads that note's entry
/// alone, and once met, adding or replacing it again, or any note met before, reads nothing.
#[test]
fn a_stream_reads_each_stored_notes_header_once() {
    let (graph, memory, entries) = warmed_notes("met", &["261.63", "392"]);
    let player = opened(&memory, u64::MAX);
    let stream = notes(&graph, 8_000, &player);
    let c = now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    next(&mut stream.borrow_mut()).expect("a block");
    taken(&memory);

    let g = "@string(t - 700sp, f0=392)";
    let g = now(added(&stream, &graph, &term(g), &player)).expect("added");
    let reads = taken(&memory);
    assert!(!reads.is_empty(), "the unmet note is looked up");
    for (name, _) in &reads {
        assert!(
            entries[1].contains(name),
            "{name} is no entry of the unmet note"
        );
    }
    next(&mut stream.borrow_mut()).expect("a block");
    taken(&memory);

    let fade =
        |at: &str, of: &str| format!("{of} * (1 - step(t - {at})*(1 - exp(-(t - {at})/0.15s)))");
    let again = [
        STRIKE,
        "@string(t - 900sp, f0=392)",
        "@string(t - 1000sp, f0=261.63)",
    ];
    for (k, note) in again.iter().enumerate() {
        now(added(&stream, &graph, &term(note), &player)).expect("added");
        let released = (c, &term(&fade(&format!("{}sp", 1100 + k), STRIKE)));
        assert!(now(replaced(&stream, &graph, released, &player)).expect("replaced"));
        let released = fade(&format!("{}sp", 1100 + k), "@string(t - 700sp, f0=392)");
        assert!(now(replaced(&stream, &graph, (g, &term(&released)), &player)).expect("replaced"));
        next(&mut stream.borrow_mut()).expect("a block");
    }
    assert_eq!(taken(&memory), [], "every note here was met");
    assert!(!store_hits(&stream).is_empty());
}

fn warm_into(graph: &Graph, store: &Store<Memory>) {
    let mut warmed = graph.clone();
    let warm = sva_ast::parse_expr("@string(t, f0=261.63)").expect("an expression");
    assert!(warmed.define("warm", warm));
    let config = RenderConfig::seconds(RATE, SECONDS);
    now(render_through(&warmed, "warm", config, store)).expect("a render");
}

/// A note a stream met as missing is asked for again, so one another holder persists while it
/// plays is found.
#[test]
fn a_stream_finds_a_note_another_holder_persisted_after_it_missed_it() {
    let (graph, memory, _) = warmed_notes("persisted-later", &[]);
    let player = opened(&memory, u64::MAX);
    let stream = notes(&graph, 8_000, &player);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert_eq!(store_hits(&stream), Vec::<String>::new());
    let other = opened(&memory, u64::MAX);
    warm_into(&graph, &other);
    now(other.persist()).expect("persisted");
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert_eq!(store_hits(&stream), ["string(f0=261.63, release=inf)"]);
}

/// A hit met while staged is asked for again once its store commits it, as its samples moved.
#[test]
fn a_stream_asks_again_for_a_hit_its_store_committed() {
    let (graph, memory, _) = warmed_notes("committed-later", &[]);
    let player = opened(&memory, u64::MAX);
    warm_into(&graph, &player);
    let stream = notes(&graph, 8_000, &player);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    taken(&memory);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert_eq!(taken(&memory), [], "a hit met once is not asked again");
    now(player.persist()).expect("persisted");
    taken(&memory);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert!(
        !taken(&memory).is_empty(),
        "the committed note is asked for"
    );
}

/// `Memory` whose every read answers only on its `delay`th poll, waking itself between.
#[derive(Clone)]
struct Slow {
    memory: Memory,
    delay: usize,
}

async fn later(polls: usize) {
    let mut left = polls;
    std::future::poll_fn(|cx| match left {
        0 => Poll::Ready(()),
        _ => {
            left -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

impl Backend for Slow {
    type Lock = Held;

    async fn lock(&self) -> Result<Held, String> {
        self.memory.lock().await
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        later(self.delay).await;
        self.memory.get(name).await
    }

    async fn get_range(&self, name: &str, from: u64, len: u64) -> Result<Option<Vec<u8>>, String> {
        later(self.delay).await;
        self.memory.get_range(name, from, len).await
    }

    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.memory.put(name, bytes).await
    }

    async fn delete(&self, name: &str) -> Result<bool, String> {
        self.memory.delete(name).await
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        self.memory.list().await
    }

    async fn staging(&self) -> Result<Slow, String> {
        let memory = self.memory.staging().await?;
        Ok(Slow { memory, ..*self })
    }

    async fn rename(&self, name: &str, to: &Slow) -> Result<bool, String> {
        self.memory.rename(name, &to.memory).await
    }
}

/// Polled until it is done: every wait here wakes itself.
fn settled<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    loop {
        let polled = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        if let Poll::Ready(out) = polled {
            return out;
        }
    }
}

type Pending<'s> = std::pin::Pin<Box<dyn Future<Output = Result<Changed, EngineError>> + 's>>;

/// Edit `k`: at an onset, a note, and the handle it replaces, if any.
type Spec = (i64, &'static str, Option<Handle>);

fn changed_by(graph: &Graph, (at, f0, replaced): Spec) -> Change {
    match replaced {
        Some(handle) => {
            let fade = term(&format!("0.5*@string(t - {at}sp, f0={f0})"));
            Change::Replace(handle, graph.clone(), fade, Placed::Written)
        }
        None => Change::Add(
            graph.clone(),
            term(&format!("@string(t - {at}sp, f0={f0})")),
            Placed::Written,
        ),
    }
}

fn notes_over(graph: &Graph, store: &impl sva_engine::Through, live: bool) -> RefCell<Stream> {
    let config = StreamConfig {
        block: 256,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(8_000),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let stream = settled(Stream::open(graph, &term("@notes"), config, None, store));
    let mut stream = stream.expect("a stream");
    if live {
        stream.go_live();
    }
    RefCell::new(stream)
}

/// A live stream edited once a block, adds of stored notes and replaces of sounding ones, over
/// a store whose every read takes several polls: every block comes when asked, each edit lands
/// while the stream plays on, and what it plays is, bit for bit, an exact stream taking each
/// edit where it landed.
#[test]
fn a_live_stream_plays_on_while_its_edits_await_a_slow_store() {
    let hz = ["261.63", "293.66", "329.63", "349.23", "392"];
    let (graph, memory, _) = warmed_notes("slow", &hz);
    let slow = Slow {
        memory: memory.clone(),
        delay: 3,
    };
    let player = settled(Store::open(slow, u64::MAX)).expect("the store opens");
    let shared = notes_over(&graph, &player, true);
    let (mut specs, mut landed, mut handles) = (Vec::<Spec>::new(), Vec::new(), Vec::new());
    let (mut pending, mut heard, mut waited) = (Vec::<(usize, Pending)>::new(), Vec::new(), 0);
    let mut added = BTreeMap::new();
    let mut k = 0;
    while k < 20 || !pending.is_empty() {
        if k < 20 {
            let at = shared.borrow().position() + 64;
            let replaced = handles.pop().filter(|_| k % 2 == 1);
            specs.push((at, hz[k % hz.len()], replaced));
            let (spec, graph) = (specs[k], &graph);
            let build = move |_: &Stream| Ok::<_, EngineError>(changed_by(graph, spec));
            pending.push((k, Box::pin(change(&shared, build, &player))));
        }
        let mut left = Vec::new();
        for (edit, mut future) in pending.drain(..) {
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(done) => {
                    if let Changed::Added(handle) = done.expect("an edit") {
                        handles.push(handle);
                        added.insert(handle, edit);
                    }
                    landed.push((heard.len(), edit));
                }
                Poll::Pending => left.push((edit, future)),
            }
        }
        waited += left.len();
        pending = left;
        let block = next(&mut shared.borrow_mut()).expect("a block");
        heard.push(block.expect("a block when asked").plane(0).to_vec());
        k += 1;
    }
    assert!(waited > 0, "some edit awaited the store across a block");

    let at_once = opened(&memory, u64::MAX);
    let reference = notes_over(&graph, &at_once, false);
    let (mut expected, mut ours) = (Vec::new(), BTreeMap::new());
    for n in 0..heard.len() {
        for (_, edit) in landed.iter().filter(|(at, _)| *at == n) {
            let (at, f0, replaced) = specs[*edit];
            let replaced = replaced.map(|live| ours[&added[&live]]);
            let build = |_: &Stream| Ok::<_, EngineError>(changed_by(&graph, (at, f0, replaced)));
            if let Changed::Added(handle) =
                now(change(&reference, build, &at_once)).expect("an edit")
            {
                ours.insert(*edit, handle);
            }
        }
        let block = next(&mut reference.borrow_mut()).expect("a block");
        let block = block.expect("a block");
        expected.push(block.plane(0).to_vec());
    }
    assert!(
        heard.iter().flatten().any(|v| *v != 0.0),
        "silence tests nothing"
    );
    assert!(
        heard == expected,
        "the slow store's live stream plays what the exact one does"
    );
    assert!(!store_hits(&shared).is_empty());
}

/// A target that only crops or moves a stored node is stored as that node's samples, not a
/// copy of them: its entry is a header alone, and a warm render reads it back bit for bit.
#[test]
fn a_crop_of_a_stored_node_stores_no_copy() {
    for (name, target) in [
        ("crop-range", "@string(t, f0=261.63)\n"),
        ("crop-call", "crop(@string(t, f0=261.63), 0.01s, 0.2s)\n"),
    ] {
        let graph = graph_of(name, &[("string", HELD), ("warm", target)]);
        let memory = Memory::default();
        let config = RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(2_000),
            },
            ..RenderConfig::at(RATE)
        };
        let store = opened(&memory, u64::MAX);
        let cold = now(render_through(&graph, "warm", config.clone(), &store)).expect("a render");
        now(store.persist()).expect("persisted");
        let root = stats(&cold).lookups.iter().find(|l| l.node == "warm");
        let root = root.expect("the root was looked up").key;
        let entry = memory
            .bytes(&format!("{:016x}{:016x}", root.0, root.1))
            .expect("the root is stored");
        assert_eq!(
            entry.len(),
            head_of(&entry),
            "{name}: the root holds no sample"
        );
        assert_eq!(memory.entries().len(), 2, "{name}: the note and the root");

        let store = opened(&memory, u64::MAX);
        let warm = now(render_through(&graph, "warm", config, &store)).expect("a render");
        assert_eq!(stats(&warm).computed(), 0, "{name}: {:?}", stats(&warm));
        assert_eq!(bits(&warm), bits(&cold), "{name}");
    }
}

fn entry_of(memory: &Memory, render: &Render, node: &str) -> Vec<u8> {
    let key = stats(render).lookups.iter().find(|l| l.node == node);
    let key = key.unwrap_or_else(|| panic!("`{node}` was looked up")).key;
    let name = format!("{:016x}{:016x}", key.0, key.1);
    memory
        .bytes(&name)
        .unwrap_or_else(|| panic!("`{node}` is stored"))
}

/// A crop of a moved note, and a crop of a target the store answers as another's samples,
/// each refer to the note's samples themselves; a referral whose samples are gone is a miss,
/// computed again bit for bit.
#[test]
fn a_referral_names_the_samples_themselves_and_misses_once_they_are_gone() {
    let graph = graph_of(
        "referrals",
        &[
            ("string", HELD),
            ("moved", "@string(t - 80sp, f0=261.63)\n"),
            ("warm", "@string(t, f0=261.63)\n"),
            ("chain", "crop(@moved, 0.02s, 0.2s)\n"),
            ("outer", "crop(@warm, 0s, 0.2s)\n"),
        ],
    );
    let memory = Memory::default();
    let config = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(2_000),
        },
        ..RenderConfig::at(RATE)
    };
    let render = |target: &str| {
        let store = opened(&memory, u64::MAX);
        let done = now(render_through(&graph, target, config.clone(), &store)).expect("a render");
        now(store.persist()).expect("persisted");
        done
    };
    render("warm");
    for target in ["chain", "outer"] {
        let cold = render(target);
        let entry = entry_of(&memory, &cold, target);
        assert_eq!(entry.len(), head_of(&entry), "{target}: a header alone");
        let warm = render(target);
        assert_eq!(stats(&warm).computed(), 0, "{target}: {:?}", stats(&warm));
        assert_eq!(bits(&warm), bits(&cold), "{target}");
    }

    let before = render("outer");
    for name in memory.entries() {
        let entry = memory.bytes(&name).expect("an entry");
        if entry.len() > head_of(&entry) {
            memory.held.lock().unwrap().remove(&name);
        }
    }
    let after = render("outer");
    assert!(stats(&after).computed() > 0, "{:?}", stats(&after));
    assert_eq!(bits(&after), bits(&before));
}

fn warmed_into(graph: &Graph, store: &Store<Memory>) -> CacheStats {
    now(warm(
        graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        store,
    ))
    .expect("warmed")
}

/// A warmed target, once persisted, renders from the store alone, bit for bit the fresh render.
#[test]
fn a_warmed_and_persisted_target_renders_as_a_store_hit() {
    let memory = Memory::default();
    let graph = nested("warmed", 220);
    let store = opened(&memory, u64::MAX);
    let warmed = warmed_into(&graph, &store);
    assert!(warmed.computed() > 0, "{warmed:?}");
    assert!(memory.entries().is_empty(), "warm persists nothing");
    now(store.persist()).expect("persisted");

    let hit = rendered(&graph, &opened(&memory, u64::MAX));
    assert_eq!(stats(&hit).lookups.len(), 1, "{:?}", stats(&hit));
    assert_eq!(stats(&hit).lookups[0].outcome, Outcome::Hit);
    let fresh = render(&graph, "master", RenderConfig::seconds(RATE, SECONDS), None);
    assert_eq!(bits(&hit), bits(&fresh.expect("a render")));
}

/// Warming what the store holds looks the root up, header alone, and computes and stages nothing.
#[test]
fn warming_a_stored_target_computes_nothing() {
    let memory = Memory::default();
    let graph = nested("rewarmed", 220);
    let store = opened(&memory, u64::MAX);
    warmed_into(&graph, &store);
    now(store.persist()).expect("persisted");
    let store = opened(&memory, u64::MAX);
    memory.reads.lock().unwrap().clear();

    let again = warmed_into(&graph, &store);
    assert_eq!(again.computed(), 0, "{again:?}");
    assert_eq!(again.lookups.len(), 1, "{again:?}");
    assert_eq!(again.lookups[0].outcome, Outcome::Hit);
    for (name, bytes) in memory.reads.lock().unwrap().iter() {
        let span = memory.bytes(name).map_or(0, |entry| head_of(&entry));
        assert!(
            *bytes <= span,
            "{name}: {bytes} bytes read, past its header"
        );
    }
    assert!(memory.staged().is_empty(), "nothing staged");
    assert_eq!(now(store.persist()).expect("persisted").written, 0);
}

/// Two workers over one directory, each warming its own note and persisting it: both notes
/// are stored, and a store opened afterwards answers each from its root.
#[test]
fn stores_sharing_a_directory_each_persist_what_they_warmed() {
    let memory = Memory::default();
    let (low, high) = (nested("note-low", 220), nested("note-high", 440));
    let (first, second) = (opened(&memory, u64::MAX), opened(&memory, u64::MAX));
    warmed_into(&low, &first);
    warmed_into(&high, &second);
    now(first.persist()).expect("persisted");
    now(second.persist()).expect("persisted");

    let reader = opened(&memory, u64::MAX);
    for graph in [&low, &high] {
        let hit = rendered(graph, &reader);
        assert_eq!(stats(&hit).lookups.len(), 1, "{:?}", stats(&hit));
        assert_eq!(stats(&hit).lookups[0].outcome, Outcome::Hit);
    }
}

/// A live stream adding a term that reads a stored value at a moving index plays the stored
/// samples, bit for bit as a storeless exact stream, and names nothing dropped: nothing it
/// started silent is ever read.
#[test]
fn a_moving_index_read_of_a_stored_value_drops_nothing() {
    let looped = "crop(lowpass(sample(saw(110*t)), cutoff=900, q=0.7), 0s, 0.1s)\n";
    let graph = graph_of(
        "moving-stored",
        &[("looped", looped), ("warm", "@looped(t)\n")],
    );
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    now(render_through(
        &graph,
        "warm",
        RenderConfig::seconds(RATE, 0.1),
        &store,
    ))
    .expect("warmed");
    now(store.persist()).expect("persisted");
    let loop_term = term("crop(@looped[idx((t - 2048sp) % 800sp)], 2048sp, inf)");
    let heard = |live: bool, player: Option<&Store<Memory>>| {
        let stream = match player {
            Some(player) => notes(&graph, 6_000, player),
            None => notes(&graph, 6_000, &NoStore),
        };
        if live {
            stream.borrow_mut().go_live();
        }
        let mut heard = Vec::new();
        for _ in 0..4 {
            let block = next(&mut stream.borrow_mut())
                .expect("a block")
                .expect("a block");
            heard.extend(block.plane(0).iter().map(|v| v.to_bits()));
        }
        match player {
            Some(player) => now(added(&stream, &graph, &loop_term, player)),
            None => now(added(&stream, &graph, &loop_term, &NoStore)),
        }
        .expect("added");
        heard.extend(played(&stream));
        let dropped: Vec<String> = stream
            .borrow()
            .dropped()
            .iter()
            .map(|n| n.to_string())
            .collect();
        (heard, dropped, store_hits(&stream))
    };
    let (cold, _, _) = heard(false, None);
    let player = opened(&memory, u64::MAX);
    let (warm, dropped, hits) = heard(true, Some(&player));
    assert!(cold.iter().any(|b| *b != 0), "silence tests nothing");
    assert!(warm == cold, "the stored samples, bit for bit");
    assert!(dropped.is_empty(), "{dropped:?}");
    assert_eq!(hits, ["looped"], "the store answers it");
}
