// Concern: proves a released note reads the held note's run up to its release and computes only the rest, bit for bit | Non-concern: the store's cap (stores.rs) | IO: (a store, renders) -> samples, work

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Cache, CacheStats, Outcome, PayloadKind, Range, RenderConfig, Stream, StreamConfig, render,
};

const RATE: u32 = 8_000;
const LEN: i64 = RATE as i64;
const EVERY: usize = 1_024;

/// A string whose felt, lifted until `release`, ramps its dashpot in over 0.03 s.
const STRING: &str = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*crop(min(1, \
    (t - release)/0.03), release, inf))\n";

fn composition() -> Graph {
    graph_of(
        "segments",
        &[
            ("string", STRING),
            ("held", "@string(t, f0=261.63)\n"),
            (
                "released",
                "release = inf\n@string(t, f0=261.63, release=release)\n",
            ),
        ],
    )
}

fn outcomes(stats: &CacheStats) -> Vec<Outcome> {
    let runs = stats.lookups.iter().filter(|l| l.kind == PayloadKind::Run);
    runs.map(|l| l.outcome).collect()
}

fn whole(g: &Graph, target: &str, cache: Option<&Cache>) -> (Vec<f64>, Vec<Outcome>) {
    let mut g = g.clone();
    assert!(g.define("target", sva_ast::parse_expr(target).expect("a target")));
    let config = RenderConfig::seconds(RATE, LEN as f64 / f64::from(RATE));
    let held = render(&g, "target", config, cache).unwrap_or_else(|e| panic!("{e}"));
    let samples = held.output(held.root).expect("a buffer").plane(0).to_vec();
    (
        samples,
        held.cache_stats.as_ref().map(outcomes).unwrap_or_default(),
    )
}

fn streamed(g: &Graph, target: &str, cache: Option<&Cache>) -> (Vec<f64>, u128, Vec<Outcome>) {
    let config = StreamConfig {
        block: 1_024,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(LEN),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let target = sva_ast::parse_expr(target).expect("a target");
    let mut stream = Stream::open(g, &target, config, cache).unwrap_or_else(|e| panic!("{e}"));
    let mut heard = Vec::new();
    while let Some(block) = stream.next_block().unwrap_or_else(|e| panic!("{e}")) {
        heard.extend_from_slice(block.plane(0));
    }
    (heard, stream.work().priced_flops, outcomes(&stream.stats()))
}

/// Before its release a note is the held one, so after one held render each release reads the
/// held run up to it, resumes from a state at most `EVERY` samples back, and computes the
/// rest; asked again, it is whole in the store. One release is on a sample, one between.
#[test]
fn each_release_reads_the_held_run_and_computes_only_its_tail() {
    let g = composition();
    let cache = Cache::new();
    cache.set_mark_every(EVERY);
    whole(&g, "@held(t)", Some(&cache));
    for release in [0.3, 0.5123] {
        let target = format!("@released(t, release={release})");
        let (cold, cold_work, _) = streamed(&g, &target, None);
        let (heard, work, found) = streamed(&g, &target, Some(&cache));
        assert_eq!(heard, cold, "release at {release}");
        assert!(
            found.contains(&Outcome::Prefix),
            "release at {release}: {found:?}"
        );
        let tail = LEN - (release * f64::from(RATE)).ceil() as i64 + EVERY as i64;
        assert!(
            work * LEN as u128 <= cold_work * tail as u128,
            "release at {release}: {work} of {cold_work} computed, past its tail"
        );
        let (first, _) = whole(&g, &target, Some(&cache));
        assert_eq!(first, whole(&g, &target, None).0, "release at {release}");
        let (again, found) = whole(&g, &target, Some(&cache));
        assert_eq!(again, first, "release at {release}, again");
        assert!(found.iter().all(|o| *o == Outcome::Hit), "{found:?}");
    }
}
