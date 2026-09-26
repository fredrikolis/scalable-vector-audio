// Concern: the fixture paths, scratch directories and reading shortcuts every suite here shares | Non-concern: what any suite asserts | IO: (name) -> a path, a Rendered's reading

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sva_cli::success_envelope;
use sva_core::{Rendered, SAMPLE_LIMIT};
use sva_core::{Report, query_data};
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

pub fn secs(r: &Rendered) -> f64 {
    r.config.horizon.end_secs
}

pub fn buffer<'a>(r: &'a Rendered, node: &str) -> &'a Buffer {
    let id = r.render.id(node).unwrap_or_else(|| panic!("{node} typed"));
    r.render
        .buffer(id)
        .unwrap_or_else(|| panic!("{node} rendered"))
}

pub fn plane<'a>(r: &'a Rendered, node: &str) -> &'a [f64] {
    buffer(r, node).plane(0)
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
    let answers = [(
        name.to_string(),
        r.answer(&r.target, representation).unwrap(),
    )];
    success_envelope(
        &query_data(&Report {
            target: &r.target,
            rate: r.config.rate,
            horizon: r.config.horizon,
            profile: r.config.profile.name,
            label: r.label(),
            written: &[],
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
            skim: false,
        }),
        &[],
    )
}
