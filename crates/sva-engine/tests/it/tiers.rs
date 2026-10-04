// Concern: proves memory reads each value off the disk once and keeps it, by counters | Non-concern: the disk's version, budget and commits (persist.rs) | IO: (composition, fake disk) -> counters, reads

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use crate::disk::{Memory, now, opened};
use crate::fixtures::{added, graph_of, next};
use sva_ast::Graph;
use sva_engine::{
    Backend, Counters, FETCH_READS, Range, Render, RenderConfig, Stream, StreamConfig, Tier, fetch,
    render_over,
};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

/// A note that ends a tenth of a second in, and a pad nothing here plays until a term reads it.
fn blip(name: &str, pad: u32) -> Graph {
    graph_of(
        name,
        &[
            (
                "blip",
                "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.1s)\n",
            ),
            ("pad", &format!("sample(sin(2*pi*{pad}*t))*0.1\n")),
            ("warm", "@blip(t, f0=200)\n"),
        ],
    )
}

/// `graph`'s `target` over `range` through a fresh tier over `memory`, persisted; the names of
/// the entries it wrote.
fn persisted(memory: &Memory, graph: &Graph, target: &str, end: i64) -> BTreeSet<String> {
    let before: BTreeSet<String> = memory.entries().into_iter().collect();
    let tier = opened(memory, u64::MAX);
    now(render_over(graph, target, over(end), &tier)).expect("a render");
    now(tier.persist()).expect("persisted");
    let after = memory.entries().into_iter();
    after.filter(|name| !before.contains(name)).collect()
}

fn over(end: i64) -> RenderConfig {
    RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(end),
        },
        ..RenderConfig::at(RATE)
    }
}

fn notes<B: Backend>(graph: &Graph, end: i64, tier: &Tier<B>) -> RefCell<Stream> {
    let config = StreamConfig {
        block: BLOCK,
        channels: None,
        render: over(end),
    };
    let master = sva_ast::parse_expr("@notes").expect("an expression");
    RefCell::new(now(Stream::open(graph, &master, config, tier)).expect("a stream"))
}

