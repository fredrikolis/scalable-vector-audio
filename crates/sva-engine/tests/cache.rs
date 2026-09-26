// Concern: states what a cache key is made of, so a warm render answers a cold one | Non-concern: a store's medium or budget (stores.rs) | IO: (a composition, twice) -> the same bytes

mod fixtures;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fixtures::dir_of;
use sva_engine::{
    Ask, Cache, Expected, MemoryCache, Payload, PayloadKind, Render, RenderConfig, Representation,
    render, symbolic_key,
};
use sva_formula::hash::hash_closed_form_under;
use sva_formula::{ClosedForm, Origin, Var};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

fn store_all() -> MemoryCache {
    MemoryCache::new()
}

fn rendered(dir: &Path, root: &str, cache: Option<&dyn Cache>) -> Render {
    let graph = sva_ast::parse_composition(dir).expect("a composition that parses");
    render(&graph, root, RenderConfig::seconds(RATE, SECONDS), cache)
        .unwrap_or_else(|e| panic!("rendering `{root}`: {e}"))
}

/// Every node held as samples, which is what a reading that consumes buffers asks for.
fn all_of(dir: &Path, root: &str, nodes: &[&str], cache: Option<&dyn Cache>) -> Render {
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

fn samples(r: &Render) -> BTreeMap<String, Vec<f64>> {
    r.buffers
        .iter()
        .map(|(id, b)| (r.tys.name(*id).to_string(), b.plane(0).to_vec()))
        .collect()
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
    let cold = samples(&rendered(&dir, "master", Some(&cache)));
    cache.sweep();
    assert!(cache.held_bytes() > 0, "the cold render filled the store");
    let warm = samples(&rendered(&dir, "master", Some(&cache)));
    assert_eq!(cold, warm);
}

/// A hit carries the label the collapse wrote, so a reused node still says how exact it is.
#[test]
fn a_reused_node_still_reports_its_label() {
    let dir = chain("labelled");
    let cache = store_all();
    let cold = rendered(&dir, "master", Some(&cache));
    let warm = rendered(&dir, "master", Some(&cache));
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
    let before = samples(&all_of(&dir, "master", &nodes, Some(&cache)));
    write(&dir, "voiced", "@chord*0.75\n");
    let after = samples(&all_of(&dir, "master", &nodes, Some(&cache)));
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
    let held = all_of(&spellings, "master", &["plain", "fx/walked"], Some(&cache));
    assert_eq!(
        held.buffer(held.id("plain").expect("plain")),
        held.buffer(held.id("fx/walked").expect("the walked spelling")),
        "one value, one entry, however the ref was spelled"
    );

    let renamed = chain("renamed");
    let cache = store_all();
    let before = samples(&all_of(
        &renamed,
        "master",
        &["voiced", "master"],
        Some(&cache),
    ));
    fs::rename(renamed.join("voiced"), renamed.join("coloured")).expect("a rename");
    write(&renamed, "master", "@coloured + @chord*0.25\n");
    let after = samples(&all_of(
        &renamed,
        "master",
        &["coloured", "master"],
        Some(&cache),
    ));
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
    let held = all_of(&identical, "master", &["left", "right"], Some(&cache));
    assert_eq!(
        held.buffer(held.id("left").expect("left")),
        held.buffer(held.id("right").expect("right")),
        "one written value, one entry"
    );
}

/// Subtraction does not commute, and the spectral sum is what says so: one order of a
/// difference never shares an entry with the other.
#[test]
fn reordering_a_difference_never_shares_an_entry_with_its_reverse() {
    let of = |name: &str, body: &str| {
        let dir = dir_of(name, &[("master", body)]);
        samples(&rendered(&dir, "master", None))["master"].clone()
    };
    let difference = of("difference", "sin(2*pi*256*t) - sin(2*pi*384*t)\n");
    let reversed = of("difference-reversed", "sin(2*pi*384*t) - sin(2*pi*256*t)\n");
    assert_ne!(difference, reversed);
}

/// Rate keys a buffer and nothing above it: the rate-free law behind two rates is one entry,
/// so each rate gets its own buffer while sharing the sum that produced it.
#[test]
fn a_rate_change_keys_a_new_buffer_but_reuses_the_rate_free_law() {
    let dir = chain("rates");
    let graph = sva_ast::parse_composition(&dir).expect("a composition");
    let cache = MemoryCache::new();
    let first = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        Some(&cache),
    )
    .expect("a render");
    let id = first.id("master").expect("the root");
    let symbolic = first
        .symbolic
        .get(&id)
        .expect("the root's spectral sum")
        .clone();
    cache.sweep();
    let held_at_one_rate = cache.held_bytes();
    assert!(held_at_one_rate > 0, "the first render kept its buffer");

    let second = render(
        &graph,
        "master",
        RenderConfig::seconds(RATE * 2, SECONDS),
        Some(&cache),
    )
    .expect("a render at another rate");
    let second_id = second.id("master").expect("the root");
    assert_eq!(
        second.symbolic[&second_id], symbolic,
        "one spectral sum answers both rates"
    );

    let one = first.buffer(id).expect("a buffer");
    let two = second.buffer(second_id).expect("a buffer");
    assert_eq!(two.len(), one.len() * 2, "each rate keeps its own buffer");
    cache.sweep();
    assert!(
        cache.held_bytes() > held_at_one_rate,
        "the second rate's buffer is a new entry, not a reuse of the first"
    );
}

