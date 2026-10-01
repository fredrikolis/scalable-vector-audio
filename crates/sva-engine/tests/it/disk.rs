// Concern: a disk in memory that logs reads, refuses writes on cue and holds names open as another holder would | Non-concern: what a tier keeps | IO: (name[, bytes]) -> bytes, a log

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use sva_engine::{Backend, INDEX_NAME, Store, Tier};

/// One map of names; each staging area's names sit under its own prefix, where `list` never looks.
/// While `refusing` is up, every write and rename fails. `reads` logs each read: the name and the bytes it
/// answered. A name in `open` is another holder's, so, as in OPFS, it is neither removed nor
/// moved onto.
#[derive(Clone, Default)]
pub(crate) struct Memory {
    pub(crate) held: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub(crate) prefix: String,
    pub(crate) refusing: Arc<AtomicBool>,
    pub(crate) reads: Arc<Mutex<Vec<(String, usize)>>>,
    pub(crate) lists: Arc<AtomicUsize>,
    pub(crate) locked: Arc<AtomicBool>,
    pub(crate) open: Arc<Mutex<BTreeSet<String>>>,
}

/// The fake's lock, released when dropped.
pub(crate) struct Held(Arc<AtomicBool>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Memory {
    pub(crate) fn names(&self) -> Vec<String> {
        self.held
            .lock()
            .unwrap()
            .keys()
            .filter(|n| !n.contains('/'))
            .cloned()
            .collect()
    }

    pub(crate) fn staged(&self) -> Vec<String> {
        let held = self.held.lock().unwrap();
        held.keys().filter(|n| n.contains('/')).cloned().collect()
    }

    pub(crate) fn at(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    pub(crate) fn entries(&self) -> Vec<String> {
        let names = self.names().into_iter();
        names.filter(|n| n != INDEX_NAME).collect()
    }

    pub(crate) fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.held.lock().unwrap().get(&self.at(name)).cloned()
    }

    pub(crate) fn set(&self, name: &str, bytes: Vec<u8>) {
        self.held.lock().unwrap().insert(self.at(name), bytes);
    }

    pub(crate) fn opened(&self, name: &str) -> bool {
        self.open.lock().unwrap().contains(&self.at(name))
    }
}

impl Backend for Memory {
    type Lock = Held;

    async fn lock(&self) -> Result<Held, String> {
        std::future::poll_fn(|_| {
            let free =
                self.locked
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed);
            match free {
                Ok(_) => Poll::Ready(Ok(Held(self.locked.clone()))),
                Err(_) => Poll::Pending,
            }
        })
        .await
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let found = self.bytes(name);
        let read = (self.at(name), found.as_ref().map_or(0, Vec::len));
        self.reads.lock().unwrap().push(read);
        Ok(found)
    }

    async fn get_range(&self, name: &str, from: u64, len: u64) -> Result<Option<Vec<u8>>, String> {
        let found = self.bytes(name).map(|bytes| {
            let from = (from as usize).min(bytes.len());
            let to = from.saturating_add(len as usize).min(bytes.len());
            bytes[from..to].to_vec()
        });
        let read = (self.at(name), found.as_ref().map_or(0, Vec::len));
        self.reads.lock().unwrap().push(read);
        Ok(found)
    }

    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if self.refusing.load(Ordering::Relaxed) {
            return Err("the medium refused".to_string());
        }
        self.set(name, bytes.to_vec());
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<bool, String> {
        if self.opened(name) {
            return Ok(false);
        }
        self.held.lock().unwrap().remove(&self.at(name));
        Ok(true)
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        self.lists.fetch_add(1, Ordering::Relaxed);
        let held = self.held.lock().unwrap();
        Ok(held
            .iter()
            .filter_map(|(n, b)| Some((n.strip_prefix(&self.prefix)?, b)))
            .filter(|(n, _)| !n.contains('/'))
            .map(|(n, b)| (n.to_string(), b.len() as u64))
            .collect())
    }

    /// Each store its own area, as the backend promises: none writes another's.
    async fn staging(&self) -> Result<Memory, String> {
        static AREAS: AtomicUsize = AtomicUsize::new(0);
        let area = AREAS.fetch_add(1, Ordering::Relaxed);
        Ok(Memory {
            prefix: format!("{}staging-{area}/", self.prefix),
            ..self.clone()
        })
    }

    async fn rename(&self, name: &str, to: &Memory) -> Result<bool, String> {
        if self.refusing.load(Ordering::Relaxed) {
            return Err("the medium refused".to_string());
        }
        if self.opened(name) || to.opened(name) {
            return Ok(false);
        }
        let mut held = self.held.lock().unwrap();
        let bytes = held.remove(&self.at(name)).ok_or("nothing staged")?;
        held.insert(to.at(name), bytes);
        Ok(true)
    }
}

/// The fake never waits, so one poll finishes every future it makes.
pub(crate) fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(out) => out,
        Poll::Pending => panic!("the in-memory backend never waits"),
    }
}

/// Fresh memory over a fresh store over `memory`, as a new process opens them.
pub(crate) fn opened(memory: &Memory, max_bytes: u64) -> Tier<Memory> {
    let store = now(Store::open(memory.clone(), max_bytes)).expect("the store opens");
    Tier::over(store, u64::MAX)
}

/// The disk beneath `tier`.
pub(crate) fn disk(tier: &Tier<Memory>) -> &Store<Memory> {
    tier.disk().expect("a tier over a disk")
}