fn term(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

fn played(stream: &RefCell<Stream>, blocks: usize) -> Vec<u64> {
    let mut heard = Vec::new();
    for _ in 0..blocks {
        let block = next(&mut stream.borrow_mut()).expect("a block");
        heard.extend(block.expect("a block").plane(0).iter().map(|v| v.to_bits()));
    }
    heard
}

/// The names the disk was read under since the last call.
fn read(memory: &Memory) -> Vec<String> {
    let reads = std::mem::take(&mut *memory.reads.lock().unwrap());
    reads.into_iter().map(|(name, _)| name).collect()
}

/// A stream adds a stored note, plays past its end, takes a term whose graph renames every
/// instance, and adds the note again: only the first add reads the note's entries, header or
/// samples, and the blocks are a stream's over memory alone.
#[test]
fn a_stream_reads_a_note_off_the_disk_once_however_often_it_comes_back() {
    let memory = Memory::default();
    let graph = blip("once", 110);
    let note = persisted(&memory, &graph, "warm", 800);
    assert!(!note.is_empty());
    let player = opened(&memory, u64::MAX);
    let mut heard = Vec::new();
    for tier in [Some(&player), None] {
        let alone = Tier::default();
        let stream = match tier {
            Some(tier) => notes(&graph, 8_000, tier),
            None => notes(&graph, 8_000, &alone),
        };
        let add = |graph: &Graph, text: &str| match tier {
            Some(tier) => now(added(&stream, graph, &term(text), tier)),
            None => now(added(&stream, graph, &term(text), &alone)),
        };
        read(&memory);
        add(&graph, "@blip(t - 256sp, f0=200)").expect("added");
        let mut blocks = played(&stream, 8);
        if tier.is_some() {
            let first = read(&memory);
            assert!(first.iter().any(|name| note.contains(name)), "{first:?}");
        }
        add(&blip("once-renamed", 111), "@pad(t - 2500sp)").expect("added");
        blocks.extend(played(&stream, 2));
        add(&graph, "@blip(t - 3000sp, f0=200)").expect("added");
        blocks.extend(played(&stream, 8));
        if tier.is_some() {
            let later = read(&memory);
            let again: Vec<&String> = later.iter().filter(|name| note.contains(*name)).collect();
            assert_eq!(again, Vec::<&String>::new(), "read once: {later:?}");
        }
        heard.push(blocks);
    }
    assert!(heard[0].iter().any(|b| *b != 0), "silence tests nothing");
    assert!(
        heard[0] == heard[1],
        "memory's samples are the computed ones"
    );
}

/// A second stream over the same memory reads nothing off the disk: what the first read is
/// resident.
#[test]
fn a_second_stream_over_one_memory_reads_nothing_off_the_disk() {
    let memory = Memory::default();
    let graph = blip("reopened", 110);
    persisted(&memory, &graph, "warm", 800);
    let player = opened(&memory, u64::MAX);
    let mut heard = Vec::new();
    for _ in 0..2 {
        let stream = notes(&graph, 4_000, &player);
        now(added(
            &stream,
            &graph,
            &term("@blip(t - 256sp, f0=200)"),
            &player,
        ))
        .expect("added");
        heard.push((played(&stream, 8), stream.borrow().counts().tier));
    }
    let (first, second) = (&heard[0], &heard[1]);
    assert!(
        first.1.disk_reads > 0 && first.1.disk_lookups > 0,
        "{:?}",
        first.1
    );
    assert_eq!(
        (second.1.disk_lookups, second.1.disk_reads),
        (0, 0),
        "{:?}",
        second.1
    );
    assert!(first.0 == second.0);
}

fn bits(render: &Render) -> Vec<u64> {
    let root = render.output(render.root).expect("the root's samples");
    root.plane(0).iter().map(|v| v.to_bits()).collect()
}

/// A render the disk answered, made again over the same memory, calls the disk not once: every
/// header and sample it read is resident, and memory answers its root, one hit.
#[test]
fn a_second_identical_render_makes_no_disk_call() {
    let memory = Memory::default();
    let graph = blip("again", 110);
    persisted(&memory, &graph, "warm", 800);
    let tier = opened(&memory, u64::MAX);
    let first = now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    assert!(tier.counters().disk_reads > 0, "{:?}", tier.counters());
    let (held, counters) = (memory.held.lock().unwrap().clone(), tier.counters());
    read(&memory);
    memory.lists.store(0, Ordering::Relaxed);
    let second = now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    assert_eq!(read(&memory), Vec::<String>::new(), "no read");
    assert_eq!(memory.lists.load(Ordering::Relaxed), 0, "no listing");
    assert!(*memory.held.lock().unwrap() == held, "no write");
    let hit = Counters {
        hits: counters.hits + 1,
        ..counters
    };
    assert_eq!(tier.counters(), hit);
    assert_eq!(bits(&first), bits(&second));
}

/// Memory capped below a stored note's samples keeps none of them: each read is promoted and
/// evicted at once, the render plays them all the same, and the next render reads them again,
/// the trade a cap that small makes.
#[test]
fn memory_capped_below_a_note_evicts_it_and_reads_it_again() {
    let memory = Memory::default();
    let graph = blip("capped", 110);
    persisted(&memory, &graph, "warm", 800);
    let fresh = now(render_over(&graph, "warm", over(800), &Tier::default())).expect("a render");
    let tier = opened(&memory, u64::MAX);
    tier.set_max_bytes(800 * 8 / 2);
    let first = now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    let once: Counters = tier.counters();
    assert!(once.evictions() > 0 && once.promotions > 0, "{once:?}");
    assert!(tier.bytes() <= tier.max_bytes());
    let second = now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    let twice = tier.counters().since(once);
    assert!(twice.disk_reads > 0, "read again: {twice:?}");
    assert_eq!(bits(&first), bits(&fresh));
    assert_eq!(bits(&second), bits(&fresh));
}

/// A miss holds for the change that met it and no later one: what another holder commits
/// meanwhile is found by the next change, which reads it off the disk.
#[test]
fn a_miss_expires_so_another_holders_commit_is_found() {
    let memory = Memory::default();
    let graph = blip("expired", 110);
    let player = opened(&memory, u64::MAX);
    let stream = notes(&graph, 8_000, &player);
    let add = || {
        now(added(
            &stream,
            &graph,
            &term("@blip(t - 256sp, f0=200)"),
            &player,
        ))
    };
    add().expect("added");
    let missed = player.counters();
    assert!(
        missed.disk_lookups > 0 && missed.promotions == 0,
        "{missed:?}"
    );
    persisted(&memory, &graph, "warm", 800);
    add().expect("added");
    let found = player.counters().since(missed);
    assert!(found.disk_lookups > 0 && found.promotions > 0, "{found:?}");
    let hits = stream.borrow().stats().lookups;
    assert!(
        hits.iter()
            .any(|l| l.node.starts_with("blip") && l.store == Some(true))
    );
}

/// A held note at each of `hz`, two seconds of it, persisted.
fn held_notes(name: &str, hz: &[u32]) -> (Graph, Memory) {
    let held = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*crop(min(1, \
        (t - release)/0.03s), release, inf))\n";
    let mut nodes = vec![("string".to_string(), held.to_string())];
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
    for f in hz {
        persisted(&memory, &graph, &format!("warm{f}"), 16_000);
    }
    (graph, memory)
}

