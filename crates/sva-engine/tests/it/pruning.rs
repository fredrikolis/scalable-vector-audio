// Concern: proves a render skips only work that is exactly zero, writing the same bits | Non-concern: what any row computes (sva-samples) | IO: (a composition, a range) -> samples, priced work

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Ask, Cache, Output, Range, Render, RenderConfig, Representation, answer, flops, render,
};
use sva_samples::LATTICE_8K;

const RATE: u32 = LATTICE_8K.lattice_hz;

fn notes() -> Graph {
    graph_of(
        "pruning",
        &[
            ("note", "crop(sin(2*pi*440*t)*exp(-t/0.2), 0s, 0.5s)\n"),
            ("song", "@note(t) + @note(t - 2s) + @note(t - 4s)\n"),
        ],
    )
}

fn sampled_notes() -> Graph {
    graph_of(
        "pruning-sampled",
        &[
            (
                "note",
                "crop(lowpass(sample(0.5*sin(2*pi*440*t)), cutoff=2000, q=0.7), 0s, 0.5s)\n",
            ),
            ("song", "@note(t) + @note(t - 2s) + @note(t - 4s)\n"),
        ],
    )
}

fn over(g: &Graph, target: &str, secs: f64, cache: Option<&Cache>) -> Render {
    render(g, target, config(secs), cache).unwrap_or_else(|e| panic!("{target}: {e}"))
}

fn config(secs: f64) -> RenderConfig {
    RenderConfig {
        range: Range {
            start: Some(0),
            end: Some((secs * f64::from(RATE)) as i64),
        },
        ..RenderConfig::at(RATE).under(LATTICE_8K)
    }
}

/// A range reaching further past the last note is the same stored value.
#[test]
fn a_closed_form_sums_only_the_atoms_live_at_each_instant() {
    let g = notes();
    let cache = Cache::new();
    let held = over(&g, "song", 6.0, Some(&cache));
    let sum = &held.symbolic[&held.root];
    let samples = held.output(held.root).expect("the song").plane(0).to_vec();
    let step = 1.0 / f64::from(RATE);
    for (n, sample) in samples.iter().enumerate() {
        let whole = sva_samples::eval_spectral_sum_at(sum, 0, n as f64 * step).expect("a value");
        assert_eq!(sample.to_bits(), whole.re.to_bits(), "sample {n}");
    }
    let (atoms, live) = (2 * 3, u128::from(RATE / 2));
    assert_eq!(held.work().priced_flops, atoms * live);

    let further = over(&g, "song", 8.0, Some(&cache));
    let stats = further.cache_stats.as_ref().expect("stats");
    assert_eq!(stats.hits(), stats.lookups.len(), "{stats:?}");
    let reread = further.output(further.root).expect("the song").plane(0)[..samples.len()].to_vec();
    assert_eq!(reread, samples);
}

/// A sum of sampled notes reads each only while it sounds: the bits are the notes added in
/// order from +0, and the sum pays only for the notes sounding.
#[test]
fn a_sampled_sum_reads_each_operand_only_where_it_is_nonzero() {
    let g = sampled_notes();
    let held = over(&g, "song", 4.5, None);
    let song = held.output(held.root).expect("the song").plane(0).to_vec();
    let alone = over(&g, "note", 4.5, None);
    let note = alone
        .output(alone.root)
        .expect("the note")
        .plane(0)
        .to_vec();
    let at = |n: i64| {
        usize::try_from(n)
            .ok()
            .and_then(|n| note.get(n))
            .copied()
            .unwrap_or(0.0)
    };
    let gap = i64::from(RATE) * 2;
    for (n, sample) in song.iter().enumerate() {
        let n = n as i64;
        let added = 0.0 + at(n) + at(n - gap) + at(n - 2 * gap);
        assert_eq!(sample.to_bits(), added.to_bits(), "sample {n}");
    }
    // `(a + b) + c`: a read and two adds, a read and one add for `c`, a zero while none sounds.
    let note = u128::from(RATE / 2);
    let silent = song.len() as u128 - 3 * note;
    assert_eq!(flops::tree(&held).rows[0].own, (3 + 3 + 2) * note + silent);
}

#[test]
fn a_count_prices_the_render_it_names() {
    let g = sampled_notes();
    let held = over(&g, "song", 4.5, None);
    let asked = config(4.5).asking(vec![Ask {
        node: "song".to_string(),
        representation: Representation::Flops,
    }]);
    let counted = render(&g, "song", asked, None).expect("a count");
    let Output::Flops(tree) = answer(&counted, counted.root, Representation::Flops)
        .expect("a count")
        .value
    else {
        panic!("a count");
    };
    assert_eq!(tree.total, held.work().priced_flops);
}

/// An open range ends where its root is exactly zero from: a ramp past its foot, and a decay
/// where the engine's own `exp` underflows it.
#[test]
fn a_ramp_and_a_decay_end_where_they_are_exactly_zero() {
    let g = graph_of(
        "pruning-decay",
        &[
            ("decay", "sin(2*pi*440*t)*exp(-t/0.05)*exp(-t/0.1)\n"),
            ("ramp", "sample(max(0, 1 - t/2))\n"),
        ],
    );
    let open = |target: &str| {
        render(&g, target, RenderConfig::at(RATE).under(LATTICE_8K), None)
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let ramp = open("ramp");
    assert_eq!(ramp.range.expect("a range").end, 2 * i64::from(RATE));

    let decay = open("decay");
    let end = decay.range.expect("a range").end;
    let sum = &decay.symbolic[&decay.root];
    let step = 1.0 / f64::from(RATE);
    let at = |n: i64| sva_samples::eval_spectral_sum_at(sum, 0, n as f64 * step).expect("a value");
    let last = (0..end).rev().find(|n| !at(*n).is_zero()).expect("a sound");
    assert!(end - last < i64::from(RATE), "{last} {end}");
    assert!((end..end + i64::from(RATE)).all(|n| at(n).is_zero()));
}
