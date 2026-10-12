// Concern: the worker threads a page starts and the limit each render computes on | Non-concern: computing in parallel (sva-engine) | IO: (n) -> workers; (asked) -> a limit

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

use sva_engine::BootPhase;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::stats::booted;

static STARTED: AtomicUsize = AtomicUsize::new(0);
/// Held while a pool starts: until it is up, a render computes on one thread.
const STARTING: usize = usize::MAX;

/// Once, from a worker of a cross-origin isolated page. The module's subscriber hears it start
/// as an `EngineBoot`.
#[wasm_bindgen(js_name = startThreads)]
pub async fn start_threads(threads: usize) -> Result<(), JsValue> {
    let first = || STARTED.compare_exchange(0, STARTING, Ordering::Relaxed, Ordering::Relaxed);
    if threads == 0 || first().is_err() {
        return Err(crate::refuse(
            format!("`startThreads({threads})` asks for no threads, or for a second pool"),
            "start one pool of one or more threads, once per module",
        ));
    }
    booted(BootPhase::StartingThreads, Some(threads), None);
    let started = started(threads).await;
    STARTED.store(if started.is_ok() { threads } else { 0 }, Ordering::Relaxed);
    let phase = match started {
        Ok(()) => BootPhase::ThreadsStarted,
        Err(_) => BootPhase::ThreadsFailed,
    };
    booted(phase, None, None);
    started
}

/// The pool's threads, once it is up.
pub(crate) fn pool() -> Option<usize> {
    match STARTED.load(Ordering::Relaxed) {
        0 | STARTING => None,
        started => Some(started),
    }
}

#[cfg(target_feature = "atomics")]
async fn started(threads: usize) -> Result<(), JsValue> {
    let pool = wasm_bindgen_rayon::init_thread_pool(threads);
    wasm_bindgen_futures::JsFuture::from(pool).await.map(|_| ())
}

#[cfg(not(target_feature = "atomics"))]
async fn started(_: usize) -> Result<(), JsValue> {
    Err(crate::refuse(
        "this build of sva-wasm computes on one thread".into(),
        "load the build made with wasm threads, or leave `threads` out",
    ))
}

pub(crate) fn limit(asked: Option<NonZeroUsize>) -> NonZeroUsize {
    let started = match STARTED.load(Ordering::Relaxed) {
        STARTING => NonZeroUsize::MIN,
        started => NonZeroUsize::new(started).unwrap_or(NonZeroUsize::MIN),
    };
    asked.map_or(started, |asked| asked.min(started))
}
