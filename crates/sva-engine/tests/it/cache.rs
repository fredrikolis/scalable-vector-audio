// Concern: states what a cache key is made of, so a warm render answers a cold one | Non-concern: memory's cap and prunes (stores.rs) | IO: (a composition, twice) -> the same bytes

use std::fs;
use std::path::{Path, PathBuf};

use crate::fixtures::{dir_of, samples};
use sva_engine::{Ask, PayloadKind, Render, RenderConfig, Representation, Tier, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

fn store_all() -> Tier {
    Tier::default()
}

fn rendered(dir: &Path, root: &str, cache: &Tier) -> Render {
    let graph = sva_ast::parse_composition(dir).expect("a composition that parses");
    render(&graph, root, RenderConfig::seconds(RATE, SECONDS), cache)
        .unwrap_or_else(|e| panic!("rendering `{root}`: {e}"))
}

/// Every node held as samples, which is what a reading that consumes buffers asks for.
fn all_of(dir: &Path, root: &str, nodes: &[&str], cache: &Tier) -> Render {
    let graph = sva_ast::parse_composition(dir).expect("a composition that parses");
    let asks = nodes
        .iter()
        .map(|node| Ask {
            node: (*node).to_string(),
            representation: Representation::Samples,
        })
        .collect();
    render(
        &graph,
        root,
        RenderConfig::seconds(RATE, SECONDS).asking(asks),
        cache,
    )
    .unwrap_or_else(|e| panic!("rendering `{root}`: {e}"))
}

fn write(dir: &Path, rel: &str, text: &str) {
    fs::write(dir.join(rel), text).expect("a node file");
}

/// One composition of a chord, a voicing over it and a master that sums both.
fn chain(name: &str) -> PathBuf {
    dir_of(
        name,
        &[
            ("chord", "sin(2*pi*256*t) + sin(2*pi*384*t)\n"),
            ("voiced", "@chord*0.5\n"),
            ("master", "@voiced + @chord*0.25\n"),
        ],
    )
}

#[test]
fn a_warm_render_is_byte_identical_to_a_cold_one() {
    let dir = chain("warm-cold");
    let cache = store_all();
    let cold = samples(&rendered(&dir, "master", &cache));
    assert!(cache.bytes() > 0, "the cold render filled the store");
    let warm = samples(&rendered(&dir, "master", &cache));
    assert_eq!(cold, warm);
}

/// A hit carries the label the collapse wrote, so a reused node still says how exact it is.
#[test]
fn a_reused_node_still_reports_its_label() {
    let dir = chain("labelled");
    let cache = store_all();
    let cold = rendered(&dir, "master", &cache);
    let warm = rendered(&dir, "master", &cache);
    let id = warm.id("master").expect("the root");
    assert_eq!(
        warm.labels[&id].profile,
        cold.labels[&cold.id("master").expect("the root")].profile
    );
    assert_eq!(warm.labels[&id].rate, RATE);
}

#[test]
fn editing_one_file_re_renders_it_and_its_dependents_and_nothing_else() {
    let dir = chain("edited");
    let cache = store_all();
    let nodes = ["chord", "voiced", "master"];
    let before = samples(&all_of(&dir, "master", &nodes, &cache));
    write(&dir, "voiced", "@chord*0.75\n");
    let after = samples(&all_of(&dir, "master", &nodes, &cache));
    assert_eq!(
        before["chord"], after["chord"],
        "the untouched law is one value"
    );
    assert_ne!(before["voiced"], after["voiced"]);
    assert_ne!(before["master"], after["master"]);
}

/// A cache key is content, not the path or spelling a node was written under: two spellings
/// of one ref, a file renamed after it rendered, and a subtree duplicated verbatim all answer
/// from one entry.
#[test]
fn a_cache_key_follows_content_not_the_path_or_spelling_it_was_written_under() {
    let spellings = dir_of(
        "spellings",
        &[
            ("chord", "sin(2*pi*256*t)\n"),
            ("plain", "@chord*0.5\n"),
            ("fx/walked", "@../chord*0.5\n"),
            ("master", "@plain + @fx/walked\n"),
        ],
    );
    let cache = store_all();
    let held = all_of(&spellings, "master", &["plain", "fx/walked"], &cache);
    assert_eq!(
        held.output(held.id("plain").expect("plain")),
        held.output(held.id("fx/walked").expect("the walked spelling")),
        "one value, one entry, however the ref was spelled"
    );

    let renamed = chain("renamed");
    let cache = store_all();
    let before = samples(&all_of(&renamed, "master", &["voiced", "master"], &cache));
    fs::rename(renamed.join("voiced"), renamed.join("coloured")).expect("a rename");
    write(&renamed, "master", "@coloured + @chord*0.25\n");
    let after = samples(&all_of(&renamed, "master", &["coloured", "master"], &cache));
    assert_eq!(before["master"], after["master"], "one value, one key");
    assert_eq!(before["voiced"], after["coloured"]);

    let identical = dir_of(
        "identical",
        &[
            ("left", "sin(2*pi*256*t)*0.5\n"),
            ("right", "sin(2*pi*256*t)*0.5\n"),
            ("master", "@left + @right\n"),
        ],
    );
    let cache = store_all();
    let held = all_of(&identical, "master", &["left", "right"], &cache);
    assert_eq!(
        held.output(held.id("left").expect("left")),
        held.output(held.id("right").expect("right")),
        "one written value, one entry"
    );
}

/// Subtraction does not commute, and the spectral sum is what says so: one order of a
/// difference never shares an entry with the other.
#[test]
fn reordering_a_difference_never_shares_an_entry_with_its_reverse() {
    let of = |name: &str, body: &str| {
        let dir = dir_of(name, &[("master", body)]);
        samples(&rendered(&dir, "master", &Tier::default()))["master"].clone()
    };
    let difference = of("difference", "sin(2*pi*256*t) - sin(2*pi*384*t)\n");
    let reversed = of("difference-reversed", "sin(2*pi*384*t) - sin(2*pi*256*t)\n");
    assert_ne!(difference, reversed);
}

/// Rate keys every value: a render at another rate stores values of its own beside the first.
#[test]
fn a_rate_change_keys_new_values() {
    let dir = chain("rates");
    let graph = sva_ast::parse_composition(&dir).expect("a composition");
    let cache = Tier::default();
    let first = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        &cache,
    )
    .expect("a render");
    let id = first.id("master").expect("the root");
    let held_at_one_rate = cache.bytes();
    assert!(held_at_one_rate > 0, "the first render kept its values");

    let second = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE * 2, SECONDS),
        &cache,
    )
    .expect("a render at another rate");
    let second_id = second.id("master").expect("the root");
    let one = first.output(id).expect("a buffer");
    let two = second.output(second_id).expect("a buffer");
    assert_eq!(two.len(), one.len() * 2, "each rate keeps its own buffer");
    assert!(
        cache.bytes() > held_at_one_rate,
        "the second rate's values are new entries, not a reuse of the first"
    );
}

