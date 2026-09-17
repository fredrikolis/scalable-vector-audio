// Concern: proves a ledger names every ref under its target, whatever each holds | Non-concern: the arithmetic of one entry (sva-samples) | IO: (a composition) -> LedgerEntry per node

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, LedgerEntry, Output, RenderConfig, Representation, answer, render};

fn ledger(name: &str, files: &[(&str, &str)], depth: usize) -> Vec<LedgerEntry> {
    let g = graph_of(name, files);
    let asks = vec![Ask {
        node: "master".to_string(),
        representation: Representation::Ledger { depth },
    }];
    let held = render(
        &g,
        "master",
        RenderConfig::seconds(8_000, 1.0).asking(asks),
        None,
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    let id = held.id("master").expect("the root");
    match answer(&held, id, Representation::Ledger { depth })
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .value
    {
        Output::Ledger(entries) => entries,
        other => panic!("expected a ledger, got {other:?}"),
    }
}

fn named<'a>(entries: &'a [LedgerEntry], node: &str) -> &'a LedgerEntry {
    entries
        .iter()
        .find(|e| e.node == node)
        .unwrap_or_else(|| panic!("`{node}` is not in the ledger: {:?}", names(entries)))
}

fn names(entries: &[LedgerEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.node.as_str()).collect()
}

/// The tree a ledger walks is the ref tree, and a closed form's refs are written in its own body.
#[test]
fn a_law_master_ledgers_each_ref_share() {
    let entries = ledger(
        "law-master",
        &[
            ("quiet", "0.1*sin(2*pi*200*t)\n"),
            ("loud", "0.8*sin(2*pi*300*t)\n"),
            ("master", "@quiet(t) + @loud(t)\n"),
        ],
        3,
    );

    assert_eq!(
        named(&entries, "master").share,
        Some(1.0),
        "the target carries its own whole energy"
    );
    let quiet = named(&entries, "quiet");
    let loud = named(&entries, "loud");
    assert!(
        (quiet.rms - 0.1 / 2f64.sqrt()).abs() < 1e-3,
        "a sine at 0.1 reads its own rms: {}",
        quiet.rms
    );
    assert!(
        loud.share.expect("an audio share") > quiet.share.expect("an audio share"),
        "the louder ref carries the larger share: {quiet:?} {loud:?}"
    );
}

/// Forwarding one ref is not a reason to vanish: the file is a node and carries all of it.
#[test]
fn a_pass_through_node_appears_in_the_ledger() {
    let entries = ledger(
        "pass-through",
        &[
            ("tone", "0.4*sin(2*pi*200*t)\n"),
            ("voice", "@tone(t)\n"),
            ("master", "@voice(t)\n"),
        ],
        3,
    );

    let held = names(&entries);
    assert!(held.contains(&"voice"), "{held:?}");
    assert_eq!(
        named(&entries, "voice").share,
        Some(1.0),
        "a node that forwards one ref forwards all of its energy"
    );
    assert!(held.contains(&"tone"), "{held:?}");
}

/// Two chains reach one node at two depths. The reading walks level by level, so the node
/// is held at the shorter chain's depth, and a longer chain reaching it first must not
/// spend the budget that would have carried its own children.
#[test]
fn a_node_two_chains_reach_is_held_at_the_shorter_one() {
    let entries = ledger(
        "diamond",
        &[
            ("leaf", "0.2*sin(2*pi*500*t)\n"),
            ("near", "@leaf(t)\n"),
            ("far", "@near(t)\n"),
            ("master", "@far(t) + @near(t)\n"),
        ],
        2,
    );

    let held = names(&entries);
    assert!(held.contains(&"near"), "depth 1 through `near`: {held:?}");
    assert!(held.contains(&"far"), "depth 1 through `far`: {held:?}");
    assert!(
        held.contains(&"leaf"),
        "`leaf` sits two hops down the short chain: {held:?}"
    );
    assert!(
        named(&entries, "leaf").rms > 0.0,
        "a named node is a rendered one, never a zero standing in for one"
    );
}

/// An entry is what a ref contributed to the node reading it, at that node's own offset, not
/// what the ref's own buffer holds from its own zero.
#[test]
fn a_shifted_section_contributes_nothing_before_its_bar() {
    let entries = ledger(
        "shifted-section",
        &[
            ("sections/s1", "crop(0.5*sin(2*pi*220*t), 0s, 2s)\n"),
            ("sections/s2", "crop(0.5*sin(2*pi*330*t), 0s, 2s)\n"),
            (
                "master",
                "crop(@sections/s1(t) + @sections/s2(t - 2s), 0s, 4s)\n",
            ),
        ],
        1,
    );

    let early = named(&entries, "sections/s2");
    assert_eq!(early.rms, 0.0, "s2 sounds from the third second on");
    assert_eq!(early.share, Some(0.0));
    let sounding = named(&entries, "sections/s1");
    assert!(
        (sounding.share.expect("an audible share") - 1.0).abs() < 1e-9,
        "s1 is the whole of the first second: {:?}",
        sounding.share
    );
}

