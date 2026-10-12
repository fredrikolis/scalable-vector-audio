// Concern: tells a tier's one subscriber what each call over it is doing, working and at rest | Non-concern: counting a lookup, what a call computes | IO: (phase, samples, work, tally) -> EngineStats

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::memory::{Counters, Memory};
use super::stats::Tally;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Render,
    Stream,
    Edit,
    Persist,
}

/// Samples are delivered in `Delivering` alone; the last three end a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Parsing,
    Typing,
    LookingUp,
    Planning,
    /// Stored samples read ahead of the first block.
    Fetching,
    Delivering,
    /// Past the last sample.
    Finishing,
    Storing,
    Done,
    Failed,
    Abandoned,
}

/// Samples of values: one unit for what memory answered and what was computed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gauge {
    /// Answered by what memory or the disk held before the call began, each once a call.
    pub cached_samples: u64,
    /// Those of `cached_samples` the disk answered.
    pub stored_samples: u64,
    /// Summed over every value its pulls computed.
    pub computed_samples: u64,
}

impl Gauge {
    pub fn done(&self) -> u64 {
        self.cached_samples + self.computed_samples
    }

    pub fn plus(self, more: Gauge) -> Gauge {
        self.each(more, u64::wrapping_add)
    }

    fn minus(self, less: Gauge) -> Gauge {
        self.each(less, u64::wrapping_sub)
    }

    fn each(self, other: Gauge, by: fn(u64, u64) -> u64) -> Gauge {
        Gauge {
            cached_samples: by(self.cached_samples, other.cached_samples),
            stored_samples: by(self.stored_samples, other.stored_samples),
            computed_samples: by(self.computed_samples, other.computed_samples),
        }
    }
}

