// Concern: the engine-stats hook: the report's JS shape, who hears each composition, the boot | Non-concern: what a call counts (sva-engine), the calls (lib.rs) | IO: (callback) -> EngineStats reports

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use sva_core::{EngineStats, engine_stats_json};
use sva_engine::{Boot, BootPhase, Opening, StoreBoot, Tier, Worked};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{logged, opfs, parse, threads};

#[wasm_bindgen(typescript_custom_section)]
const ENGINE_STATS: &str = r#"
/** What one call did so far, or every call of one composition since its subscriber took it. */
export interface EngineWork {
    /** Samples of the target delivered. */
    samples: number;
    /**
     * The work scale, in samples of values: `cached_samples + computed_samples` is the work
     * done, and `cached_samples / (cached_samples + computed_samples)` the share of it that was
     * cached. A value is the target or anything it reads; a sample of one counts once per call.
     *
     * `cached_samples`: samples answered by what memory or the store held before the call
     * began, counted as the call reads them. Nothing the call itself computed counts, however
     * often it reads it back. A cached value counts its own samples only: the work that once
     * computed it and everything beneath it is not known, so a target read back whole counts
     * the target's samples and nothing else. They are counted only while a subscriber
     * listens: a call under way when its subscriber arrives counts them from then on.
     */
    cached_samples: number;
    /** The part of `cached_samples` whose value the store answered for this call. */
    cached_from_store_samples: number;
    /** Samples computed, summed over every value: 0 when all was read back. */
    computed_samples: number;
    /**
     * Lookups by what each came to, the counts `Rendering.stats()` ends at: unweighted, and
     * `reused` counts a call's reads of what it computed itself, so no share of work is read
     * off them. Cached: `hit` (`from_memory` + `from_store`). Uncached: `miss`, and `prefix`
     * (found only in part). `new`: misses kept for the next call. A count may fall as a lookup
     * settles.
     */
    lookups: {
        hit: number; from_memory: number; from_store: number;
        prefix: number; miss: number; new: number; reused: number;
    };
    /** Memory's disk traffic meanwhile, whichever call asked: `Rendering.stats().tier`. */
    tier: {
        disk_lookups: number; disk_reads: number; disk_read_bytes: number; promotions: number;
        writebacks: number; hits: number; evictions: number;
        probation_evictions: number; protected_evictions: number;
    };
}

/** One call: a render, a stream from its open on, one stream edit, or a persist. */
export interface EngineCall extends EngineWork {
    /** Counted from 1 over the composition, never given twice. */
    id: number;
    kind: "render" | "stream" | "edit" | "persist";
    /** As the page wrote it; a persist's is empty. */
    target: string;
    /**
     * Samples are delivered in `delivering` alone. `done`, `failed` and `abandoned` end the call:
     * its last report.
     */
    phase: "parsing" | "typing" | "looking_up" | "planning" | "fetching" | "delivering"
        | "finishing" | "storing" | "done" | "failed" | "abandoned";
    /**
     * What `samples` counts up to. null until the range is decided (before `delivering`), and
     * for a stream whose end is open.
     */
    total_samples: number | null;
    /**
     * What `cached_samples + computed_samples` counts up to: the call's whole work, so
     * `(cached_samples + computed_samples) / planned_samples` is its progress, moving while a
     * long pull delivers nothing. It is what the engine now plans, never an estimate: every
     * sample its values still have to read back or compute over the rest of the range. The
     * plan is corrected, by about a hundredth at most where measured, when the first block is
     * pulled: only then is it known which values memory answers on their first read, and what
     * the run-up before the first block left held. It is corrected again where memory lets go
     * of a value meanwhile, and where `until` ends the call early. The work done never falls,
     * and a call that ends `done` ends with the two equal. null until the call's values are built
     * (before `delivering`), for a persist, and for a stream whose end is open: there the
     * engine knows no total, and the counts stand alone.
     */
    planned_samples: number | null;
    /**
     * A render's typing, `typed` of `nodes`: null for a stream, an edit and a persist, which
     * report the phase alone, and null in a render until the instances the target reaches are
     * scanned: the engine finds them as it scans, so that stretch has no total and reports no
     * count. `instances`: how many it reaches. `nodes`: how many this call types, 0 where the
     * composition's last call typed them all. `typed`: how many are typed so far, ending at
     * `nodes`. What follows in `typing` (naming each node by what it computes, and bounding
     * the target) reports no count either.
     */
    typing: { instances: number; nodes: number; typed: number } | null;
}