#[test]
fn a_warm_cyclic_render_is_byte_identical_to_a_cold_one() {
    let dir = dir_of(
        "cyclic",
        &[("master", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let cache = store_all();
    let cold = samples(&rendered(&dir, "master", &cache));
    let warm = samples(&rendered(&dir, "master", &cache));
    assert_eq!(cold, warm);
}

/// A loop's own gain is part of the value, so editing it retires the whole loop's entry.
#[test]
fn editing_a_loop_retires_it() {
    let dir = dir_of(
        "loop-edit",
        &[("master", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let cache = store_all();
    let before = samples(&rendered(&dir, "master", &cache));
    write(&dir, "master", "sin(2*pi*220*t) + 0.25*self(t - 0.01s)\n");
    let after = samples(&rendered(&dir, "master", &cache));
    assert_ne!(before["master"], after["master"], "the loop retires");
}

/// A loop of refs closes through no `self`, so nothing substitutes it: it says so rather
/// than unrolling forever.
#[test]
fn a_loop_of_refs_refuses_rather_than_substituting_forever() {
    let dir = dir_of(
        "ref-loop",
        &[
            ("a", "sin(2*pi*220*t) + 0.5*@b(t - 0.01s)\n"),
            ("b", "@a(t - 0.01s)*0.5\n"),
            ("master", "@a\n"),
        ],
    );
    let graph = sva_ast::parse_composition(&dir).expect("a composition");
    let refused = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        &Tier::default(),
    );
    let Err(refused) = refused else {
        panic!("a loop of refs has no law");
    };
    assert_eq!(refused.code(), "engine.cyclic_substitution");
}

/// One analysis of one buffer is one entry, and the buffer's own content is in its key:
/// editing what was analysed retires it.
#[test]
fn frames_are_held_under_the_window_they_were_read_through() {
    let dir = dir_of(
        "frames",
        &[
            ("chord", "sin(2*pi*256*t)\n"),
            (
                "master",
                "istft(stft(sample(crop(@chord, 0s, 0.05s)), window=256sp, hop=64sp))\n",
            ),
        ],
    );
    let cache = Tier::default();
    let cold = samples(&rendered(&dir, "master", &cache));
    assert!(cache.bytes() > 0, "the analysis was kept");
    let warm = samples(&rendered(&dir, "master", &cache));
    assert_eq!(cold, warm);

    write(&dir, "chord", "sin(2*pi*512*t)\n");
    let edited = samples(&rendered(&dir, "master", &cache));
    assert_ne!(cold, edited, "editing what was analysed retires the frames");
}

/// FORMAT 9.3: the label is part of the value a collapse produced, so it is stored with the
/// buffer and answered on a hit. Warm and cold are one command answering one way.
#[test]
fn a_warm_hit_carries_the_cold_label() {
    let dir = dir_of(
        "warm-label",
        &[("chord", "sin(2*pi*256*t) + sin(2*pi*384*t)\n")],
    );
    let label_of = |cache: &Tier| {
        let held = rendered(&dir, "chord", cache);
        held.labels
            .get(&held.root)
            .expect("the root collapsed")
            .clone()
    };

    let cache = Tier::new(1 << 20);
    let cold = label_of(&cache);
    assert_eq!(
        cold.source,
        sva_engine::Source::Exact,
        "the cold run is exact"
    );
    let sva_engine::Detail::Lines { placed, summed, .. } = &cold.detail else {
        panic!("a line spectrum states what it placed: {cold:?}");
    };
    assert!(placed + summed > 0, "the lines are counted");

    let warm = label_of(&cache);
    assert_eq!(
        warm.detail, cold.detail,
        "a hit answers the row the cold run took"
    );
    assert_eq!(warm.source, cold.source);
    assert_eq!(warm.rate, cold.rate);
    assert_eq!(warm.profile, cold.profile);
}

/// A sampled node is the dearest kind the engine runs and the one a caller most wants back
/// from the store, so the cache contract has to be proven over one, not only over collapses.
#[test]
fn a_sampled_node_is_stored_and_answered_from_the_store() {
    let dir = dir_of(
        "sampled-cache",
        &[
            ("tone", "sin(2*pi*220*t)\n"),
            ("acc", "self[idx(t) - 1]*0.5 + sample(@tone)\n"),
            ("master", "@acc*0.5\n"),
        ],
    );
    let cache = store_all();
    let cold = all_of(&dir, "master", &["acc"], &cache);
    assert_eq!(
        cold.tys.ty(cold.id("acc").expect("acc types")).held,
        sva_engine::Held::Sampled,
        "the node under test really is sampled"
    );
    let stats = cold.cache_stats.as_ref().expect("a render handed a store");
    let key = stats
        .lookups
        .iter()
        .rev()
        .find(|l| l.node == "acc")
        .expect("acc was looked up")
        .key;
    assert!(cache.holds(key), "the sampled node is in the store");

    let warm = all_of(&dir, "master", &["acc"], &cache);
    assert_eq!(samples(&cold), samples(&warm), "byte for byte");
    let filled = cache.bytes();
    let plain = rendered(&dir, "master", &cache);
    assert_eq!(samples(&plain)["master"], samples(&cold)["master"]);
    assert_eq!(cache.bytes(), filled, "and nothing was written twice");
}

fn over(dir: &Path, root: &str, seconds: f64, cache: &Tier) -> Render {
    let graph = sva_ast::parse_composition(dir).expect("a composition that parses");
    render(&graph, root, RenderConfig::seconds(RATE, seconds), cache)
        .unwrap_or_else(|e| panic!("rendering `{root}` for {seconds}s: {e}"))
}

fn every_buffer_hit(r: &Render) -> bool {
    let stats = r.cache_stats.as_ref().expect("a render handed a store");
    stats
        .lookups
        .iter()
        .filter(|l| matches!(l.kind, PayloadKind::Segments | PayloadKind::Run))
        .all(|l| matches!(l.outcome, sva_engine::Outcome::Hit))
}

/// A longer horizon extends the one value a shorter one stored, and either reads it back.
#[test]
fn two_horizons_of_one_node_are_one_value() {
    let dir = dir_of(
        "horizons",
        &[
            ("acc", "sample(sin(2*pi*220*t))*0.5\n"),
            ("master", "@acc + @acc*0.25\n"),
        ],
    );
    let memory = Tier::default();
    let store = &memory;
    let short = samples(&over(&dir, "master", SECONDS, store));
    let long = samples(&over(&dir, "master", 2.0 * SECONDS, store));
    let again_short = over(&dir, "master", SECONDS, store);
    let again_long = over(&dir, "master", 2.0 * SECONDS, store);
    assert!(
        every_buffer_hit(&again_short),
        "the short horizon stayed held"
    );
    assert!(every_buffer_hit(&again_long), "and so did the long one");
    assert_eq!(samples(&again_short), short);
    assert_eq!(samples(&again_long), long);
}

#[test]
fn sat_drives_a_sampled_operand_as_it_drives_a_closed_form() {
    let dir = dir_of(
        "sat-drive",
        &[
            ("x", "sample(sin(2*pi*220*t))*0.5\n"),
            ("soft", "sat(@x, drive=1)\n"),
            ("hard", "sat(@x, drive=5)\n"),
            ("written", "sat(@x*5)\n"),
        ],
    );
    let store = Tier::default();
    let root = |node: &str| {
        let held = rendered(&dir, node, &store);
        let id = held.id(node).expect("the root types");
        let key = sva_engine::identity(&held.tys, id).expect("an identity");
        (held.output(id).expect("a buffer").plane(0).to_vec(), key)
    };
    let (soft, soft_key) = root("soft");
    let (hard, hard_key) = root("hard");
    let (written, written_key) = root("written");
    assert_ne!(soft, hard, "the drive reached the renderer");
    assert_ne!(soft_key, hard_key, "and the key");
    assert_eq!(
        hard, written,
        "sat(x, drive=d) is sat(x*d), sample for sample"
    );
    assert_eq!(hard_key, written_key, "under one key");
}

/// A stateful node read at any shift is one value, however many samples apart the reads are:
/// a second reader at the same offset and a third at another are answered by the first.
#[test]
fn reads_of_a_stateful_node_at_any_offset_share_one_value() {
    let dir = dir_of(
        "offsets",
        &[
            (
                "kick",
                "lowpass(crop(sample(sin(2*pi*55*t)), 0s, 0.05s), cutoff=900, q=0.8)\n",
            ),
            ("early", "@kick(t - 0.1234s)\n"),
            ("again", "0.5*@kick(t - 0.1234s)\n"),
            ("later", "@kick(t - 0.2345s)\n"),
        ],
    );
    let cache = store_all();
    let kick = |root: &str| {
        let r = over(&dir, root, 0.4, &cache);
        let stats = r.cache_stats.expect("a render handed a store");
        stats
            .lookups
            .into_iter()
            .filter(|l| l.node == "kick")
            .map(|l| l.outcome)
            .collect::<Vec<_>>()
    };
    let hit = |o: &sva_engine::Outcome| *o == sva_engine::Outcome::Hit;
    let cold = kick("early");
    assert!(!cold.is_empty() && !cold.iter().any(hit), "{cold:?}");
    let again = kick("again");
    assert!(!again.is_empty() && again.iter().all(hit), "{again:?}");
    let later = kick("later");
    assert!(!later.is_empty() && later.iter().all(hit), "{later:?}");
}

/// A series' index is numbered by the typing that lowers it, so a node lowered after another
/// series holds other numbers; its key is the node's own, wherever it is read from.
#[test]
fn a_series_keys_alike_whatever_was_lowered_before_it() {
    let graph = crate::fixtures::graph_of(
        "series-key",
        &[
            ("a", "sum(j, 1, 50, (1/j)*sin(2*pi*30*j*t))\n"),
            ("b", "sum(k, 1, 400, (1/k)*sin(2*pi*100*k*t))\n"),
            ("song", "@a + @b\n"),
        ],
    );
    let key = |root: &str| {
        let tys = sva_engine::types(&graph, root).expect("typed");
        sva_engine::identity(&tys, tys.id("b").expect("b")).expect("an identity")
    };
    assert_eq!(key("song"), key("b"));
}

/// A profile's floor decides how many terms a series keeps, so a render under another floor
/// computes its own value rather than answering with the first profile's.
#[test]
fn a_render_under_another_floor_is_not_answered_by_the_first() {
    let dir = dir_of(
        "floors",
        &[("node", "sum(k, 1, inf, cos(2*pi*440*t)/(k*k))\n")],
    );
    let graph = sva_ast::parse_composition(&dir).expect("a composition that parses");
    let under = |floor_db: f64, cache: &Tier| {
        let config = RenderConfig {
            profile: sva_engine::Profile {
                floor_db,
                floor_db_above_5k: floor_db,
                ..sva_engine::PSYCHOACOUSTIC_V1
            },
            ..RenderConfig::seconds(RATE, SECONDS)
        };
        samples(&render(&graph, "node", config, cache).expect("a render"))
    };
    let cold = under(-60.0, &store_all());
    assert!(
        cold != under(-20.0, &store_all()),
        "the floor moves the samples"
    );
    let cache = store_all();
    under(-20.0, &cache);
    assert!(
        under(-60.0, &cache) == cold,
        "a warm render under another floor is the cold one"
    );
}