/// A call's work, and all it expects: `None` over a range with no end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub work: Gauge,
    pub planned_samples: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub instances: usize,
    /// Those this call types.
    pub nodes: usize,
    pub typed: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Worked {
    pub samples: u64,
    pub work: Gauge,
    pub lookups: Tally,
    /// Memory's disk traffic meanwhile, whichever call asked.
    pub tier: Counters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallStats {
    pub id: u64,
    pub kind: Kind,
    pub target: String,
    pub phase: Phase,
    /// `None` until its range is decided, and for an open end.
    pub total_samples: Option<u64>,
    /// What `counts.work.done()` ends at, where its range ends.
    pub planned_samples: Option<u64>,
    pub typing: Option<Typed>,
    pub counts: Worked,
}

/// A step of the engine starting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootPhase {
    StartingThreads,
    ThreadsStarted,
    ThreadsFailed,
    OpeningStore,
    ReadingIndex,
    IndexRead,
    StoreFailed,
    Opened,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreBoot {
    pub max_bytes: u64,
    /// The index file's size, its entries and their bytes.
    pub read: Option<(u64, usize, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    pub phase: BootPhase,
    pub threads: Option<usize>,
    pub store: Option<StoreBoot>,
}

impl Boot {
    pub fn under_way(&self) -> bool {
        use BootPhase::{Opened, ThreadsFailed, ThreadsStarted};
        !matches!(self.phase, ThreadsStarted | ThreadsFailed | Opened)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineStats {
    /// Renders, opens, edits and persists in flight, a stream's reads never: zero is idle.
    pub working: usize,
    /// No tier reports one.
    pub boot: Option<Boot>,
    /// `None` in the one report a subscription opens with.
    pub call: Option<CallStats>,
    pub totals: Worked,
}

pub type Hears = Arc<dyn Fn(&EngineStats) + Send + Sync>;

struct Heard {
    hears: Hears,
    since: Counters,
    done: Told,
}

#[derive(Clone, Copy, Default)]
struct Told {
    samples: u64,
    work: Gauge,
    lookups: Tally,
}

/// Unsubscribed, a call reads one flag at each point it would report.
pub(crate) struct Watch {
    memory: Memory,
    heard: Mutex<Option<Heard>>,
    subscribed: AtomicBool,
    calls: AtomicU64,
    working: AtomicUsize,
}

impl Watch {
    pub(crate) fn over(memory: Memory) -> Watch {
        Watch {
            memory,
            heard: Mutex::new(None),
            subscribed: AtomicBool::new(false),
            calls: AtomicU64::new(0),
            working: AtomicUsize::new(0),
        }
    }

    fn heard(&self) -> MutexGuard<'_, Option<Heard>> {
        self.heard.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn subscribe(&self, hears: Hears) {
        let since = self.memory.counters();
        *self.heard() = Some(Heard {
            hears,
            since,
            done: Told::default(),
        });
        self.subscribed.store(true, Ordering::Relaxed);
        self.tell(None);
    }

    pub(crate) fn unsubscribe(&self) {
        self.subscribed.store(false, Ordering::Relaxed);
        *self.heard() = None;
    }

    /// The subscriber is called with nothing held, so it may subscribe anew or call the engine.
    fn tell(&self, call: Option<(&Call, Told)>) {
        let mut heard = self.heard();
        let Some(held) = heard.as_mut() else {
            return;
        };
        let now = self.memory.counters();
        let call = call.map(|(call, more)| {
            held.done.samples = held.done.samples.wrapping_add(more.samples);
            held.done.work = held.done.work.plus(more.work);
            held.done.lookups = held.done.lookups.plus(more.lookups);
            call.stats(now)
        });
        let stats = EngineStats {
            working: self.working.load(Ordering::Relaxed),
            boot: None,
            call,
            totals: counts(held.done, now.since(held.since)),
        };
        let hears = Arc::clone(&held.hears);
        drop(heard);
        hears(&stats);
    }
}

fn counts(told: Told, tier: Counters) -> Worked {
    Worked {
        samples: told.samples,
        work: told.work,
        lookups: told.lookups,
        tier,
    }
}

pub struct Call {
    watch: Arc<Watch>,
    id: u64,
    kind: Kind,
    target: String,
    since: Counters,
    phase: Cell<Phase>,
    total_samples: Cell<Option<u64>>,
    planned_samples: Cell<Option<u64>>,
    typing: Cell<Option<Typed>>,
    marked: Cell<(u64, usize)>,
    told: Cell<Told>,
    summed: Cell<Told>,
    reported: Cell<u64>,
    occupies: Cell<bool>,
    ended: Cell<bool>,
}

impl Call {
    pub(crate) fn begin(watch: &Arc<Watch>, kind: Kind, target: &str, phase: Phase) -> Call {
        watch.working.fetch_add(1, Ordering::Relaxed);
        let call = Call {
            watch: Arc::clone(watch),
            id: watch.calls.fetch_add(1, Ordering::Relaxed) + 1,
            kind,
            target: target.to_string(),
            since: watch.memory.counters(),
            phase: Cell::new(phase),
            total_samples: Cell::new(None),
            planned_samples: Cell::new(None),
            typing: Cell::new(None),
            marked: Cell::new((0, 0)),
            told: Cell::new(Told::default()),
            summed: Cell::new(Told::default()),
            reported: Cell::new(0),
            occupies: Cell::new(true),
            ended: Cell::new(false),
        };
        call.report();
        call
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn subscribed(&self) -> bool {
        self.watch.subscribed.load(Ordering::Relaxed)
    }

    fn stats(&self, now: Counters) -> CallStats {
        CallStats {
            id: self.id,
            kind: self.kind,
            target: self.target.clone(),
            phase: self.phase.get(),
            total_samples: self.total_samples.get(),
            planned_samples: self.planned_samples.get(),
            typing: self.typing.get(),
            counts: counts(self.told.get(), now.since(self.since)),
        }
    }

    fn report(&self) {
        if !self.subscribed() {
            return;
        }
        let (told, summed) = (self.told.get(), self.summed.get());
        let more = Told {
            samples: told.samples.wrapping_sub(summed.samples),
            work: told.work.minus(summed.work),
            lookups: told.lookups.minus(summed.lookups),
        };
        self.summed.set(told);
        self.reported.set(told.samples);
        let typed = self.typing.get().map_or(0, |typing| typing.typed);
        self.marked.set((told.work.done(), typed));
        self.watch.tell(Some((self, more)));
    }

    pub fn phase(&self, phase: Phase) {
        if self.phase.replace(phase) != phase {
            self.report();
        }
    }

    pub(crate) fn looked(&self, phase: Phase, lookups: impl FnOnce() -> Tally) {
        self.phase.set(phase);
        if self.subscribed() {
            let told = self.told.get();
            self.told.set(Told {
                lookups: lookups(),
                ..told
            });
            self.report();
        }
    }

    /// Reported once a hundredth more of its nodes are typed.
    pub(crate) fn typed(&self, typing: Typed) {
        let last = self.typing.replace(Some(typing));
        if !self.subscribed() {
            return;
        }
        let step = typing.nodes.div_ceil(100).max(1);
        let due = typing.typed >= self.marked.get().1.saturating_add(step);
        if last.is_none() || due || typing.typed == typing.nodes {
            self.report();
        }
    }

    /// Reported once `every` samples passed, at the total, and where the phase or total is new.
    pub(crate) fn delivered(
        &self,
        (samples, total_samples, every): (u64, Option<u64>, u64),
        work: impl FnOnce() -> (Progress, Tally),
    ) {
        let entered = self.phase.replace(Phase::Delivering) != Phase::Delivering;
        let ranged = self.total_samples.replace(total_samples) != total_samples;
        if !self.subscribed() {
            return;
        }
        let due = samples >= self.reported.get().saturating_add(every);
        if entered || ranged || due || Some(samples) == total_samples {
            let (progress, lookups) = work();
            self.planned_samples.set(progress.planned_samples);
            self.told.set(Told {
                samples,
                work: progress.work,
                lookups,
            });
            self.report();
        }
    }

    /// Inside a pull: reported once a hundredth more of the work planned is done.
    pub(crate) fn worked(&self, progress: Progress) {
        let Some(planned) = progress.planned_samples.filter(|_| self.subscribed()) else {
            return;
        };
        let step = planned.div_ceil(100).max(1);
        if progress.work.done() < self.marked.get().0.saturating_add(step) {
            return;
        }
        self.planned_samples.set(Some(planned));
        self.told.set(Told {
            work: progress.work,
            ..self.told.get()
        });
        self.report();
    }

    /// A root memory answered, read by none: all it holds is the call's work.
    pub(crate) fn answered(&self, cached_samples: u64, from_store: bool, lookups: Tally) {
        if !self.subscribed() {
            return;
        }
        let work = Gauge {
            cached_samples,
            stored_samples: if from_store { cached_samples } else { 0 },
            computed_samples: 0,
        };
        self.planned_samples.set(Some(cached_samples));
        self.told.set(Told {
            work,
            lookups,
            ..self.told.get()
        });
        self.report();
    }

    /// A stream once open: its reads occupy nothing, and let go from here on it is done.
    pub(crate) fn rest(&self) {
        if self.occupies.replace(false) {
            self.watch.working.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn end(&self, phase: Phase) {
        if self.ended.replace(true) {
            return;
        }
        if self.occupies.replace(false) {
            self.watch.working.fetch_sub(1, Ordering::Relaxed);
        }
        self.phase.set(phase);
        self.report();
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.end(match self.occupies.get() {
            true => Phase::Failed,
            false => Phase::Done,
        });
    }
}
