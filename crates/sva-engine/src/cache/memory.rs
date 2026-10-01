// Concern: the memory tier: every resident value and node under one cap, what it evicts and writes back, the misses it keeps | Non-concern: the disk | IO: (key) -> held, answered; offers -> kept

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use sva_formula::Hash;
use sva_samples::{Buffer, Extent, Label};

use super::stored::{Header, Samples};
use super::{Entry, Expected, Payload, Stored, joined};

pub const DEFAULT_CACHE_BYTES: u64 = 2 << 30;

/// Samples between two states a run keeps, so a reader resumes from one at most this far back.
pub const DEFAULT_MARK_EVERY: usize = 16_384;

/// Which computed values memory keeps besides those a render offers as nodes; the rest are
/// computed again when asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CachePolicy {
    #[default]
    All,
    /// The values two or more nodes read, and the render target.
    Forks,
    Target,
}

impl CachePolicy {
    pub const ALL: [CachePolicy; 3] = [CachePolicy::All, CachePolicy::Forks, CachePolicy::Target];

    pub fn name(self) -> &'static str {
        match self {
            CachePolicy::All => "all",
            CachePolicy::Forks => "forks",
            CachePolicy::Target => "target",
        }
    }

    pub fn named(name: &str) -> Option<CachePolicy> {
        CachePolicy::ALL.into_iter().find(|p| p.name() == name)
    }

    fn keeps(self, fork: bool, target: bool) -> bool {
        match self {
            CachePolicy::All => true,
            CachePolicy::Forks => fork || target,
            CachePolicy::Target => target,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrunePolicy {
    /// Every entry the newest render neither stored nor read.
    #[default]
    Oldest,
    /// Every entry whose node fewer than two nodes read.
    Forks,
}

impl PrunePolicy {
    pub const ALL: [PrunePolicy; 2] = [PrunePolicy::Oldest, PrunePolicy::Forks];

    pub fn name(self) -> &'static str {
        match self {
            PrunePolicy::Oldest => "oldest",
            PrunePolicy::Forks => "forks",
        }
    }

    pub fn named(name: &str) -> Option<PrunePolicy> {
        PrunePolicy::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// What passed between memory and the disk beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub disk_lookups: u64,
    pub disk_reads: u64,
    pub disk_read_bytes: u64,
    /// Disk answers made resident: a header looked up, or samples read.
    pub promotions: u64,
    pub writebacks: u64,
    pub evictions: u64,
}

impl Counters {
    pub fn since(self, then: Counters) -> Counters {
        Counters {
            disk_lookups: self.disk_lookups - then.disk_lookups,
            disk_reads: self.disk_reads - then.disk_reads,
            disk_read_bytes: self.disk_read_bytes - then.disk_read_bytes,
            promotions: self.promotions - then.promotions,
            writebacks: self.writebacks - then.writebacks,
            evictions: self.evictions - then.evictions,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub tree: u64,
    pub fork: bool,
    /// A volatile node's value replaces the last one stored under the same slot.
    pub slot: Option<Hash>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kept {
    Held,
    Replaced,
    Refused,
}

/// What memory answers a node's key with: a miss holds for the round it was met in and later
/// ones begun before it, so another holder's commit is found once a new round begins.
pub(crate) enum Known {
    Hit(Arc<Stored>),
    Miss,
    Unknown,
}

/// Where an offered node's sample `n` is: sample `n + by` of the values memory holds, a value's
/// segments or a run's, each under its key from its start on, or of another node.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Offered {
    Values {
        keys: Vec<(i64, Hash)>,
        by: i64,
    },
    Moves {
        of: Hash,
        by: i64,
    },
    /// Samples no value holds, the node's own.
    Held(Vec<Arc<Buffer>>),
}

pub(crate) struct Writeback {
    pub(crate) key: Hash,
    pub(crate) head: Header,
    pub(crate) parts: Vec<Arc<Buffer>>,
}

enum Item {
    Value {
        payload: Payload,
        label: Option<Label>,
        slot: Option<Hash>,
    },
    Node {
        source: Source,
        dirty: bool,
        slot: Option<Hash>,
    },
}

enum Source {
    Disk {
        head: Box<Header>,
        chunks: Vec<Arc<Buffer>>,
    },
    Offered {
        stored: Box<Stored>,
        offered: Offered,
    },
}

impl Source {
    fn stored(&self) -> &Stored {
        match self {
            Source::Disk { head, .. } => head.stored(),
            Source::Offered { stored, .. } => stored,
        }
    }
}

struct Held {
    item: Item,
    read: u64,
    tree: u64,
    fork: bool,
}

fn planes(b: &Buffer) -> u64 {
    (b.len() * b.width * size_of::<f64>()) as u64
}

impl Held {
    fn bytes(&self) -> u64 {
        match &self.item {
            Item::Value { payload, .. } => payload.bytes() as u64,
            Item::Node {
                source:
                    Source::Disk { chunks, .. }
                    | Source::Offered {
                        offered: Offered::Held(chunks),
                        ..
                    },
                ..
            } => chunks.iter().map(|c| planes(c)).sum(),
            Item::Node { .. } => 0,
        }
    }

    fn slot(&self) -> Option<Hash> {
        match &self.item {
            Item::Value { slot, .. } | Item::Node { slot, .. } => *slot,
        }
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<Hash, Held>,
    slots: HashMap<Hash, Hash>,
    misses: HashMap<Hash, u64>,
    pending: Vec<Writeback>,
    bytes: u64,
    max_bytes: u64,
    policy: CachePolicy,
    prune: PrunePolicy,
    clock: u64,
    tree: u64,
    round: u64,
    mark_every: usize,
    counters: Counters,
    disk: bool,
    /// Writes that failed, and why the latest did: none is tried again until a persist.
    failed: (u64, Option<String>),
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn stands(&self, key: Hash) -> bool {
        let doomed = self.doomed(key);
        doomed.iter().skip(1).any(|at| {
            matches!(
                self.entries.get(at),
                Some(Held {
                    item: Item::Node { dirty: true, .. },
                    ..
                })
            )
        })
    }

    /// The values and nodes reading `key`'s samples, and theirs, `key` first.
    fn doomed(&self, key: Hash) -> Vec<Hash> {
        let mut out = vec![key];
        let mut k = 0;
        while k < out.len() {
            let gone = out[k];
            for (at, held) in &self.entries {
                let Item::Node {
                    source: Source::Offered { offered, .. },
                    ..
                } = &held.item
                else {
                    continue;
                };
                let reads = match offered {
                    Offered::Values { keys, .. } => keys.iter().any(|(_, key)| *key == gone),
                    Offered::Moves { of, .. } => *of == gone,
                    Offered::Held(_) => false,
                };
                if reads && !out.contains(at) {
                    out.push(*at);
                }
            }
            k += 1;
        }
        out
    }

    /// `key` and everything reading its samples gone, each node not yet on the disk sent
    /// there first.
    fn remove(&mut self, key: Hash) -> bool {
        if !self.entries.contains_key(&key) {
            return false;
        }
        let doomed = self.doomed(key);
        for at in &doomed {
            self.flush(*at);
        }
        for at in doomed {
            self.discard(at);
        }
        true
    }

    fn discard(&mut self, key: Hash) {
        let Some(gone) = self.entries.remove(&key) else {
            return;
        };
        self.bytes -= gone.bytes();
        let value = matches!(gone.item, Item::Value { .. });
        if let Some(slot) = gone.slot().filter(|_| value)
            && self.slots.get(&slot) == Some(&key)
        {
            self.slots.remove(&slot);
        }
    }

    fn flush(&mut self, key: Hash) {
        let dirty = matches!(
            self.entries.get(&key),
            Some(Held {
                item: Item::Node { dirty: true, .. },
                ..
            })
        );
        if !dirty {
            return;
        }
        if let Some(back) = self.written(key) {
            self.pending.push(back);
        }
        if let Some(Held {
            item: Item::Node { dirty, .. },
            ..
        }) = self.entries.get_mut(&key)
        {
            *dirty = false;
        }
    }

    fn written(&self, key: Hash) -> Option<Writeback> {
        let Item::Node {
            source: Source::Offered { stored, offered },
            ..
        } = &self.entries.get(&key)?.item
        else {
            return None;
        };
        let head = |samples| Header::new((**stored).clone(), samples);
        Some(match offered {
            Offered::Moves { of, by } => {
                let (of, by) = self.referred(*of, *by)?;
                Writeback {
                    key,
                    head: head(Samples::Of { key: of, by }),
                    parts: Vec::new(),
                }
            }
            _ => Writeback {
                key,
                head: head(Samples::None),
                parts: self.offered_parts(offered, Extent::EVERYWHERE)?,
            },
        })
    }

    fn referred(&self, of: Hash, by: i64) -> Option<(Hash, i64)> {
        match &self.entries.get(&of)?.item {
            Item::Node {
                source: Source::Disk { head, .. },
                ..
            } => match head.samples() {
                Samples::Entry { file, shift, .. } => Some((*file, by - shift)),
                _ => None,
            },
            Item::Node {
                source:
                    Source::Offered {
                        offered: Offered::Moves { of, by: more },
                        ..
                    },
                ..
            } => self.referred(*of, by + more),
            Item::Node { .. } => Some((of, by)),
            Item::Value { .. } => None,
        }
    }

    /// Each part the values under `keys` hold, with the stretch of it that is the node's.
    fn shares(&self, keys: &[(i64, Hash)]) -> Option<Vec<(Arc<Buffer>, Extent)>> {
        let mut out = Vec::new();
        for (k, (start, key)) in keys.iter().enumerate() {
            let end = keys.get(k + 1).map_or(i64::MAX, |(next, _)| *next);
            let within = Extent::new(*start, end);
            let parts = match &self.entries.get(key).map(|held| &held.item) {
                Some(Item::Value {
                    payload: Payload::Segments(parts),
                    ..
                }) => parts.clone(),
                Some(Item::Value {
                    payload: Payload::Run(run),
                    ..
                }) => vec![Arc::clone(&run.samples)],
                _ => Vec::new(),
            };
            let met = |part: Arc<Buffer>| {
                let met = part.extent().intersect(within);
                (!met.is_empty()).then_some((part, met))
            };
            out.extend(parts.into_iter().filter_map(met));
        }
        let held = keys.iter().any(|(_, key)| self.entries.contains_key(key));
        held.then_some(out)
    }

    fn offered_parts(&self, offered: &Offered, over: Extent) -> Option<Vec<Arc<Buffer>>> {
        let (parts, by): (Vec<Arc<Buffer>>, i64) = match offered {
            Offered::Values { keys, by } => {
                let within = over.shifted(*by);
                let met = self.shares(keys)?.into_iter();
                let met = met.filter(|(_, held)| !held.intersect(within).is_empty());
                (met.map(|(part, held)| clipped(&part, held)).collect(), *by)
            }
            Offered::Moves { of, by } => (self.resident(*of, over.shifted(*by))?.0, *by),
            Offered::Held(parts) => (parts.clone(), 0),
        };
        let over = over.shifted(by);
        let meets = |b: &&Arc<Buffer>| !b.extent().intersect(over).is_empty();
        Some(parts.iter().filter(meets).map(|p| moved(p, -by)).collect())
    }

    /// What memory holds of `key` meeting `over`, and for a node off the disk, what of `over`
    /// the disk holds that memory lacks, with its layout.
    fn resident(&self, key: Hash, over: Extent) -> Option<Resident> {
        let Item::Node { source, .. } = &self.entries.get(&key)?.item else {
            return None;
        };
        match source {
            Source::Offered { offered, .. } => Some((self.offered_parts(offered, over)?, None)),
            Source::Disk { head, chunks } => {
                let meets = |b: &&Arc<Buffer>| !b.extent().intersect(over).is_empty();
                let parts: Vec<Arc<Buffer>> = chunks.iter().filter(meets).cloned().collect();
                let mut lacks = Vec::new();
                for held in head.stored().extents() {
                    let mut asked = vec![held.intersect(over)];
                    for part in chunks {
                        asked = asked
                            .into_iter()
                            .flat_map(|a| minus(a, part.extent()))
                            .collect();
                    }
                    lacks.extend(asked.into_iter().filter(|a| !a.is_empty()));
                }
                let hull = lacks.iter().fold(Extent::NOWHERE, |h, e| h.hull(*e));
                Some((parts, (!hull.is_empty()).then(|| ((**head).clone(), hull))))
            }
        }
    }

    fn coverage(&self, key: Hash, depth: usize) -> Option<Vec<Extent>> {
        let Item::Node { source, .. } = &self.entries.get(&key)?.item else {
            return None;
        };
        let offered = match source {
            Source::Disk { head, .. } => return Some(head.stored().extents().to_vec()),
            Source::Offered { offered, .. } => offered,
        };
        match offered {
            Offered::Moves { of, by } if depth < 64 => {
                let held = self.coverage(*of, depth + 1)?;
                Some(held.into_iter().map(|e| e.shifted(-by)).collect())
            }
            Offered::Moves { .. } => None,
            Offered::Values { keys, by } => {
                let shares = self.shares(keys)?.into_iter();
                Some(shares.map(|(_, held)| held.shifted(-by)).collect())
            }
            Offered::Held(parts) => Some(parts.iter().map(|p| p.extent()).collect()),
        }
    }

    fn evict(&mut self, key: Hash) {
        let disk = match self.entries.get_mut(&key) {
            Some(Held {
                item:
                    Item::Node {
                        source: Source::Disk { chunks, .. },
                        ..
                    },
                ..
            }) => Some(std::mem::take(chunks)),
            _ => None,
        };
        match disk {
            Some(chunks) => {
                self.bytes -= chunks.iter().map(|c| planes(c)).sum::<u64>();
                self.counters.evictions += 1;
            }
            None => {
                if self.remove(key) {
                    self.counters.evictions += 1;
                }
            }
        }
    }

    /// Every entry `policy` names, oldest-read first, until `bytes` is at most `to`; then whole
    /// trees oldest-first, until the cap holds. A node off the disk keeps its header; one that
    /// holds no bytes goes only where everything named goes.
    fn prune(&mut self, policy: PrunePolicy, to: u64) {
        let newest = self.tree;
        let mut named: Vec<(u64, Hash)> = self
            .entries
            .iter()
            .filter(|(_, held)| match policy {
                PrunePolicy::Oldest => held.tree != newest,
                PrunePolicy::Forks => !held.fork,
            })
            .filter(|(_, held)| to == 0 || held.bytes() > 0)
            .map(|(key, held)| (held.read, *key))
            .collect();
        named.sort_unstable();
        for (_, key) in named {
            if self.bytes <= to && to > 0 {
                break;
            }
            self.evict(key);
        }
        let mut trees: Vec<u64> = self.entries.values().map(|held| held.tree).collect();
        trees.sort_unstable();
        trees.dedup();
        for tree in trees {
            if self.bytes <= self.max_bytes {
                break;
            }
            let whole: Vec<Hash> = self
                .entries
                .iter()
                .filter(|(_, held)| held.tree == tree && held.bytes() > 0)
                .map(|(key, _)| *key)
                .collect();
            for key in whole {
                self.evict(key);
            }
        }
    }

    fn bounded(&mut self) {
        if self.bytes > self.max_bytes {
            self.prune(self.prune, self.max_bytes);
        }
    }
}

type Resident = (Vec<Arc<Buffer>>, Option<(Header, Extent)>);

fn minus(e: Extent, cut: Extent) -> Vec<Extent> {
    let met = e.intersect(cut);
    if met.is_empty() {
        return vec![e];
    }
    vec![Extent::new(e.start, met.start), Extent::new(met.end, e.end)]
}

fn clipped(part: &Arc<Buffer>, met: Extent) -> Arc<Buffer> {
    let held = part.extent();
    match met == held {
        true => Arc::clone(part),
        false => Arc::new(part.over(met, held)),
    }
}

fn moved(part: &Arc<Buffer>, by: i64) -> Arc<Buffer> {
    if by == 0 {
        return Arc::clone(part);
    }
    let mut out = (**part).clone();
    out.start += by;
    Arc::new(out)
}

/// The memory tier: the one owner of every value and node held resident, and of what memory
/// knows a disk beneath it holds or lacks. A clone is a handle on the same memory.
#[derive(Clone)]
pub(crate) struct Memory {
    state: Arc<Mutex<State>>,
}

impl Default for Memory {
    fn default() -> Memory {
        Memory::holding(DEFAULT_CACHE_BYTES)
    }
}

impl Memory {
    pub(crate) fn holding(max_bytes: u64) -> Memory {
        Memory {
            state: Arc::new(Mutex::new(State {
                max_bytes,
                mark_every: DEFAULT_MARK_EVERY,
                ..State::default()
            })),
        }
    }

    /// Over a disk: each node a render offers is written back before it goes.
    pub(crate) fn over_disk(max_bytes: u64) -> Memory {
        let memory = Memory::holding(max_bytes);
        memory.locked().disk = true;
        memory
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.entries.clear();
            state.slots.clear();
            state.bytes = 0;
            self.state.clear_poison();
            state
        })
    }

    pub(crate) fn max_bytes(&self) -> u64 {
        self.locked().max_bytes
    }

    pub(crate) fn set_max_bytes(&self, max_bytes: u64) {
        let mut state = self.locked();
        state.max_bytes = max_bytes;
        state.bounded();
    }

    pub(crate) fn policy(&self) -> CachePolicy {
        self.locked().policy
    }

    pub(crate) fn set_policy(&self, policy: CachePolicy) {
        self.locked().policy = policy;
    }

    pub(crate) fn prune_policy(&self) -> PrunePolicy {
        self.locked().prune
    }

    pub(crate) fn set_prune_policy(&self, policy: PrunePolicy) {
        self.locked().prune = policy;
    }

    /// Evicts every entry `policy` names, and more where the cap still needs it.
    pub(crate) fn prune(&self, policy: PrunePolicy) {
        self.locked().prune(policy, 0);
    }

    /// Everything gone, each node not yet on the disk sent there first.
    pub(crate) fn clear(&self) {
        let mut state = self.locked();
        let keys: Vec<Hash> = state.entries.keys().copied().collect();
        for key in &keys {
            state.flush(*key);
        }
        for key in keys {
            state.discard(key);
        }
        state.misses.clear();
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.locked().bytes
    }

    pub(crate) fn entries(&self) -> usize {
        self.locked().entries.len()
    }

    pub(crate) fn holds(&self, key: Hash) -> bool {
        self.locked().entries.contains_key(&key)
    }

    pub(crate) fn counters(&self) -> Counters {
        self.locked().counters
    }

    pub(crate) fn count(&self, by: impl FnOnce(&mut Counters)) {
        by(&mut self.locked().counters);
    }

    pub(crate) fn mark_every(&self) -> usize {
        self.locked().mark_every
    }

    pub(crate) fn set_mark_every(&self, samples: usize) {
        self.locked().mark_every = samples.max(1);
    }

    /// Whether memory takes a value computed: as its policy says, and one offered as a node
    /// whatever it says while a disk lies beneath, to write it there; a memory of no bytes
    /// takes none.
    pub(crate) fn keeps(&self, fork: bool, target: bool, offered: bool) -> bool {
        let state = self.locked();
        state.max_bytes > 0 && ((offered && state.disk) || state.policy.keeps(fork, target))
    }

    pub(crate) fn begin_tree(&self) -> u64 {
        let mut state = self.locked();
        state.tree += 1;
        state.tree
    }

    /// A new round of lookups: what any earlier round missed is asked again.
    pub(crate) fn begin(&self) -> u64 {
        let mut state = self.locked();
        state.round += 1;
        let round = state.round;
        state.misses.retain(|_, met| *met + 1 >= round);
        round
    }

    pub(crate) fn answer(&self, key: Hash, round: u64) -> Known {
        let mut state = self.locked();
        let covered = state.coverage(key, 0);
        if let Some(held) = covered.clone().filter(|held| !held.is_empty()) {
            let tick = state.tick();
            let found = state.entries.get_mut(&key).expect("a node it covers");
            found.read = tick;
            let Item::Node { source, .. } = &found.item else {
                unreachable!("only a node covers");
            };
            return Known::Hit(Arc::new(source.stored().holding(held)));
        }
        let node = matches!(
            state.entries.get(&key),
            Some(Held {
                item: Item::Node { .. },
                ..
            })
        );
        match (node, covered) {
            (true, None) => {
                state.remove(key);
            }
            (true, Some(_)) => return Known::Miss,
            (false, _) => {}
        }
        match state.misses.get(&key) {
            Some(met) if *met >= round => Known::Miss,
            _ => Known::Unknown,
        }
    }

    pub(crate) fn miss(&self, key: Hash, round: u64) {
        let mut state = self.locked();
        let met = state.misses.entry(key).or_insert(round);
        *met = (*met).max(round);
    }

    /// A header the disk answered, resident from now on, unless memory took the node meanwhile.
    pub(crate) fn promote(&self, head: Header) {
        let mut state = self.locked();
        let (key, read, tree) = (head.stored().key, state.tick(), state.tree);
        if state.entries.contains_key(&key) {
            return;
        }
        state.misses.remove(&key);
        state.counters.promotions += 1;
        let item = Item::Node {
            source: Source::Disk {
                head: Box::new(head),
                chunks: Vec::new(),
            },
            dirty: false,
            slot: None,
        };
        let held = Held {
            item,
            read,
            tree,
            fork: false,
        };
        state.entries.insert(key, held);
    }

    pub(crate) fn promote_samples(&self, key: Hash, read: Vec<Buffer>) -> Vec<Arc<Buffer>> {
        let read: Vec<Arc<Buffer>> = read.into_iter().map(Arc::new).collect();
        let mut state = self.locked();
        let tick = state.tick();
        let Some(held) = state.entries.get_mut(&key) else {
            return read;
        };
        let Item::Node {
            source: Source::Disk { chunks, .. },
            ..
        } = &mut held.item
        else {
            return read;
        };
        let mut added = 0;
        for part in &read {
            let covered = chunks
                .iter()
                .any(|c| c.extent().intersect(part.extent()) == part.extent());
            if !covered {
                added += planes(part);
                chunks.push(Arc::clone(part));
            }
        }
        chunks.sort_by_key(|c| c.start);
        held.read = tick;
        state.bytes += added;
        state.counters.promotions += 1;
        state.bounded();
        read
    }

    /// What memory holds of `key` over `over`, and what it lacks there that the disk holds.
    pub(crate) fn resident(&self, key: Hash, over: Extent) -> Resident {
        let mut state = self.locked();
        let tick = state.tick();
        if let Some(held) = state.entries.get_mut(&key) {
            held.read = tick;
        }
        state.resident(key, over).unwrap_or_default()
    }

    pub(crate) fn forget(&self, key: Hash) {
        self.locked().remove(key);
    }

    /// A node a render computes, standing on samples memory holds, in place of the last one
    /// offered under `slot`. Until `settled` it holds what is computed so far, and a write
    /// back sends that much; settled, it goes where memory holds none of its samples and has
    /// no disk to name it to.
    pub(crate) fn offer(
        &self,
        stored: Stored,
        offered: Offered,
        (slot, settled): (Option<Hash>, bool),
    ) {
        let mut state = self.locked();
        let key = stored.key;
        state.discard(key);
        let (read, tree, dirty) = (state.tick(), state.tree, state.disk);
        let slot = slot.map(|slot| super::mixed(slot, &[0x6e_6f_64_65]));
        let item = Item::Node {
            source: Source::Offered {
                stored: Box::new(stored),
                offered,
            },
            dirty,
            slot,
        };
        let held = Held {
            item,
            read,
            tree,
            fork: false,
        };
        state.bytes += held.bytes();
        state.entries.insert(key, held);
        state.misses.remove(&key);
        let covered = state.coverage(key, 0).is_some_and(|held| !held.is_empty());
        if settled && !covered && !dirty {
            state.discard(key);
            return;
        }
        if let Some(last) = slot.and_then(|slot| state.slots.insert(slot, key))
            && last != key
        {
            state.remove(last);
        }
        state.bounded();
    }

    /// Every node not yet on the disk, on its way there.
    pub(crate) fn flush(&self) {
        let mut state = self.locked();
        let keys: Vec<Hash> = state.entries.keys().copied().collect();
        for key in keys {
            state.flush(key);
        }
        state.failed.1 = None;
    }

    /// The nodes on their way to the disk; none while a write that failed waits for a persist.
    pub(crate) fn pending(&self) -> Vec<Writeback> {
        let mut state = self.locked();
        match state.failed.1 {
            Some(_) => Vec::new(),
            None => std::mem::take(&mut state.pending),
        }
    }

    /// `left` still on its way, after a write failed for `why`.
    pub(crate) fn failed(&self, why: String, left: Vec<Writeback>) {
        let mut state = self.locked();
        state.failed.0 += 1;
        state.failed.1 = Some(why);
        let mut more = std::mem::take(&mut state.pending);
        state.pending = left;
        state.pending.append(&mut more);
    }

    pub(crate) fn written(&self) {
        self.locked().counters.writebacks += 1;
    }

    pub(crate) fn failures(&self) -> (u64, Option<String>) {
        self.locked().failed.clone()
    }

    pub(crate) fn blocked(&self) -> bool {
        self.locked().failed.1.is_some()
    }

    /// The disk committed what was staged: a header that named staged samples names nothing
    /// now.
    pub(crate) fn committed(&self) {
        let mut state = self.locked();
        let staged: Vec<Hash> = state
            .entries
            .iter()
            .filter(|(_, held)| {
                let Item::Node {
                    source: Source::Disk { head, .. },
                    ..
                } = &held.item
                else {
                    return false;
                };
                matches!(head.samples(), Samples::Staged { .. })
            })
            .map(|(key, _)| *key)
            .collect();
        for key in staged {
            state.discard(key);
        }
    }

    /// What `key` holds, shared, never copied.
    pub(crate) fn load(&self, key: Hash, expected: Expected, stamp: Stamp) -> Option<Entry> {
        let mut state = self.locked();
        let tick = state.tick();
        let held = state.entries.get_mut(&key)?;
        let Item::Value { payload, label, .. } = &held.item else {
            return None;
        };
        if !payload.answers(expected) {
            state.remove(key);
            return None;
        }
        let entry = Entry {
            payload: payload.clone(),
            label: label.clone(),
        };
        held.read = tick;
        held.tree = stamp.tree;
        held.fork = stamp.fork;
        Some(entry)
    }

    /// A value's segments join those held under `key`, and a run continuing the one held
    /// there extends it, each in place; anything else replaces what `key` held.
    pub(crate) fn merge(
        &self,
        key: Hash,
        payload: Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) -> Kept {
        let mut state = self.locked();
        let tick = state.tick();
        let joined = match (state.entries.get_mut(&key), payload) {
            (Some(held), payload) if held.slot() == stamp.slot => {
                let before = held.bytes();
                let Item::Value { payload: had, .. } = &mut held.item else {
                    unreachable!("a value key holds a value");
                };
                let payload = match (had, payload) {
                    (Payload::Segments(parts), Payload::Segments(more)) => {
                        joined(parts, more);
                        None
                    }
                    (Payload::Run(run), Payload::Run(more)) if overlaps(run, &more) => {
                        let from = (run.end() - more.samples.start).max(0) as usize;
                        let run = Arc::make_mut(run);
                        let samples = Arc::make_mut(&mut run.samples);
                        for (held, more) in samples.planes.iter_mut().zip(&more.samples.planes) {
                            held.extend_from_slice(&more[from.min(more.len())..]);
                        }
                        run.marks
                            .extend(more.marks.iter().map(|(at, m)| (*at, m.clone())));
                        None
                    }
                    (_, payload) => Some(payload),
                };
                match payload {
                    None => {
                        held.read = tick;
                        held.tree = stamp.tree;
                        held.fork = stamp.fork;
                        let after = held.bytes();
                        Ok((before, after))
                    }
                    Some(payload) => Err(payload),
                }
            }
            (_, payload) => Err(payload),
        };
        match joined {
            Ok((before, after)) => {
                state.bytes = state.bytes - before + after;
                state.bounded();
                Kept::Held
            }
            Err(payload) => {
                drop(state);
                self.store(key, payload, label, stamp)
            }
        }
    }

    /// A value too large to stay is refused, unless a node not yet on the disk stands on it:
    /// then it is kept only to be evicted, and so written back.
    pub(crate) fn store(
        &self,
        key: Hash,
        payload: Payload,
        label: Option<&Label>,
        stamp: Stamp,
    ) -> Kept {
        let mut state = self.locked();
        let bytes = payload.bytes() as u64;
        if bytes > state.max_bytes && !state.stands(key) {
            return Kept::Refused;
        }
        let read = state.tick();
        let replaced = match stamp.slot.and_then(|slot| state.slots.insert(slot, key)) {
            Some(last) if last != key => state.remove(last),
            _ => false,
        };
        let held = Held {
            item: Item::Value {
                payload,
                label: label.cloned(),
                slot: stamp.slot,
            },
            read,
            tree: stamp.tree,
            fork: stamp.fork,
        };
        if let Some(old) = state.entries.insert(key, held) {
            state.bytes -= old.bytes();
        }
        state.bytes += bytes;
        state.bounded();
        match replaced {
            true => Kept::Replaced,
            false => Kept::Held,
        }
    }
}

/// A run that starts inside or at the end of the one held continues it: what it holds past
/// that one's end is laid on, the samples both hold being the same.
fn overlaps(held: &super::Run, more: &super::Run) -> bool {
    let (a, b) = (held.samples.start, held.end());
    a <= more.samples.start && more.samples.start <= b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a colliding key reaches this, so no render can: the entry is a miss, and goes.
    #[test]
    fn an_entry_that_does_not_answer_what_was_asked_is_a_miss_and_goes() {
        let memory = Memory::default();
        let key = Hash(7, 11);
        let stamp = Stamp {
            tree: memory.begin_tree(),
            fork: false,
            slot: None,
        };
        let four = Payload::Segments(vec![Arc::new(Buffer::mono(8_000, vec![0.25; 4]))]);
        for (rate, width) in [(48_000, 1), (8_000, 2)] {
            memory.store(key, four.clone(), None, stamp);
            let asked = Expected::Segments { rate, width };
            assert!(memory.load(key, asked, stamp).is_none());
            assert!(!memory.holds(key));
            assert_eq!(memory.bytes(), 0);
        }
    }

    #[test]
    fn a_load_shares_the_samples_it_holds() {
        let memory = Memory::default();
        let key = Hash(3, 5);
        let stamp = Stamp {
            tree: memory.begin_tree(),
            fork: false,
            slot: None,
        };
        let part = Arc::new(Buffer::mono(8_000, vec![0.5; 64]));
        memory.store(key, Payload::Segments(vec![Arc::clone(&part)]), None, stamp);
        let asked = Expected::Segments {
            rate: 8_000,
            width: 1,
        };
        for _ in 0..2 {
            let loaded = memory.load(key, asked, stamp).expect("a hit");
            let Payload::Segments(parts) = loaded.payload else {
                panic!("segments were stored");
            };
            assert!(Arc::ptr_eq(&parts[0], &part), "the stored part itself");
        }
    }
}