/// A share is of the node reading the ref, not of the target two hops up.
#[test]
fn a_ref_is_a_share_of_the_node_reading_it() {
    let entries = ledger(
        "share-of-its-reader",
        &[
            ("voice", "0.5*sin(2*pi*220*t)\n"),
            ("section", "@voice(t)\n"),
            ("master", "0.1*@section(t)\n"),
        ],
        2,
    );

    let section = named(&entries, "section");
    assert!(
        (section.share.expect("an audible share") - 1.0).abs() < 1e-9,
        "the section is the whole of master's own level: {:?}",
        section.share
    );
    let voice = named(&entries, "voice");
    assert!(
        (voice.share.expect("an audible share") - 1.0).abs() < 1e-9,
        "and the voice is the whole of the section's, not ten times master's: {:?}",
        voice.share
    );
}

/// A share is a share of a sum; crediting a factor with the whole product counts it twice.
#[test]
fn a_factor_of_a_product_has_no_share() {
    let entries = ledger(
        "factor-of-a-product",
        &[
            ("carrier", "sin(2*pi*220*t)\n"),
            ("shape", "0.5 + 0.5*cos(2*pi*3*t)\n"),
            ("master", "@carrier(t)*@shape(t)\n"),
        ],
        1,
    );

    for node in ["carrier", "shape"] {
        let held = named(&entries, node);
        assert_eq!(held.share, None, "`{node}` is a factor, not an addend");
        assert!(held.rms > 0.0, "`{node}` still states what it holds");
    }
    assert_eq!(named(&entries, "master").share, Some(1.0));
}

/// A ref the window does not reach contributed nothing to it, whatever its own buffer holds.
#[test]
fn a_ref_outside_the_window_has_zero_share() {
    let entries = ledger(
        "outside-the-window",
        &[
            ("early", "crop(0.5*sin(2*pi*220*t), 0s, 0.5s)\n"),
            ("late", "crop(0.5*sin(2*pi*330*t), 0s, 0.5s)\n"),
            ("master", "@early(t) + @late(t - 2s)\n"),
        ],
        1,
    );

    assert_eq!(named(&entries, "late").share, Some(0.0));
    assert_eq!(named(&entries, "late").rms, 0.0);
    assert!(
        (named(&entries, "early").share.expect("an audio share") - 1.0).abs() < 1e-12,
        "the one ref the window reaches is the whole of it"
    );
}

/// A sampled node reads its refs through slots, and silencing every slot but one leaves what
/// that ref put into the buffer. Without that a sampled target answers one row: itself.
#[test]
fn ledger_walks_through_a_sampled_node_to_its_slots() {
    let entries = ledger(
        "sampled-master",
        &[
            (
                "hat",
                "crop(0.3*sin(2*pi*3000*t), 0s, 0.4s, rise=0.002s, fall=0.2s)\n",
            ),
            ("chord", "0.2*sin(2*pi*220*t)\n"),
            ("glue", "sample(@chord(t)) + 0.3*self(t - 1sp)\n"),
            (
                "master",
                "crop(0.6*(@glue(t) + sample(@hat(t)) + sample(@hat(t - 0.5s))), 0s, 1s)\n",
            ),
        ],
        2,
    );

    let held = names(&entries);
    for node in ["glue", "hat"] {
        assert!(
            held.contains(&node),
            "`{node}` is a buffer master reads: {held:?}"
        );
        assert!(named(&entries, node).rms > 0.0, "and it sounds");
    }
    let under_master: f64 = entries
        .iter()
        .filter(|e| e.depth == 1)
        .filter_map(|e| e.share)
        .sum();
    assert!(
        (under_master - 1.0).abs() < 1e-9,
        "the slots of a sum are the whole of it: {under_master}"
    );
    assert!(
        named(&entries, "hat").share.expect("an audio share") > 0.0,
        "both reads of one buffer stand in the one row that buffer gets"
    );
    assert!(
        held.contains(&"chord"),
        "and the walk carries on through `glue` into the closed form it sampled: {held:?}"
    );
}
