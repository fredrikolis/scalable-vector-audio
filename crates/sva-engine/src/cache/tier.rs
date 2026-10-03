// Concern: memory over a disk or over nothing, and the one passage between them | Non-concern: what memory keeps, the disk's medium | IO: (keys, needs) -> answers, samples; persist() -> commits

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sva_formula::Hash;
use sva_samples::{Buffer, Extent};

use super::memory::{Counters, DEFAULT_CACHE_BYTES, Known, Memory};
use super::persist::{Backend, Persisted, Store};

/// No disk: memory over nothing never waits.
pub enum Nothing {}

impl Backend for Nothing {
    type Lock = ();

    async fn lock(&self) -> Result<(), String> {
        match *self {}
    }

    async fn get(&self, _: &str) -> Result<Option<Vec<u8>>, String> {
        match *self {}
    }

    async fn get_range(&self, _: &str, _: u64, _: u64) -> Result<Option<Vec<u8>>, String> {
        match *self {}
    }

    async fn put(&self, _: &str, _: &[u8]) -> Result<(), String> {
        match *self {}
    }

    async fn delete(&self, _: &str) -> Result<bool, String> {
        match *self {}
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        match *self {}
    }

    async fn staging(&self) -> Result<Nothing, String> {
        match *self {}
    }

    async fn rename(&self, _: &str, _: &Nothing) -> Result<bool, String> {
        match *self {}
    }
}

/// The most disk reads one fetch makes.
pub const FETCH_READS: usize = 4;

/// What one fetch handed out, and the needs its reads did not reach.
#[derive(Default)]
pub(crate) struct Fetched {
    pub(crate) handed: Vec<(Hash, Vec<Arc<Buffer>>)>,
    pub(crate) left: Vec<(Hash, Extent)>,
}

/// Memory over a disk, or over nothing. Evaluation talks to memory alone; only memory decides
/// whether a lookup, a read or a write passes through to the disk.
pub struct Tier<B = Nothing> {
    memory: Memory,
    disk: Option<Store<B>>,
    /// Memory's clock where the disk last heard of its reads.
    reported: AtomicU64,
}

impl Default for Tier {
    fn default() -> Tier {
        Tier::new(DEFAULT_CACHE_BYTES)
    }
}

impl Tier {
    pub fn new(max_bytes: u64) -> Tier {
        Tier::alone(max_bytes)
    }
}

impl<B: Backend> Tier<B> {
    pub fn alone(max_bytes: u64) -> Tier<B> {
        Tier {
            memory: Memory::holding(max_bytes),
            disk: None,
            reported: AtomicU64::new(0),
        }
    }

    pub fn over(disk: Store<B>, max_bytes: u64) -> Tier<B> {
        Tier {
            memory: Memory::over_disk(max_bytes),
            disk: Some(disk),
            reported: AtomicU64::new(0),
        }
    }

    pub fn disk(&self) -> Option<&Store<B>> {
        self.disk.as_ref()
    }

    pub(crate) fn memory(&self) -> &Memory {
        &self.memory
    }

    pub(crate) fn begin(&self) -> u64 {
        self.memory.begin()
    }

    /// `key` answered: from memory, else off the disk and resident from then on.
    pub(crate) async fn lookup(&self, key: Hash, round: u64) {
        if !matches!(self.memory.answer(key, round), Known::Unknown) {
            return;
        }
        let found = match &self.disk {
            Some(disk) => {
                self.memory.count(|c| c.disk_lookups += 1);
                disk.lookup(key).await
            }
            None => None,
        };
        match found {
            Some(head) => self.memory.promote(head),
            None => self.memory.miss(key, round),
        }
    }

    /// What each need asks of a node memory holds, and off the disk what memory lacks, at most
    /// `FETCH_READS` reads a call, the rest left for the next. Memory first writes back.
    pub(crate) async fn fetch(&self, needs: &[(Hash, Extent)]) -> Fetched {
        self.write_back().await;
        let mut out = Fetched::default();
        let mut reads = 0;
        for (key, over) in needs {
            let (mut parts, lacks) = self.memory.resident(*key, *over);
            if let (Some(disk), Some((head, gap))) = (&self.disk, lacks) {
                if reads == FETCH_READS {
                    out.left.push((*key, *over));
                    continue;
                }
                reads += 1;
                let read = disk.read(&head, gap).await;
                let samples = read.iter().flatten().map(|b| b.len() * b.width);
                let bytes = (samples.sum::<usize>() * size_of::<f64>()) as u64;
                self.memory.count(|c| {
                    c.disk_reads += 1;
                    c.disk_read_bytes += bytes;
                });
                match read {
                    Some(read) => parts.extend(self.memory.promote_samples(*key, read)),
                    None => self.memory.forget(*key),
                }
            }
            if !parts.is_empty() {
                out.handed.push((*key, parts));
            }
        }
        out
    }

    /// What memory let go of while not yet on the disk, written there; a failure stops every
    /// write until the next persist.
    async fn write_back(&self) {
        if let Some(disk) = &self.disk {
            let pending = self.memory.pending();
            if let Err((why, left)) = self.written(disk, pending).await {
                self.memory.failed(why, left);
            }
        }
    }

    async fn written(
        &self,
        disk: &Store<B>,
        mut pending: Vec<super::memory::Writeback>,
    ) -> Result<(), (String, Vec<super::memory::Writeback>)> {
        pending.reverse();
        while let Some(back) = pending.pop() {
            let mut staged = Ok(());
            for part in &back.parts {
                staged = disk.stage(back.key, part).await;
                if staged.is_err() {
                    break;
                }
            }
            let staged = match staged {
                Ok(()) => disk.stage_meta(back.key, &back.head).await,
                Err(why) => Err(why),
            };
            if let Err(why) = staged {
                pending.push(back);
                pending.reverse();
                return Err((why, pending));
            }
            self.memory.written();
        }
        Ok(())
    }

    /// Every node memory holds that the disk lacks, committed; each it answered since, used.
    pub async fn persist(&self) -> Result<Persisted, String> {
        let Some(disk) = &self.disk else {
            return Ok(Persisted::default());
        };
        let (read, now) = self
            .memory
            .read_since(self.reported.load(Ordering::Relaxed));
        read.into_iter().for_each(|key| disk.used(key));
        self.reported.fetch_max(now, Ordering::Relaxed);
        self.memory.flush();
        let pending = self.memory.pending();
        if let Err((why, left)) = self.written(disk, pending).await {
            self.memory.failed(why.clone(), left);
            return Err(why);
        }
        let done = disk.persist().await?;
        self.memory.committed();
        Ok(done)
    }

    pub fn counters(&self) -> Counters {
        self.memory.counters()
    }

    pub fn max_bytes(&self) -> u64 {
        self.memory.max_bytes()
    }

    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.memory.set_max_bytes(max_bytes);
    }

    pub fn bytes(&self) -> u64 {
        self.memory.bytes()
    }

    pub fn entries(&self) -> usize {
        self.memory.entries()
    }

    pub fn evictions(&self) -> u64 {
        self.memory.counters().evictions()
    }

    pub fn holds(&self, key: Hash) -> bool {
        self.memory.holds(key)
    }

    pub fn mark_every(&self) -> usize {
        self.memory.mark_every()
    }

    pub fn set_mark_every(&self, samples: usize) {
        self.memory.set_mark_every(samples);
    }
}

/// A future over memory alone, finished in its one poll.
pub(crate) fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
    {
        std::task::Poll::Ready(out) => out,
        std::task::Poll::Pending => unreachable!("memory over nothing never waits"),
    }
}