/**
 * The engine starting, heard by the module's `subscribe` alone. `startThreads` reports
 * `starting_threads`, then `threads_started` or `threads_failed`. `Composition.open` reports
 * `opening_store` (waiting for the store's lock), `reading_index`, `index_read`, then
 * `opened`; `store_failed` then `opened` where the store would not open and memory runs
 * alone; `opened` alone where it was handed no directory. `new Composition()` reports none.
 * Fetching and compiling the wasm module happen before any engine code runs: they are the
 * page's to report.
 */
export interface EngineBoot {
    phase: "starting_threads" | "threads_started" | "threads_failed"
        | "opening_store" | "reading_index" | "index_read" | "store_failed" | "opened";
    /**
     * The threads asked for in `starting_threads`, else the pool's: null where none started.
     * The pool starts as one step: no count of threads up so far exists.
     */
    threads: number | null;
    /** null where no store is opened: in `store_failed`, how far its opening got. */
    store: {
        /** The store's budget. */
        max_bytes: number;
        /** The index file's size; null until `index_read`. */
        index_bytes: number | null;
        /** The entries the index names, and their bytes; null until `index_read`. */
        entries: number | null;
        bytes: number | null;
    } | null;
}

/**
 * One report of the module's `subscribe` or of `Composition.subscribe`. A call reports when
 * it begins, at each new phase, while `typing` once a hundredth more of its nodes are typed,
 * per fetch from the store while `fetching`, while `delivering` at block boundaries no more
 * than ten times per second of samples delivered and once a hundredth more of its planned work
 * is done, and when it ends. The callback runs inside the call, on the thread that made it,
 * never on a render thread: a worker forwards each report with `postMessage`. A page on the
 * engine's own thread hears every report as it is made, but paints nothing until the call
 * yields to the event loop, which a render does between blocks only when given a `signal` or
 * reading a store. What the callback throws is logged, and stops nothing. The hook only
 * observes: samples and stored bytes are the same subscribed or not.
 */
export interface EngineStats {
    /**
     * "working" while a render, a stream's open, an edit or a persist is in flight. A stream's
     * reads never count: what they report says "idle" unless such a call is in flight beside it.
     * "booting" in a boot report whose step is still under way.
     */
    state: "working" | "idle" | "booting";
    /** How many such calls are in flight in the composition reporting; 0 in a boot report. */
    working: number;
    /** The boot step this report is about; null in every report of a composition. */
    boot: EngineBoot | null;
    /**
     * The call this report is about; null in a boot report and in the one report a
     * composition answers its new subscriber with.
     */
    call: EngineCall | null;
    /** Every call's work in the composition reporting since its subscriber took it. */
    totals: EngineWork;
}

/**
 * One subscription for the module: `callback`, in place of the last one, hears the boot
 * (`startThreads`, each `Composition.open`) and every composition, open now or made later,
 * that has no subscriber of its own, each as `Composition.subscribe` reports it. Subscribe
 * before `startThreads` and `open` to hear them. At teardown, `unsubscribe`, or terminate the
 * worker: either leaves nothing behind.
 */
export function subscribe(callback: (stats: EngineStats) => void): void;
/** The module's subscriber hears no more; a composition's own is left as it is. */
export function unsubscribe(): void;
"#;

thread_local! {
    /// Each subscribed page function on this thread: the engine's subscriber crosses threads, a
    /// JS function cannot.
    static HEARD: RefCell<Vec<(u32, js_sys::Function)>> = const { RefCell::new(Vec::new()) };
    static SUBSCRIPTIONS: Cell<u32> = const { Cell::new(0) };
    static MODULE: Subscription = const { Subscription(Cell::new(None)) };
    static OPEN: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
}

/// A composition's tier and its own subscription, for the module's subscriber to reach.
type Open = (Weak<Tier<opfs::Opfs>>, Weak<Subscription>);

/// One subscriber's place in `HEARD`, emptied with it.
#[derive(Default)]
pub(crate) struct Subscription(Cell<Option<u32>>);

