// Concern: proves memory over a disk answers renders and streams from the root down, writes until persist, keeps a version and budget | Non-concern: a real medium | IO: (composition, fake disk) -> hits

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Waker};

use crate::disk::{Held, Memory, disk, now, opened};
use crate::fixtures::{added, graph_of, next, replaced, samples};
use sva_ast::Graph;
use sva_engine::{
    Ask, Backend, CacheStats, Change, Changed, EngineError, Handle, Hash, INDEX_NAME, Out, Outcome,
    Persisted, Placed, Range, Render, RenderConfig, Representation, STORE_FORMAT, Store, Stream,
    StreamConfig, Tier, change, render, render_over,
};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// A render's node lookups alone, each of which memory may pass to the disk.
fn nodes(stats: &CacheStats) -> Vec<&sva_engine::Lookup> {
    stats.lookups.iter().filter(|l| l.store.is_some()).collect()
}

fn rendered(graph: &Graph, store: &Tier<Memory>) -> Render {
    now(render_over(
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
    assert_eq!(disk(&reopened).bytes(), 0);
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
    assert!(disk(&reopened).bytes() > 0, "the index names every entry");
}

/// The format a digest of these values' stored bytes was pinned under, and the digest.
const PINNED: (u32, u64) = (17, 9463521731038124131);

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

    let root = nodes(stats(&cold))
        .into_iter()
        .find(|l| l.node == "master")
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
    let before = disk(&store).bytes();
    let render = rendered(&tone(&format!("tone-{hz}"), hz), &store);
    now(store.persist()).expect("persisted");
    assert!(
        disk(&store).bytes() <= max_bytes,
        "the store keeps its budget"
    );
    let keys = nodes(stats(&render)).iter().map(|l| l.key).collect();
    (keys, disk(&store).bytes().saturating_sub(before))
}

fn all_held(memory: &Memory, keys: &[Hash]) -> bool {
    let store = opened(memory, u64::MAX);
    keys.iter().all(|k| disk(&store).holds(*k))
}

/// A store's reads reach the index with its next commit, ahead of what that commit writes.
#[test]
fn past_its_budget_the_store_evicts_the_least_recently_used_first() {
    let (_, one) = persisted_tone(&Memory::default(), u64::MAX, 100);
    let budget = one * 5 / 2;
    let memory = Memory::default();
    let (a, _) = persisted_tone(&memory, budget, 100);
    let (b, _) = persisted_tone(&memory, budget, 200);
    let store = opened(&memory, budget);
    rendered(&tone("tone-100", 100), &store);
    let render = rendered(&tone("tone-300", 300), &store);
    now(store.persist()).expect("persisted");
    let c: Vec<Hash> = nodes(stats(&render)).iter().map(|l| l.key).collect();
    assert!(all_held(&memory, &a), "read since, so kept");
    assert!(!all_held(&memory, &b), "least recently used, so gone");
    assert!(all_held(&memory, &c), "just written, so kept");
}

/// A node rendered short and persisted, then rendered longer and persisted by the same memory,
/// sends the disk only what it lacks, and the entry holds both: a new process renders the
/// longer range off the disk alone.
#[test]
fn a_node_persisted_twice_holds_what_both_persists_sent() {
    let memory = Memory::default();
    let graph = tone("tone-longer", 100);
    let range = |secs: f64| RenderConfig::seconds(RATE, secs);
    let store = opened(&memory, u64::MAX);
    for secs in [SECONDS, 2.0 * SECONDS] {
        now(render_over(&graph, "master", range(secs), &store)).expect("a render");
        now(store.persist()).expect("persisted");
    }
    let cold = now(render_over(
        &graph,
        "master",
        range(2.0 * SECONDS),
        &Tier::default(),
    ));
    let next = opened(&memory, u64::MAX);
    let warm = now(render_over(&graph, "master", range(2.0 * SECONDS), &next));
    let (cold, warm) = (cold.expect("a render"), warm.expect("a render"));
    assert_eq!(stats(&warm).computed(), 0, "{:?}", stats(&warm));
    assert_eq!(samples(&cold), samples(&warm));
}

/// A value larger than the store's whole budget is never stored, and the persist counts it.
#[test]
fn a_value_past_the_whole_budget_is_refused_and_counted() {
    let (_, one) = persisted_tone(&Memory::default(), u64::MAX, 100);
    let memory = Memory::default();
    let store = opened(&memory, one / 2);
    rendered(&tone("tone-large", 100), &store);
    let done = now(store.persist()).expect("persisted");
    assert_eq!(done.written, 0, "{done:?}");
    assert!(done.refused > 0, "{done:?}");
    assert!(memory.entries().is_empty());
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

/// A store that only read from the disk, or that already committed everything, persists
/// without a single call of the disk: no lock, no index.
#[test]
fn a_persist_with_nothing_to_commit_calls_nothing_of_the_disk() {
    let memory = Memory::default();
    let graph = two_voices("idle", 330);
    let store = opened(&memory, u64::MAX);
    rendered(&graph, &store);
    now(store.persist()).expect("persisted");
    let reader = opened(&memory, u64::MAX);
    let warm = rendered(&graph, &reader);
    assert_eq!(stats(&warm).computed(), 0, "{:?}", stats(&warm));
    for store in [&reader, &store] {
        memory.calls.store(0, Ordering::Relaxed);
        let done = now(store.persist()).expect("persisted");
        assert_eq!(done, Persisted::default());
        assert_eq!(memory.calls.load(Ordering::Relaxed), 0);
    }
}

/// A store of `n` entries at this format, each `BYTES` long, as another build left it.
fn holding(n: usize) -> Memory {
    const BYTES: usize = 64;
    let memory = Memory::default();
    let mut index = String::from_utf8(format_only()).expect("text");
    for i in 0..n {
        let name = format!("{:032x}", 0xfeed_0000 + i);
        memory.set(&name, vec![0; BYTES]);
        index += &format!("{name} {BYTES}\n");
    }
    memory.set(INDEX_NAME, index.into_bytes());
    memory
}

/// Each entry the index file names, in its order.
fn indexed(memory: &Memory) -> Vec<String> {
    let text = String::from_utf8(memory.bytes(INDEX_NAME).expect("an index")).expect("text");
    let lines = text.lines().skip(1);
    lines
        .map(|line| {
            line.split_once(' ')
                .expect("a name and a size")
                .0
                .to_string()
        })
        .collect()
}

/// The calls and listings of the disk one persist of a fresh render over `n` entries makes.
fn persisted_over(n: usize) -> (usize, usize) {
    let memory = holding(n);
    let store = opened(&memory, u64::MAX);
    rendered(&two_voices("over-many", 330), &store);
    memory.calls.store(0, Ordering::Relaxed);
    memory.lists.store(0, Ordering::Relaxed);
    let done = now(store.persist()).expect("persisted");
    assert_eq!(done.written, 3, "x, y and master");
    assert_eq!(indexed(&memory).len(), n + 3, "the index names every entry");
    let calls = memory.calls.load(Ordering::Relaxed);
    (calls, memory.lists.load(Ordering::Relaxed))
}

#[test]
fn a_persists_calls_scale_with_what_it_commits_not_with_what_the_store_holds() {
    let (few, many) = (persisted_over(10), persisted_over(300));
    assert_eq!(few, many, "10 entries held against 300");
    assert_eq!(few.1, 0, "a persist lists nothing");
}

/// Two stores over one directory, persisting by turns what overlaps and what does not, each
/// over an index the other wrote since it last read it.
#[test]
fn stores_persisting_by_turns_over_one_directory_index_every_entry_once() {
    let memory = Memory::default();
    let (first, second) = (opened(&memory, u64::MAX), opened(&memory, u64::MAX));
    for (turn, hz) in [330, 440, 550, 660].into_iter().enumerate() {
        let store = [&first, &second][turn % 2];
        rendered(&two_voices(&format!("turn-{turn}"), hz), store);
        assert!(now(store.persist()).expect("persisted").written > 0);
    }
    let mut named = indexed(&memory);
    named.sort();
    let mut once = named.clone();
    once.dedup();
    assert_eq!(named, once, "no entry is named twice");
    assert_eq!(named, memory.entries(), "the index names each entry held");
}

fn rendered_over(graph: &Graph, store: &Tier<Memory>, seconds: f64) -> Render {
    now(render_over(
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
    let walked = nodes(stats);
    let visited: Vec<String> = walked.iter().map(|l| l.node.clone()).collect();
    assert_eq!(
        sorted(&visited),
        ["e", "master", "p", "q", "s"],
        "{stats:?}"
    );
    assert_eq!(visited.len(), 5, "each node is visited once");
    for lookup in walked {
        let hit = ["q", "s"].contains(&lookup.node.as_str());
        assert_eq!(lookup.outcome == Outcome::Hit, hit, "{lookup:?}");
    }
    assert_eq!(sorted(&stats.typed), ["e", "master", "p"]);
    assert_eq!(sorted(&stats.planned), ["e", "master", "p"]);
    let fresh = render(
        &edited,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        &Tier::default(),
    );
    assert_eq!(
        bits(&warm),
        bits(&fresh.expect("a render")),
        "hits read as computed"
    );
}

/// A reading of a node under one the store answers reaches it: the store's hit above it would
/// hide it, so each node above an asked one is computed, and the asked node read as its samples.
#[test]
fn a_reading_of_a_node_under_a_stored_one_reads_it() {
    let memory = Memory::default();
    let graph = nested("asked-under", 220);
    let store = opened(&memory, u64::MAX);
    rendered(&graph, &store);
    now(store.persist()).expect("persisted");

    let asks = vec![Ask {
        node: "e".to_string(),
        representation: Representation::Samples,
    }];
    let config = RenderConfig::seconds(RATE, SECONDS).asking(asks);
    let fresh = render(&graph, "master", config.clone(), &Tier::default()).expect("a render");
    let store = opened(&memory, u64::MAX);
    let warm = now(render_over(&graph, "master", config, &store)).expect("a render");
    let e = |render: &Render| {
        let id = render.id("e").expect("`e` was typed");
        render.output(id).expect("`e` was read").plane(0).to_vec()
    };
    assert_eq!(e(&warm), e(&fresh));
    assert!(outcomes(stats(&warm), "e").contains(&Outcome::Hit));
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
    let fresh = render(&graph, "master", config.clone(), &Tier::default()).expect("a render");
    let store = opened(&Memory::default(), u64::MAX);
    let cold = now(render_over(&graph, "master", config, &store)).expect("a render");
    assert!(
        bits(&fresh)[0].iter().any(|b| *b != 0),
        "silence tests nothing"
    );
    assert_eq!(bits(&cold), bits(&fresh));
}

/// What a render offers memory goes to the disk only as memory lets it go, and is committed
/// only by persist: under a cap below what the render offers, memory writes back what it
/// evicts, and the store is unchanged until persist moves everything into it.
#[test]
fn memory_writes_back_what_it_evicts_and_only_persist_commits_it() {
    let memory = Memory::default();
    let graph = two_voices("staging", 330);
    let store = opened(&memory, u64::MAX);
    rendered_over(&graph, &store, 2.0);
    assert!(
        memory.staged().is_empty(),
        "memory holds all it was offered"
    );
    let held = store.bytes();

    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    store.set_max_bytes(held / 3);
    rendered_over(&graph, &store, 2.0);
    assert!(memory.entries().is_empty(), "the store is unchanged");
    assert!(
        !memory.staged().is_empty(),
        "memory wrote back what it evicted"
    );
    assert!(store.counters().writebacks > 0);
    assert!(store.bytes() <= held / 3, "memory keeps its cap");
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

/// A medium that refuses every write fails no render, its out dropped or not: the samples are a
/// render's over memory alone, and each one's stats say why nothing was written; once the medium
/// takes writes, a persist writes what memory still holds.
#[test]
fn a_store_refusing_every_write_fails_no_render() {
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    store.set_max_bytes(1);
    memory.refusing.store(true, Ordering::Relaxed);
    let graph = two_voices("refusing", 330);
    let through = rendered(&graph, &store);
    let config = RenderConfig::seconds(RATE, SECONDS);
    let fresh = render(&graph, "master", config.clone(), &Tier::default()).expect("a render");
    assert_eq!(samples(&through), samples(&fresh));
    assert!(stats(&through).unstaged.is_some(), "{:?}", stats(&through));
    let prepared = dropped(&graph, config, &store);
    assert!(
        stats(&prepared).unstaged.is_some(),
        "{:?}",
        stats(&prepared)
    );
    assert!(now(store.persist()).is_err());
    memory.refusing.store(false, Ordering::Relaxed);
    assert!(now(store.persist()).expect("persisted").written > 0);
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
    now(render_over(
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
        &Tier::default(),
    ));
    let cold = RefCell::new(cold.expect("a stream"));
    let warm = now(Stream::open(&graph, &master, config, &player));
    let warm = RefCell::new(warm.expect("a stream"));
    now(added(&cold, &graph, &term, &Tier::default())).expect("added");
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
    now(render_over(&graph, "warm", config, &store)).expect("a render");
    now(store.persist()).expect("persisted");
    (graph, memory)
}

const STRIKE: &str = "@string(t - 512sp, f0=261.63)";

fn notes<B: Backend>(graph: &Graph, end: i64, store: &Tier<B>) -> RefCell<Stream> {
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
    RefCell::new(now(Stream::open(graph, &master, config, store)).expect("a stream"))
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
            None => notes(&graph, 2_000, &Tier::default()),
        };
        let handle = match store {
            Some(store) => now(added(&stream, &graph, &term(STRIKE), store)),
            None => now(added(&stream, &graph, &term(STRIKE), &Tier::default())),
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
            None => now(replaced(&stream, &graph, keyed, &Tier::default())),
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
    let cold = notes(&graph, 8_000, &Tier::default());
    now(added(&cold, &graph, &term(STRIKE), &Tier::default())).expect("added");
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
fn late<B: Backend>(graph: &Graph, live: bool, store: &Tier<B>) -> (Vec<u64>, Vec<String>) {
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
    let (cold, _) = late(&graph, false, &Tier::default());
    let (unready, _) = late(&graph, true, &Tier::default());
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
        now(render_over(&graph, &format!("warm{f}"), config, &store)).expect("a render");
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

fn warm_into(graph: &Graph, store: &Tier<Memory>) {
    let mut warmed = graph.clone();
    let warm = sva_ast::parse_expr("@string(t, f0=261.63)").expect("an expression");
    assert!(warmed.define("warm", warm));
    let config = RenderConfig::seconds(RATE, SECONDS);
    now(render_over(&warmed, "warm", config, store)).expect("a render");
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

/// A note a render left in memory is the stream's from memory, before its persist and after:
/// committing it moves nothing memory holds.
#[test]
fn a_stream_reads_a_note_memory_holds_across_its_commit() {
    let (graph, memory, _) = warmed_notes("committed-later", &[]);
    let player = opened(&memory, u64::MAX);
    warm_into(&graph, &player);
    let stream = notes(&graph, 8_000, &player);
    taken(&memory);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert_eq!(taken(&memory), [], "memory answers the note it was offered");
    now(player.persist()).expect("persisted");
    taken(&memory);
    now(added(&stream, &graph, &term(STRIKE), &player)).expect("added");
    assert_eq!(taken(&memory), [], "and still does once it is committed");
    assert_eq!(store_hits(&stream), ["string(f0=261.63, release=inf)"; 3]);
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

fn notes_over<B: Backend>(graph: &Graph, store: &Tier<B>, live: bool) -> RefCell<Stream> {
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
    let stream = settled(Stream::open(graph, &term("@notes"), config, store));
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
    let player = Tier::over(player, u64::MAX);
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
        let cold = now(render_over(&graph, "warm", config.clone(), &store)).expect("a render");
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
        let warm = now(render_over(&graph, "warm", config, &store)).expect("a render");
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
        let done = now(render_over(&graph, target, config.clone(), &store)).expect("a render");
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

/// `graph`'s master rendered with its out dropped.
fn dropped(graph: &Graph, config: RenderConfig, store: &Tier<Memory>) -> Render {
    let config = RenderConfig {
        out: Out::Dropped,
        ..config
    };
    now(render_over(graph, "master", config, store)).expect("a render")
}

fn prepared(graph: &Graph, store: &Tier<Memory>) -> Render {
    dropped(graph, RenderConfig::seconds(RATE, SECONDS), store)
}

/// A target rendered with its out dropped hands back no samples; persisted, it renders from the
/// store alone, bit for bit the fresh render.
#[test]
fn a_target_prepared_and_persisted_renders_as_a_store_hit() {
    let memory = Memory::default();
    let graph = nested("prepared", 220);
    let store = opened(&memory, u64::MAX);
    let prepared = prepared(&graph, &store);
    assert!(stats(&prepared).computed() > 0, "{:?}", stats(&prepared));
    assert!(prepared.output(prepared.root).is_err(), "no samples");
    assert!(prepared.buffers.is_empty());
    assert!(memory.entries().is_empty(), "a render persists nothing");
    now(store.persist()).expect("persisted");

    let hit = rendered(&graph, &opened(&memory, u64::MAX));
    assert_eq!(stats(&hit).lookups.len(), 1, "{:?}", stats(&hit));
    assert_eq!(stats(&hit).lookups[0].outcome, Outcome::Hit);
    let fresh = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        &Tier::default(),
    );
    assert_eq!(bits(&hit), bits(&fresh.expect("a render")));
}

/// A render with its out dropped holds as much of its root however long it runs, where one
/// handing back its samples holds them all.
#[test]
fn a_render_with_its_out_dropped_holds_its_root_a_block_at_a_time() {
    let master = "lowpass(sample(0.1*saw(110*t)), cutoff=900, q=0.7)\n";
    let graph = graph_of("out-dropped", &[("master", master)]);
    let held = |secs: f64, out: Out| {
        let config = RenderConfig {
            out,
            ..RenderConfig::seconds(RATE, secs)
        };
        let done = render(&graph, "master", config, &Tier::default()).expect("a render");
        done.held_bytes
    };
    assert_eq!(held(4.0, Out::Dropped), held(8.0, Out::Dropped));
    assert!(held(8.0, Out::Kept) > held(4.0, Out::Kept));
}

/// A second preparation over what a first one stored looks the root up, header alone, and
/// computes, prices and stages nothing.
#[test]
fn preparing_a_stored_target_computes_nothing() {
    let memory = Memory::default();
    let graph = nested("reprepared", 220);
    let store = opened(&memory, u64::MAX);
    prepared(&graph, &store);
    now(store.persist()).expect("persisted");
    let store = opened(&memory, u64::MAX);
    memory.reads.lock().unwrap().clear();

    let again = prepared(&graph, &store);
    assert_eq!(again.work().priced_flops, 0);
    let again = stats(&again);
    assert_eq!(again.computed(), 0, "{again:?}");
    assert_eq!(again.lookups.len(), 1, "{again:?}");
    assert_eq!(again.lookups[0].outcome, Outcome::Hit);
    assert!(
        !memory.reads.lock().unwrap().is_empty(),
        "the disk answered"
    );
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

/// Two workers over one directory, each preparing its own note and persisting it: both notes
/// are stored, and a store opened afterwards answers each from its root.
#[test]
fn stores_sharing_a_directory_each_persist_what_they_prepared() {
    let memory = Memory::default();
    let (low, high) = (nested("note-low", 220), nested("note-high", 440));
    let (first, second) = (opened(&memory, u64::MAX), opened(&memory, u64::MAX));
    prepared(&low, &first);
    prepared(&high, &second);
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
    now(render_over(
        &graph,
        "warm",
        RenderConfig::seconds(RATE, 0.1),
        &store,
    ))
    .expect("warmed");
    now(store.persist()).expect("persisted");
    let loop_term = term("crop(@looped[idx((t - 2048sp) % 800sp)], 2048sp, inf)");
    let heard = |live: bool, player: Option<&Tier<Memory>>| {
        let stream = match player {
            Some(player) => notes(&graph, 6_000, player),
            None => notes(&graph, 6_000, &Tier::default()),
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
            None => now(added(&stream, &graph, &loop_term, &Tier::default())),
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

/// A fade the profile's prune level cuts where its bound falls under it: the level decides its
/// samples, so a node stored at one level never answers a render at another.
#[test]
fn a_node_stored_at_one_prune_level_never_answers_another() {
    let graph = graph_of(
        "prune-keyed",
        &[("master", "sample(sin(2*pi*200*t)*exp(-t/0.02))\n")],
    );
    let at = |prune_db: f64| RenderConfig {
        profile: sva_engine::Profile {
            prune_db,
            ..sva_engine::PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 1.0)
    };
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    now(render_over(&graph, "master", at(-120.0), &store)).expect("a render");
    now(store.persist()).expect("persisted");

    let warm = now(render_over(
        &graph,
        "master",
        at(-60.0),
        &opened(&memory, u64::MAX),
    ))
    .expect("a render");
    assert!(stats(&warm).computed() > 0, "{:?}", stats(&warm));
    let fresh = render(&graph, "master", at(-60.0), &Tier::default()).expect("a render");
    assert_eq!(bits(&warm), bits(&fresh));
}

/// A ramp-in and a bell of partials under envelopes, as a page's pad keys write one.
fn bell(name: &str) -> Graph {
    let ramp = "tc = 0.006\nmin(t, tc)/tc - sin(2*pi*min(t, tc)/tc)/(2*pi)\n";
    let bell = "f0 = 587.33\nvel = 0.7\nrelease = 1\ncrop(0.54*vel*(sin(2*pi*f0*t + \
        1.2*vel*exp(-t/0.3)*sin(2*pi*3.5*f0*t))*(0.33 + 0.67*exp(-t/1.5)) + \
        0.25*sin(4*pi*f0*t)*exp(-t/0.6))*@ramp(t, tc=0.0015)*(crop(1, 0s, release) + \
        crop(exp(-(t - release)/0.3), release, release + 2s)), 0s, release + 2s)\n";
    let master = "sample(@bell(t, f0=440, vel=0.5, release=inf))\n";
    graph_of(name, &[("ramp", ramp), ("bell", bell), ("master", master)])
}

fn measuring(readings: &[Representation]) -> RenderConfig {
    let asks = readings.iter().map(|representation| Ask {
        node: "master".to_string(),
        representation: *representation,
    });
    RenderConfig::seconds(RATE, 0.5).asking(asks.collect())
}

fn read_off(render: &Render, readings: &[Representation]) -> Vec<sva_engine::Answer> {
    let read = |r: &Representation| sva_engine::answer(render, render.root, *r).expect("a reading");
    readings.iter().map(read).collect()
}

/// A page measures each key it warms off `sample(...)` of it: those readings take the samples
/// alone, so a second warm over the persisted store reads them off the disk and computes
/// nothing, and measures what the first did.
#[test]
fn a_sampled_target_measured_and_persisted_is_measured_again_off_the_disk() {
    let readings = [
        Representation::Envelope { frame_secs: None },
        Representation::from_name("pitch").expect("a reading"),
    ];
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let cold = dropped(&bell("measured"), measuring(&readings), &store);
    assert!(cold.work().priced_flops > 0);
    now(store.persist()).expect("persisted");
    memory.reads.lock().unwrap().clear();

    let store = opened(&memory, u64::MAX);
    let warm = dropped(&bell("measured"), measuring(&readings), &store);
    assert_eq!(warm.work().priced_flops, 0, "{:?}", stats(&warm));
    assert!(store.counters().disk_reads >= 1);
    assert_eq!(read_off(&warm, &readings), read_off(&cold, &readings));
}

/// A chord's envelope is its law, never one measured off samples a store holds of it; its two
/// partials share no period, so the store holds it.
#[test]
fn a_stored_closed_form_asked_for_its_law_is_never_answered_by_its_samples() {
    let chord = "sin(2*pi*256*t) + sin(2*pi*362.03867196751236*t)\n";
    let graph = graph_of("law-kept", &[("master", chord)]);
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    now(render_over(&graph, "master", measuring(&[]), &store)).expect("a render");
    now(store.persist()).expect("persisted");

    let envelope = [Representation::Envelope { frame_secs: None }];
    let config = measuring(&envelope);
    let warm = dropped(&graph, config.clone(), &opened(&memory, u64::MAX));
    let fresh = render(&graph, "master", config, &Tier::default()).expect("a render");
    assert_eq!(read_off(&warm, &envelope), read_off(&fresh, &envelope));
}

/// The fade under `master` falls under the prune level and is cut: a render answered by the
/// stored `master` states that cut, as the render that computed it did.
#[test]
fn a_hit_states_the_cuts_that_shaped_its_samples() {
    let graph = graph_of(
        "cut-carried",
        &[
            ("fade", "sin(2*pi*200*t)*exp(-t/0.02)\n"),
            ("master", "sample(@fade)\n"),
        ],
    );
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let config = RenderConfig::seconds(RATE, 1.0);
    let cold = now(render_over(&graph, "master", config.clone(), &store)).expect("a render");
    now(store.persist()).expect("persisted");
    let cut = cold.labels[&cold.root]
        .pruned
        .clone()
        .expect("a prune level");
    assert!(!cut.cuts.is_empty(), "{cut:?}");

    let warm = now(render_over(
        &graph,
        "master",
        config,
        &opened(&memory, u64::MAX),
    ));
    let warm = warm.expect("a render");
    assert_eq!(stats(&warm).computed(), 0, "{:?}", stats(&warm));
    assert_eq!(warm.labels[&warm.root], cold.labels[&cold.root]);
}

fn reads(memory: &Memory) -> usize {
    memory.reads.lock().unwrap().len()
}

/// A closed form repeating within the range: its samples are a period laid out, and stored as a
/// node of its own like any other the render computed.
#[test]
fn a_periodic_closed_form_target_prepared_and_persisted_is_read_off_the_disk() {
    let graph = graph_of("periodic", &[("master", "sin(2*pi*220*t)*0.5\n")]);
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let cold = prepared(&graph, &store);
    assert!(cold.work().priced_flops > 0);
    now(store.persist()).expect("persisted");
    memory.reads.lock().unwrap().clear();

    let again = prepared(&graph, &opened(&memory, u64::MAX));
    assert_eq!(again.work().priced_flops, 0, "{:?}", stats(&again));
    assert!(reads(&memory) >= 1);
    let warm = rendered(&graph, &opened(&memory, u64::MAX));
    let fresh = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        &Tier::default(),
    );
    assert_eq!(bits(&warm), bits(&fresh.expect("a render")));
}

/// Memory writes no node computing costs under a flop per `BYTES_PER_FLOP` bytes it holds: a
/// sine's period laid out over two seconds is computed again sooner than read back.
#[test]
fn a_node_cheaper_to_compute_than_to_read_is_never_written() {
    let graph = graph_of("cheap", &[("master", "sin(2*pi*200*t)\n")]);
    let memory = Memory::default();
    let store = opened(&memory, u64::MAX);
    let cold = dropped(&graph, RenderConfig::seconds(RATE, 2.0), &store);
    let held = u128::from(RATE) * 2 * size_of::<f64>() as u128;
    assert!(cold.work().priced_flops * sva_engine::BYTES_PER_FLOP < held);
    assert_eq!(now(store.persist()).expect("persisted").written, 0);
    assert!(memory.entries().is_empty());
}
