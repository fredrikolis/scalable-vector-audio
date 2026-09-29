// Concern: the fixture paths, scratch directories and reading shortcuts every suite here shares | Non-concern: what any suite asserts | IO: (name) -> a path, a Rendered's reading

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sva_cli::{CliError, success_envelope};
use sva_core::{Printed, Report, query_data};
use sva_core::{Rendered, SAMPLE_LIMIT};
use sva_engine::{Buffer, LedgerEntry, Output, Representation};

static RUN: AtomicU32 = AtomicU32::new(0);

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sva-cli-{name}-{:x}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a fresh directory");
    dir
}

pub fn put(dir: &Path, rel: &str, body: &str) {
    let full = dir.join(rel);
    std::fs::create_dir_all(full.parent().expect("a parent")).expect("a subdirectory");
    std::fs::write(full, body).expect("a node file");
}

/// `master` over its first second, as a render names it.
pub fn master(dir: &Path) -> Result<Rendered, CliError> {
    sva_core::probe(dir, "@master([0, 1s])")
}

/// Where the render's range ends, in seconds.
pub fn secs(r: &Rendered) -> f64 {
    let range = r.render.range.expect("a render that read samples");
    range.end as f64 / f64::from(r.config.rate)
}

pub fn buffer(r: &Rendered, node: &str) -> Buffer {
    let id = r.render.id(node).unwrap_or_else(|| panic!("{node} typed"));
    r.render
        .output(id)
        .unwrap_or_else(|e| panic!("{node} rendered: {e}"))
}

pub fn plane(r: &Rendered, node: &str) -> Vec<f64> {
    buffer(r, node).plane(0).to_vec()
}

pub fn ledger(r: &Rendered, node: &str) -> Vec<LedgerEntry> {
    match r
        .answer(node, Representation::Ledger { depth: 8 })
        .unwrap()
        .value
    {
        Output::Ledger(entries) => entries,
        other => panic!("expected a ledger, got {other:?}"),
    }
}

pub fn entry(r: &Rendered, node: &str) -> LedgerEntry {
    ledger(r, node)
        .into_iter()
        .find(|e| e.node == node)
        .unwrap_or_else(|| panic!("{node} is not in its own ledger"))
}

pub fn json_of(r: &Rendered, name: &str, representation: Representation) -> String {
    let answers = [Printed {
        name: name.to_string(),
        answer: r.answer(&r.target, representation).unwrap(),
        skim: false,
    }];
    let rate = r.config.rate;
    let interval = r
        .render
        .output
        .map(|x| (x.start_secs(rate), x.end as f64 / f64::from(rate)));
    let bounds = r.render.reconstructions();
    success_envelope(
        &query_data(&Report {
            target: &r.expression,
            rate,
            bits: Some(r.config.profile.precision_bits),
            interval,
            profile: r.config.profile.name,
            label: r.label(),
            written: &[],
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
            bounds: &bounds,
        }),
        &[],
    )
}

/// The readings a comma list of calls names, as `--representation` reads it.
pub fn asked(list: &str) -> Vec<sva_core::Asked> {
    sva_core::calls(list)
        .expect("calls")
        .iter()
        .map(|call| sva_core::asked(call).expect("a reading"))
        .collect()
}

pub fn doc(models: &str) -> String {
    format!(
        "; Models: {models} | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: fixture\n"
    )
}