/// A fetch reads off the disk at most `FETCH_READS` times, however many stored notes the next
/// second plays; the next fetch reads on from there.
#[test]
fn a_fetch_makes_a_bounded_number_of_disk_reads() {
    let hz = [220, 247, 262, 294, 330, 349, 392, 440];
    let (graph, memory) = held_notes("fetched", &hz);
    let player = opened(&memory, u64::MAX);
    let stream = notes(&graph, 16_000, &player);
    for f in hz {
        let note = format!("@string(t, f0={f})");
        now(added(&stream, &graph, &term(&note), &player)).expect("added");
    }
    played(&stream, 32);
    let mut reads = Vec::new();
    for _ in 0..3 {
        let before = player.counters();
        now(fetch(&stream, &player));
        reads.push(player.counters().since(before).disk_reads);
    }
    assert!(hz.len() > FETCH_READS, "more notes than one fetch reads");
    assert_eq!(reads[0], FETCH_READS as u64, "{reads:?}");
    assert!(reads.iter().all(|n| *n <= FETCH_READS as u64), "{reads:?}");
    assert!(reads[1] > 0, "the next fetch reads on: {reads:?}");
}

/// A render the store answers whole computes nothing, so it prices nothing; the cold one it
/// stands on priced what it computed.
#[test]
fn a_render_the_store_answers_whole_prices_nothing() {
    let memory = Memory::default();
    let graph = blip("priced", 110);
    let cold = now(render_over(&graph, "warm", over(800), &Tier::default())).expect("a render");
    persisted(&memory, &graph, "warm", 800);
    let tier = opened(&memory, u64::MAX);
    let held = now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    assert!(cold.work().priced_flops > 0, "{:?}", cold.work());
    assert_eq!(held.work().priced_flops, 0, "{:?}", held.work());
    assert_eq!(held.work().samples, cold.work().samples);
    assert_eq!(bits(&held), bits(&cold));
}

