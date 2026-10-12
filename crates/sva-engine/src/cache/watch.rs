// Concern: tells a tier's one subscriber what each call over it is doing, working and at rest | Non-concern: counting a lookup, what a call computes | IO: (phase, samples, tally) -> EngineStats

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Worked {
    pub samples: u64,
    /// Summed over every value its pulls computed.
    pub computed_samples: u64,
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
    pub counts: Worked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineStats {
    /// Renders, opens, edits and persists in flight, a stream's reads never: zero is idle.
    pub working: usize,
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
    computed_samples: u64,
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
            held.done.computed_samples =
                (held.done.computed_samples).wrapping_add(more.computed_samples);
            held.done.lookups = held.done.lookups.plus(more.lookups);
            call.stats(now)
        });
        let stats = EngineStats {
            working: self.working.load(Ordering::Relaxed),
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
        computed_samples: told.computed_samples,
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

    fn subscribed(&self) -> bool {
        self.watch.subscribed.load(Ordering::Relaxed)
    }

    fn stats(&self, now: Counters) -> CallStats {
        CallStats {
            id: self.id,
            kind: self.kind,
            target: self.target.clone(),
            phase: self.phase.get(),
            total_samples: self.total_samples.get(),
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
            computed_samples: (told.computed_samples).wrapping_sub(summed.computed_samples),
            lookups: told.lookups.minus(summed.lookups),
        };
        self.summed.set(told);
        self.reported.set(told.samples);
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

    /// Reported once `every` samples passed, at the total, and where the phase or total is new.
    pub(crate) fn delivered(
        &self,
        (samples, total_samples, every): (u64, Option<u64>, u64),
        work: impl FnOnce() -> (u64, Tally),
    ) {
        let entered = self.phase.replace(Phase::Delivering) != Phase::Delivering;
        let ranged = self.total_samples.replace(total_samples) != total_samples;
        if !self.subscribed() {
            return;
        }
        let due = samples >= self.reported.get().saturating_add(every);
        if entered || ranged || due || Some(samples) == total_samples {
            let (computed_samples, lookups) = work();
            self.told.set(Told {
                samples,
                computed_samples,
                lookups,
            });
            self.report();
        }
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
