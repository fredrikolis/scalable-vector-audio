// Concern: states what one ledger entry says about the buffer under it | Non-concern: which nodes are walked (sva-engine) | IO: (buffers, deps) -> asserted entries

use std::collections::BTreeMap;

use sva_samples::measure::ledger::attribute;
use sva_samples::{Buffer, SignalKind};

fn entry(samples: Vec<f64>) -> sva_samples::LedgerEntry {
    let len = samples.len();
    let mut buffers = BTreeMap::new();
    buffers.insert("node".to_string(), Buffer::of_planes(48_000, vec![samples]));
    let mut kinds = BTreeMap::new();
    kinds.insert("node".to_string(), SignalKind::Audio);
    let entries = attribute(
        &buffers,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &kinds,
        "node",
        0..len,
        1,
    );
    let [held] = entries.as_slice() else {
        panic!("one node, one component, one entry: {entries:?}");
    };
    held.clone()
}

/// `clipped` means strictly outside [-1, 1], the bound a `.wav` clamps to: one rule, so two
/// signals at one peak read one verdict.
#[test]
fn a_peak_of_exactly_one_is_not_clipped() {
    let held = entry(vec![0.0, 1.0, -1.0, 0.5]);
    assert_eq!(held.peak, 1.0);
    assert_eq!(
        held.clipped,
        Some(false),
        "full scale is not past full scale"
    );

    let held = entry(vec![0.0, 1.0, -f64::from_bits(1.0f64.to_bits() + 1), 0.5]);
    assert!(held.peak > 1.0, "{}", held.peak);
    assert_eq!(
        held.clipped,
        Some(true),
        "one bit past full scale is past full scale"
    );

    let held = entry(vec![0.0, 0.9999, -0.5]);
    assert_eq!(held.clipped, Some(false));
}

/// A share is the part of its reader's own energy one contribution accounts for. Over a
/// sum the parts are the whole, so the shares of one reader's refs add to exactly one,
/// however they correlate.
#[test]
fn ledger_shares_sum_to_one_for_a_sum() {
    let len = 480;
    let wave = |hz: f64, amp: f64| -> Vec<f64> {
        (0..len)
            .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin())
            .collect()
    };
    let (a, b, c) = (wave(200.0, 0.3), wave(300.0, 0.5), wave(200.0, 0.2));
    let root: Vec<f64> = (0..len).map(|i| a[i] + b[i] + c[i]).collect();

    let mut buffers = BTreeMap::new();
    let mut contributed = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    let mut deps = BTreeMap::new();
    buffers.insert("root".to_string(), Buffer::of_planes(48_000, vec![root]));
    for (name, plane) in [("a", a), ("b", b), ("c", c)] {
        let buffer = Buffer::of_planes(48_000, vec![plane]);
        buffers.insert(name.to_string(), buffer.clone());
        contributed.insert(name.to_string(), buffer);
        kinds.insert(name.to_string(), SignalKind::Audio);
    }
    kinds.insert("root".to_string(), SignalKind::Audio);
    deps.insert(
        "root".to_string(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    );

    let entries = attribute(&buffers, &contributed, &deps, &kinds, "root", 0..len, 1);
    let share = |node: &str| {
        entries
            .iter()
            .find(|e| e.node == node)
            .unwrap_or_else(|| panic!("`{node}` is in the ledger"))
            .share
            .expect("an audio share")
    };
    assert_eq!(share("root"), 1.0, "the target is the whole of itself");
    let total = share("a") + share("b") + share("c");
    assert!(
        (total - 1.0).abs() < 1e-12,
        "three refs, one whole: {total} (`a` and `c` share a frequency)"
    );
    assert!(
        (share("b") - 0.5).abs() < 1e-3 && (share("a") + share("c") - 0.5).abs() < 1e-3,
        "a and c sum to b's amplitude at another frequency, so each side is half the energy"
    );
}

/// A node composed into another holds no buffer of its own, so it declares no kind: the
/// ledger still walks it through `deps`, and reads it as audio rather than skipping it.
#[test]
fn a_node_with_no_buffer_of_its_own_is_read_as_audio() {
    let len = 8;
    let mut buffers = BTreeMap::new();
    buffers.insert("root".to_string(), Buffer::mono(8_000, vec![0.5; len]));
    let mut deps = BTreeMap::new();
    deps.insert("root".to_string(), vec!["composed".to_string()]);
    deps.insert("composed".to_string(), Vec::new());
    let mut kinds = BTreeMap::new();
    kinds.insert("root".to_string(), SignalKind::Audio);

    let entries = attribute(&buffers, &BTreeMap::new(), &deps, &kinds, "root", 0..len, 2);
    let composed = entries
        .iter()
        .find(|e| e.node == "composed")
        .expect("a node with no kind is still walked");
    assert_eq!(composed.kind, SignalKind::Audio);
}
