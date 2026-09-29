// Concern: the throwaway compositions every suite shares, and a render's samples per node | Non-concern: what any suite asserts about them | IO: (name, files) -> Graph, (Render) -> samples

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sva_ast::Graph;
use sva_engine::Render;

static RUN: AtomicU32 = AtomicU32::new(0);

pub fn dir_of(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sva-engine-{name}-{:x}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a fresh directory");
    for (rel, content) in files {
        let full = Path::new(&dir).join(rel);
        fs::create_dir_all(full.parent().expect("a parent")).expect("a subdirectory");
        fs::write(full, content).expect("a node file");
    }
    dir
}

pub fn graph_of(name: &str, files: &[(&str, &str)]) -> Graph {
    sva_ast::parse_composition(&dir_of(name, files)).expect("a composition that parses")
}

pub fn samples(r: &Render) -> BTreeMap<String, Vec<f64>> {
    r.buffers
        .iter()
        .map(|(id, b)| (r.tys.name(*id).to_string(), b.plane(0).to_vec()))
        .collect()
}
