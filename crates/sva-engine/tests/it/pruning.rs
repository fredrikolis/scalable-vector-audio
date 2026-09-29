// Concern: proves a render skips only work that is exactly zero, writing the same bits | Non-concern: what any row computes (sva-samples) | IO: (a composition, a range) -> samples, priced work

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Ask, Cache, Extent, Output, Range, Render, RenderConfig, Representation, answer, flops, render,
};

const RATE: u32 = 8_000;

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
        ..RenderConfig::at(RATE)
    }
}

/// A song reading one note three times computes the note once, over its own half second:
/// each sample is the note's own samples added from +0 in the order written, and a render
/// reaching further finds the note whole in the store.
#[test]
fn a_note_read_three_times_is_computed_once_and_added() {
    let g = notes();
    let cache = Cache::new();
    let held = over(&g, "song", 6.0, Some(&cache));
    let note = held.id("note").expect("the note");
    assert_eq!(
        held.evaluated(note),
        vec![Extent::new(0, i64::from(RATE / 2))]
    );
    let alone = over(&g, "note", 6.0, None);
    let own = alone
        .output(alone.root)
        .expect("the note")
        .plane(0)
        .to_vec();
    let at = |n: i64| {
        usize::try_from(n)
            .ok()
            .and_then(|n| own.get(n))
            .copied()
            .unwrap_or(0.0)
    };
    let samples = held.output(held.root).expect("the song").plane(0).to_vec();
    let gap = 2 * i64::from(RATE);
    for (n, sample) in samples.iter().enumerate() {
        let n = n as i64;
        let added = 0.0 + at(n) + at(n - gap) + at(n - 2 * gap);
        assert_eq!(sample.to_bits(), added.to_bits(), "sample {n}");
    }

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
    // One sum: a read and an add while each note sounds, a zero while none does.
    let note = u128::from(RATE / 2);
    let silent = song.len() as u128 - 3 * note;
    assert_eq!(flops::tree(&held).rows[0].own, 3 * 2 * note + silent);
}

/// Index reads of a closed form read its one value, as time reads do, and a long sum of them
/// pays for each term only while it sounds: the cost is linear in the terms.
#[test]
fn a_long_sum_of_index_reads_shares_one_value_and_pays_each_term_only_where_it_sounds() {
    let terms = 12;
    let song: Vec<String> = (0..terms)
        .map(|k| format!("@note[idx(t - {}s)]", 2 * k))
        .collect();
    let g = graph_of(
        "pruning-indexed",
        &[
            ("note", "crop(sin(2*pi*440*t)*exp(-t/0.2), 0s, 0.5s)\n"),
            ("song", &format!("{}\n", song.join(" + "))),
        ],
    );
    let held = over(&g, "song", 2.0 * (terms - 1) as f64 + 0.5, None);
    let note = held.id("note").expect("the note");
    assert_eq!(
        held.evaluated(note),
        vec![Extent::new(0, i64::from(RATE / 2))]
    );
    let stats = held.cache_stats.as_ref().expect("stats");
    let reads = stats.lookups.iter().filter(|l| l.node == "note").count();
    assert_eq!(
        reads, terms,
        "each read looks up the note's one value: {stats:?}"
    );
    assert_eq!(stats.hits(), terms - 1, "{stats:?}");

    let len = held.output(held.root).expect("the song").plane(0).len() as u128;
    let note = u128::from(RATE / 2);
    let sounding = terms as u128 * note;
    assert_eq!(
        flops::tree(&held).rows[0].own,
        2 * sounding + (len - sounding)
    );
}

/// A term scaled by constants is as zero as its read where that read is, so a sum of scaled
/// notes, as a grid's rows are, also pays only for the notes sounding and writes the same bits.
#[test]
fn a_scaled_sampled_term_is_read_only_where_it_is_nonzero() {
    let g = sampled_notes();
    let scaled = graph_of(
        "pruning-scaled",
        &[
            (
                "note",
                "crop(lowpass(sample(0.5*sin(2*pi*440*t)), cutoff=2000, q=0.7), 0s, 0.5s)\n",
            ),
            (
                "song",
                "@note(t)*0.5 + @note(t - 2s)*-0.75 + @note(t - 4s)*0.25\n",
            ),
        ],
    );
    let held = over(&scaled, "song", 4.5, None);
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
        let added = 0.0 + at(n) * 0.5 + at(n - gap) * -0.75 + at(n - 2 * gap) * 0.25;
        assert_eq!(sample.to_bits(), added.to_bits(), "sample {n}");
    }
    assert_eq!(
        flops::tree(&held).rows[0].own,
        flops::tree(&over(&g, "song", 4.5, None)).rows[0].own,
        "a note scaled by constants is priced only while it sounds, as an unscaled one is"
    );
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
        render(&g, target, RenderConfig::at(RATE), None).unwrap_or_else(|e| panic!("{e}"))
    };
    let ramp = open("ramp");
    assert_eq!(ramp.range.expect("a range").end, 2 * i64::from(RATE));

    let decay = open("decay");
    let end = decay.range.expect("a range").end;
    let sum = &sva_engine::spectral_sum_of(&decay.tys, decay.root, sva_engine::Var::T)
        .expect("a decay's sum");
    let step = 1.0 / f64::from(RATE);
    let at = |n: i64| sva_samples::eval_spectral_sum_at(sum, 0, n as f64 * step).expect("a value");
    let last = (0..end).rev().find(|n| !at(*n).is_zero()).expect("a sound");
    assert!(end - last < i64::from(RATE), "{last} {end}");
    assert!((end..end + i64::from(RATE)).all(|n| at(n).is_zero()));
}
