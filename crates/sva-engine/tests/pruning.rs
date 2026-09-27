// Concern: proves a render skips only work whose result is exactly zero, writing the same bits | Non-concern: what any row computes (sva-samples) | IO: (a composition, a range) -> samples, priced work

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, Range, Render, RenderConfig, flops, render};

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

fn over(g: &Graph, target: &str, secs: f64, cache: Option<&Cache>) -> Render {
    let config = RenderConfig {
        range: Range {
            start: Some(0),
            end: Some((secs * f64::from(RATE)) as i64),
        },
        ..RenderConfig::at(RATE)
    };
    render(g, target, config, cache).unwrap_or_else(|e| panic!("{target}: {e}"))
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
    let g = graph_of(
        "pruning-sampled",
        &[
            (
                "note",
                "crop(lowpass(sample(0.5*sin(2*pi*440*t)), cutoff=2000, q=0.7), 0s, 0.5s)\n",
            ),
            ("song", "@note(t) + @note(t - 2s) + @note(t - 4s)\n"),
        ],
    );
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
