// Concern: the throwaway compositions every suite shares, and a render's samples per node | Non-concern: what any suite asserts about them | IO: (name, files) -> Graph, (Render) -> samples

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use std::cell::RefCell;

use sva_ast::{Expr, Graph};
use sva_engine::{Change, Changed, EngineError, Handle, Render, Stream, Through, change};

static RUN: AtomicU32 = AtomicU32::new(0);

/// A future with nothing to wait on, as a stream over no store: one poll finishes it.
pub trait Now: Future + Sized {
    fn now(self) -> Self::Output {
        let mut future = std::pin::pin!(self);
        let waker = std::task::Waker::noop();
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(waker))
        {
            std::task::Poll::Ready(out) => out,
            std::task::Poll::Pending => panic!("a future over no store never waits"),
        }
    }
}

impl<F: Future> Now for F {}

/// Each stream edit a suite makes, through `change` as a page's would.
pub async fn added(
    stream: &RefCell<Stream>,
    graph: &Graph,
    term: &Expr,
    store: &impl Through,
) -> Result<Handle, EngineError> {
    let build = |_: &Stream| Ok::<_, EngineError>(Change::Add(graph.clone(), term.clone()));
    match change(stream, build, store).await? {
        Changed::Added(handle) => Ok(handle),
        other => panic!("an add answered {other:?}"),
    }
}

pub async fn replaced(
    stream: &RefCell<Stream>,
    graph: &Graph,
    (handle, term): (Handle, &Expr),
    store: &impl Through,
) -> Result<bool, EngineError> {
    let build = |_: &Stream| Ok(Change::Replace(handle, graph.clone(), term.clone()));
    Ok(change(stream, build, store).await? == Changed::Held(true))
}

pub async fn removed(
    stream: &RefCell<Stream>,
    handle: Handle,
    store: &impl Through,
) -> Result<bool, EngineError> {
    let build = |_: &Stream| Ok(Change::Remove(handle));
    Ok(change(stream, build, store).await? == Changed::Held(true))
}

pub async fn edited(
    stream: &RefCell<Stream>,
    graph: &Graph,
    target: &Expr,
    store: &impl Through,
) -> Result<(), EngineError> {
    let build = |_: &Stream| Ok(Change::Target(graph.clone(), target.clone()));
    change(stream, build, store).await.map(|_| ())
}

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
