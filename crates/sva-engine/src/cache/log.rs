// Concern: writes a render's lookups as log lines, per second of output and per node | Non-concern: counting them (stats.rs), where the lines go or when | IO: (CacheStats, rate) -> text

use std::collections::BTreeMap;
use std::fmt::Write;

use super::{CacheStats, Lookup, Tally};

fn line(tally: &Tally) -> String {
    format!(
        "hit={} miss={} prefix={} new={} reused={}",
        tally.hit, tally.miss, tally.prefix, tally.new, tally.reused
    )
}

fn percent(part: usize, whole: usize) -> f64 {
    match whole {
        0 => 0.0,
        _ => 100.0 * part as f64 / whole as f64,
    }
}

/// Read once the render ends, so each lookup is counted as it finally settled: a run is stored
/// only when its node ends. `pass` is what the render looked up before its first block.
pub fn cache_log(stats: &CacheStats, rate: u32) -> String {
    let (lookups, reached) = (&stats.lookups, &stats.reached);
    let mut out = String::new();
    let mut cuts: Vec<(String, usize)> = Vec::new();
    let mut second = None;
    for (k, &(at, made)) in reached.iter().enumerate() {
        let whole = at.div_euclid(i64::from(rate));
        match second {
            None => cuts.push(("pass".to_string(), made)),
            Some(last) if whole > last || k + 1 == reached.len() => {
                cuts.push((format!("t={:.3}s", at as f64 / f64::from(rate)), made));
            }
            Some(_) => continue,
        }
        second = Some(whole);
    }
    if cuts.last().is_none_or(|(_, made)| *made < lookups.len()) {
        cuts.push(("end".to_string(), lookups.len()));
    }
    let (mut from, mut hits, mut looked) = (0, 0, 0);
    for (label, to) in cuts {
        let tally = Tally::of(&lookups[from..to]);
        (hits, looked) = (hits + tally.hit, looked + tally.looked());
        let cum = percent(hits, looked);
        let tally = line(&tally);
        let _ = writeln!(out, "sva-cache {label} {tally} cum-hit={cum:.1}%");
        from = to;
    }
    let mut nodes: BTreeMap<&str, Vec<&Lookup>> = BTreeMap::new();
    for lookup in lookups {
        nodes.entry(&lookup.node).or_default().push(lookup);
    }
    for (node, lookups) in &nodes {
        let tally = line(&Tally::of(lookups.iter().copied()));
        let _ = writeln!(out, "sva-cache node {tally} {node}");
    }
    let total = Tally::of(lookups);
    let _ = writeln!(
        out,
        "sva-cache total {} hit-rate={:.1}% nodes={} entries={} bytes={}",
        line(&total),
        percent(total.hit, total.looked()),
        nodes.len(),
        stats.entries,
        stats.bytes
    );
    out
}