impl Subscription {
    fn hold(&self, callback: Option<js_sys::Function>) -> Option<u32> {
        HEARD.with(|heard| {
            let mut heard = heard.borrow_mut();
            if let Some(last) = self.0.take() {
                heard.retain(|(id, _)| *id != last);
            }
            let id = SUBSCRIPTIONS.replace(SUBSCRIPTIONS.get() + 1);
            heard.push((id, callback?));
            self.0.set(Some(id));
            Some(id)
        })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hold(None);
    }
}

fn callback(id: u32) -> Option<js_sys::Function> {
    HEARD.with(|heard| {
        let heard = heard.borrow();
        let held = heard.iter().find(|(held, _)| *held == id);
        held.map(|(_, callback)| callback.clone())
    })
}

/// A report handed to the page function held as `id`; what it throws is logged, and stops nothing.
fn told(id: u32, report: impl FnOnce() -> JsValue) {
    let Some(callback) = callback(id) else {
        return;
    };
    if let Err(thrown) = callback.call1(&JsValue::NULL, &report()) {
        logged(&format!("the engine-stats subscriber threw: {thrown:?}"));
    }
}

fn report(stats: &EngineStats) -> JsValue {
    parse(&engine_stats_json(stats)).expect("a report is JSON")
}

/// `tier` reporting to the subscriber held as `id`, or to none.
fn reporting(tier: &Tier<opfs::Opfs>, id: Option<u32>) {
    match id {
        Some(id) => tier.subscribe(move |stats| told(id, || report(stats))),
        None => tier.unsubscribe(),
    }
}

/// A composition made: the module's subscriber hears it from now on.
pub(crate) fn joined(tier: &Rc<Tier<opfs::Opfs>>, own: &Rc<Subscription>) {
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        open.retain(|(tier, _)| tier.strong_count() > 0);
        open.push((Rc::downgrade(tier), Rc::downgrade(own)));
    });
    if let Some(id) = MODULE.with(|module| module.0.get()) {
        reporting(tier, Some(id));
    }
}

/// `tier` heard by `callback` as its composition's own subscriber; with none, by the module's.
pub(crate) fn heard_by(
    tier: &Tier<opfs::Opfs>,
    own: &Subscription,
    callback: Option<js_sys::Function>,
) {
    let id = own.hold(callback);
    reporting(tier, id.or_else(|| MODULE.with(|module| module.0.get())));
}

/// Every composition with no subscriber of its own, reporting to the module's or to none.
fn module_heard(id: Option<u32>) {
    let open: Vec<_> = OPEN.with(|open| open.borrow().clone());
    for (tier, own) in open {
        if let (Some(tier), Some(own)) = (tier.upgrade(), own.upgrade())
            && own.0.get().is_none()
        {
            reporting(&tier, id);
        }
    }
}

#[wasm_bindgen(skip_typescript)]
pub fn subscribe(callback: js_sys::Function) {
    let id = MODULE.with(|module| module.hold(Some(callback)));
    module_heard(id);
}

#[wasm_bindgen(skip_typescript)]
pub fn unsubscribe() {
    MODULE.with(|module| module.hold(None));
    module_heard(None);
}

/// The store as `step` of its opening leaves it, and the boot phase that step begins.
pub(crate) fn stepped(store: StoreBoot, step: Opening) -> (BootPhase, StoreBoot) {
    match step {
        Opening::Locking => (BootPhase::OpeningStore, store),
        Opening::ReadingIndex => (BootPhase::ReadingIndex, store),
        Opening::IndexRead {
            index_bytes,
            entries,
            bytes,
        } => {
            let read = Some((index_bytes, entries, bytes));
            (BootPhase::IndexRead, StoreBoot { read, ..store })
        }
    }
}

/// A boot step told to the module's subscriber, where there is one: `asked` the threads a
/// pool starts with, the pool's own otherwise.
pub(crate) fn booted(phase: BootPhase, asked: Option<usize>, store: Option<StoreBoot>) {
    let Some(id) = MODULE.with(|module| module.0.get()) else {
        return;
    };
    let boot = Boot {
        phase,
        threads: asked.or_else(threads::pool),
        store,
    };
    let stats = EngineStats {
        working: 0,
        boot: Some(boot),
        call: None,
        totals: Worked::default(),
    };
    told(id, || report(&stats));
}
