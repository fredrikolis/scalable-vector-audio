// Concern: proves a render skips only work whose result is exactly zero, writing the same bits | Non-concern: what any row computes (sva-samples) | IO: (a composition, a range) -> samples, priced work

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, Range, Render, RenderConfig, render};

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