#[test]
fn a_warm_cyclic_render_is_byte_identical_to_a_cold_one() {
    let dir = dir_of(
        "cyclic",
        &[("master", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let cache = store_all();
    let cold = samples(&rendered(&dir, "master", Some(&cache)));
    let warm = samples(&rendered(&dir, "master", Some(&cache)));
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
    let before = samples(&rendered(&dir, "master", Some(&cache)));
    write(&dir, "master", "sin(2*pi*220*t) + 0.25*self(t - 0.01s)\n");
    let after = samples(&rendered(&dir, "master", Some(&cache)));
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
    let refused = render(&graph, "master", RenderConfig::seconds(RATE, SECONDS), None);
    let Err(refused) = refused else {
        panic!("a loop of refs has no law");
    };
    assert_eq!(refused.code(), "engine.cyclic_substitution");
}

/// The table version is already inside a term's own hash, so a bump moves every symbolic
/// key at once and the old entries are simply never asked for again.
#[test]
fn a_table_version_bump_retires_symbolic_entries() {
    let form = ClosedForm {
        var: Var::T,
        body: sva_formula::Body::Line,
        origin: Origin::UNKNOWN,
    };
    let now = symbolic_key(hash_closed_form_under(&form, sva_formula::TABLE_VERSION));
    let later = symbolic_key(hash_closed_form_under(
        &form,
        sva_formula::TABLE_VERSION + 1,
    ));
    assert_ne!(now, later, "a bump is a new key for every law");

    let cache = MemoryCache::new();
    let payload = Payload::Symbolic(Box::new(
        sva_formula::normalize_closed_form(&form).expect("a spectral sum"),
    ));
    assert!(cache.worth_storing(
        std::time::Duration::from_millis(2),
        payload.bytes(),
        PayloadKind::Symbolic
    ));
    cache.store(now, &payload, &[], None);
    assert!(cache.load(now, "n", Expected::Symbolic).is_some());
    assert!(
        cache.load(later, "n", Expected::Symbolic).is_none(),
        "nothing answers the bumped key"
    );
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
                "istft(stft(sample(@chord), window=256, hop=64))\n",
            ),
        ],
    );
    let cache = MemoryCache::new();
    let cold = samples(&rendered(&dir, "master", Some(&cache)));
    cache.sweep();
    assert!(cache.held_bytes() > 0, "the analysis was kept");
    let warm = samples(&rendered(&dir, "master", Some(&cache)));
    assert_eq!(cold, warm);

    write(&dir, "chord", "sin(2*pi*512*t)\n");
    let edited = samples(&rendered(&dir, "master", Some(&cache)));
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
    let label_of = |cache: &dyn Cache| {
        let held = rendered(&dir, "chord", Some(cache));
        held.labels
            .get(&held.root)
            .expect("the root collapsed")
            .clone()
    };

    let cache = MemoryCache::holding(1 << 20);
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
            ("acc", "self(t - 1sp)*0.5 + sample(@tone)\n"),
            ("master", "@acc*0.5\n"),
        ],
    );
    let cache = store_all();
    let cold = all_of(&dir, "master", &["acc"], Some(&cache));
    assert_eq!(
        cold.tys.ty(cold.id("acc").expect("acc types")).held,
        sva_engine::Held::Sampled,
        "the node under test really is sampled"
    );
    let id = cold.id("acc").expect("acc types");
    let key = sva_engine::buffer_key(
        sva_engine::identity(&cold.tys, id).expect("acc has an identity"),
        RATE,
        0.0,
        (SECONDS * f64::from(RATE)).round() as usize,
        cold.tys.ty(id).width as usize,
        sva_samples::AliasScore::NotAsked,
    );
    assert!(cache.holds(key), "the sampled node is in the store");

    cache.sweep();
    let filled = cache.held_bytes();
    let warm = all_of(&dir, "master", &["acc"], Some(&cache));
    assert_eq!(samples(&cold), samples(&warm), "byte for byte");
    cache.sweep();
    assert_eq!(cache.held_bytes(), filled, "and nothing was written twice");
}

fn over(dir: &Path, root: &str, seconds: f64, cache: &dyn Cache) -> Render {
    let graph = sva_ast::parse_composition(dir).expect("a composition that parses");
    render(
        &graph,
        root,
        RenderConfig::seconds(RATE, seconds),
        Some(cache),
    )
    .unwrap_or_else(|e| panic!("rendering `{root}` for {seconds}s: {e}"))
}

fn every_buffer_hit(r: &Render) -> bool {
    let stats = r.cache_stats.as_ref().expect("a render handed a store");
    stats
        .lookups
        .iter()
        .filter(|l| l.kind == PayloadKind::Samples)
        .all(|l| matches!(l.outcome, sva_engine::Outcome::Hit(_)))
}

#[test]
fn two_horizons_of_one_node_are_two_entries() {
    let dir = dir_of(
        "horizons",
        &[
            ("acc", "sample(sin(2*pi*220*t))*0.5\n"),
            ("master", "@acc + @acc*0.25\n"),
        ],
    );
    let memory = MemoryCache::new();
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
    let store = MemoryCache::new();
    let root = |node: &str| {
        let held = rendered(&dir, node, Some(&store));
        let id = held.id(node).expect("the root types");
        let key = sva_engine::identity(&held.tys, id).expect("an identity");
        (held.buffer(id).expect("a buffer").plane(0).to_vec(), key)
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