/// A note larger than memory's whole cap still lands whole on the disk: each eviction sends
/// the disk what it lacks, so the next process renders it off the disk alone, bit for bit.
#[test]
fn a_node_larger_than_memory_lands_whole_on_the_disk() {
    let memory = Memory::default();
    let graph = graph_of(
        "larger",
        &[(
            "long",
            "crop(lowpass(sample(sin(2*pi*110*t)), cutoff=2000, q=0.7), 0s, 2s)\n",
        )],
    );
    let end = 2 * i64::from(RATE);
    let cold = now(render_over(&graph, "long", over(end), &Tier::default())).expect("a render");
    let small = opened(&memory, u64::MAX);
    small.set_max_bytes(end as u64 * 8 / 3);
    now(render_over(&graph, "long", over(end), &small)).expect("a render");
    assert!(small.counters().evictions() > 0, "{:?}", small.counters());
    now(small.persist()).expect("persisted");
    let next = opened(&memory, u64::MAX);
    let held = now(render_over(&graph, "long", over(end), &next)).expect("a render");
    let stats = held.cache_stats.as_ref().expect("stats");
    assert_eq!(stats.computed(), 0, "{stats:?}");
    assert_eq!(held.work().priced_flops, 0);
    assert_eq!(bits(&held), bits(&cold));
}

fn chain(name: &str, n3: &str) -> Graph {
    graph_of(
        name,
        &[
            ("n1", "sample(sin(2*pi*110*t))*0.5\n"),
            ("n2", "lowpass(@n1, cutoff=900, q=0.7)\n"),
            ("n3", n3),
            ("master", "@n3*0.5\n"),
        ],
    )
}

/// Rendered under a cap its values pass, persisted, then edited and rendered by a new process:
/// the node the edit left alone is read off the disk, and only what the edit reaches computes.
#[test]
fn a_new_process_reuses_the_disk_for_each_node_an_edit_left_alone() {
    let memory = Memory::default();
    let first = opened(&memory, u64::MAX);
    first.set_max_bytes(10_000);
    let before = chain("process", "lowpass(@n2, cutoff=600, q=0.7)\n");
    now(render_over(&before, "master", over(800), &first)).expect("a render");
    assert!(first.counters().evictions() > 0, "{:?}", first.counters());
    now(first.persist()).expect("persisted");
    let after = chain("process", "lowpass(@n2, cutoff=500, q=0.7)\n");
    let cold = now(render_over(&after, "master", over(800), &Tier::default())).expect("a render");
    let next = opened(&memory, u64::MAX);
    let edited = now(render_over(&after, "master", over(800), &next)).expect("a render");
    let stats = edited.cache_stats.as_ref().expect("stats");
    let answered = |node: &str| {
        let looked = stats.lookups.iter().filter(|l| l.node == node);
        looked.filter_map(|l| l.store).collect::<Vec<bool>>()
    };
    assert_eq!(answered("n2"), [true], "{stats:?}");
    assert_eq!(answered("n3"), [false]);
    assert_eq!(
        answered("n1"),
        Vec::<bool>::new(),
        "under a hit, never asked"
    );
    assert!(next.counters().disk_reads > 0, "{:?}", next.counters());
    assert_eq!(bits(&edited), bits(&cold));
}

/// A note evicted to the disk's staging and read back before any persist, then read shifted:
/// the shifted read lands on the disk with the note, so the next process answers it whole.
#[test]
fn a_shifted_read_of_a_note_staged_before_a_persist_lands_on_the_disk() {
    let memory = Memory::default();
    let mut graph = blip("staged", 110);
    assert!(graph.define("late", term("@blip(t - 80sp, f0=200)")));
    let tier = opened(&memory, u64::MAX);
    tier.set_max_bytes(800 * 8 / 2);
    now(render_over(&graph, "warm", over(800), &tier)).expect("a render");
    assert!(tier.counters().writebacks > 0, "{:?}", tier.counters());
    now(render_over(&graph, "late", over(880), &tier)).expect("a render");
    now(tier.persist()).expect("persisted");
    let next = opened(&memory, u64::MAX);
    let held = now(render_over(&graph, "late", over(880), &next)).expect("a render");
    let stats = held.cache_stats.as_ref().expect("stats");
    let late = stats.lookups.iter().filter(|l| l.node == "late");
    assert_eq!(
        late.map(|l| l.store).collect::<Vec<_>>(),
        [Some(true)],
        "{stats:?}"
    );
}
