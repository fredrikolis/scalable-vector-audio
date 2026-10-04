// Concern: the memory tier: each resident value and its node under one cap, what it evicts and writes back, its misses | Non-concern: the disk | IO: (key) -> held, answered; (key, samples, node) -> kept

#[cfg(test)]
mod chains;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use sva_formula::Hash;
use sva_samples::{Buffer, Extent, Label};

use super::stats::{Outcome, Recording};
use super::stored::{Header, Samples};
use super::{Entry, Expected, Payload, PayloadKind, Run, Stored, joined};

pub const DEFAULT_CACHE_BYTES: u64 = 2 << 30;

/// Samples between two states a run keeps, so a reader resumes from one at most this far back.
pub const DEFAULT_MARK_EVERY: usize = 16_384;

/// What passed between memory and the disk beneath it, and what memory let go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub disk_lookups: u64,
    pub disk_reads: u64,
    pub disk_read_bytes: u64,
    /// Disk answers made resident: a header looked up, or samples read.
    pub promotions: u64,
    pub writebacks: u64,
    /// Every entry's hits, summed: each answer of a node or a value memory held.
    pub hits: u64,
    pub probation_evictions: u64,
    pub protected_evictions: u64,
}

impl Counters {
    pub fn since(self, then: Counters) -> Counters {
        Counters {
            disk_lookups: self.disk_lookups - then.disk_lookups,
            disk_reads: self.disk_reads - then.disk_reads,
            disk_read_bytes: self.disk_read_bytes - then.disk_read_bytes,
            promotions: self.promotions - then.promotions,
            writebacks: self.writebacks - then.writebacks,
            hits: self.hits - then.hits,
            probation_evictions: self.probation_evictions - then.probation_evictions,
            protected_evictions: self.protected_evictions - then.protected_evictions,
        }
    }

    pub fn evictions(self) -> u64 {
        self.probation_evictions + self.protected_evictions
    }
}

/// A node cheaper than a priced flop per this many bytes it holds is computed, never read back.
pub const BYTES_PER_FLOP: u128 = 64;

/// What a keep states of the node its samples answer: whether it is the target, and how many
/// samples it is asked over.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    pub(crate) target: bool,
    pub(crate) samples: u64,
}

/// What one keep sends memory: the samples a value computed, and the node they answer.
pub(crate) struct Keep<'a> {
    pub(crate) samples: Option<Payload>,
    pub(crate) label: Option<&'a Label>,
    pub(crate) slot: Option<Hash>,
    pub(crate) node: Option<(Stored, Offered, Facts)>,
}

/// The protected segment holds at most this share of the cap, so probation always has a
/// quarter for a new entry to earn its first hit in.
const PROTECTED: (u64, u64) = (3, 4);

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

/// Where a kept node's sample `n` is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Offered {
    /// The value held under the node's own key: a run's through each segment before it.
    Own,
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

/// A value's samples, and the node they were kept or read off the disk as.
struct Item {
    payload: Option<Payload>,
    label: Option<Label>,
    slot: Option<Hash>,
    node: Option<Node>,
}

struct Node {
    source: Source,
    /// Holds what the disk lacks: each keep of more samples sets it again.
    dirty: bool,
    bound: bool,
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

impl Node {
    fn stored(&self) -> &Stored {
        match &self.source {
            Source::Disk { head, .. } => head.stored(),
            Source::Offered { stored, .. } => stored,
        }
    }

    fn offered(&self) -> Option<&Offered> {
        match &self.source {
            Source::Offered { offered, .. } => Some(offered),
            Source::Disk { .. } => None,
        }
    }

    fn head(&self) -> Option<&Header> {
        match &self.source {
            Source::Disk { head, .. } => Some(head),
            Source::Offered { .. } => None,
        }
    }

    fn chunks(&self) -> &[Arc<Buffer>] {
        match &self.source {
            Source::Disk { chunks, .. }
            | Source::Offered {
                offered: Offered::Held(chunks),
                ..
            } => chunks,
            Source::Offered { .. } => &[],
        }
    }
}

struct Held {
    item: Item,
    read: u64,
    since: u64,
    hit_round: u64,
    protected: bool,
}

impl Held {
    fn admitted(item: Item, at: u64) -> Held {
        Held {
            item,
            read: at,
            since: at,
            hit_round: 0,
            protected: false,
        }
    }

    fn bytes(&self) -> u64 {
        let value = self.item.payload.as_ref().map_or(0, |p| p.bytes() as u64);
        let node = self.item.node.as_ref().map_or(&[][..], Node::chunks);
        value + node.iter().map(|c| planes(c)).sum::<u64>()
    }
}

fn planes(b: &Buffer) -> u64 {
    (b.len() * b.width() * size_of::<f64>()) as u64
}

#[derive(Default)]
struct State {
    entries: HashMap<Hash, Held>,
    slots: HashMap<Hash, Hash>,
    misses: HashMap<Hash, u64>,
    pending: Vec<Writeback>,
    bytes: u64,
    max_bytes: u64,
    clock: u64,
    round: u64,
    mark_every: usize,
    counters: Counters,
    disk: bool,
    /// Writes that failed, and why the latest did: none is tried again until a persist.
    failed: (u64, Option<String>),
    links: RefCell<Links>,
    #[cfg(test)]
    walked: std::cell::Cell<u64>,
}

/// Where a chain of moved nodes ends and the shift there; a chain that loops ends nowhere.
#[derive(Clone, Copy)]
struct Link {
    end: Hash,
    by: i64,
    unbound: bool,
    looped: bool,
}

/// Each moved node's link, a cache every change to `entries` evicts what it found through.
#[derive(Default)]
struct Links {
    /// Each link, with the key it was found through.
    found: HashMap<Hash, (Link, Hash)>,
    through: HashMap<Hash, HashSet<Hash>>,
}

impl Links {
    fn keep(&mut self, at: Hash, next: Hash, link: Link) {
        self.found.insert(at, (link, next));
        self.through.entry(next).or_default().insert(at);
    }

    fn evict(&mut self, key: Hash) {
        let mut open = vec![key];
        while let Some(at) = open.pop() {
            if let Some((_, next)) = self.found.remove(&at)
                && let Some(readers) = self.through.get_mut(&next)
            {
                readers.remove(&at);
                if readers.is_empty() {
                    self.through.remove(&next);
                }
            }
            open.extend(self.through.remove(&at).into_iter().flatten());
        }
    }
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn node(&self, key: Hash) -> Option<&Node> {
        self.entries.get(&key)?.item.node.as_ref()
    }

    fn stands(&self, key: Hash) -> bool {
        let bound = |at: &Hash| self.node(*at).is_some_and(|node| node.bound);
        self.doomed(key).iter().any(bound)
    }

    fn on_disk(&self, key: Hash, payload: &Payload) -> bool {
        let Some(head) = self.node(key).and_then(Node::head) else {
            return false;
        };
        let stored = head.stored();
        match payload {
            Payload::Segments(parts) => parts.iter().all(|part| stored.holds(part.extent())),
            Payload::Run(run) => stored.holds(run.samples.extent()),
            Payload::Frames(_) => false,
        }
    }

    /// A run's segment, then each before it, ending where the one after it starts.
    fn segments(&self, key: Hash) -> Vec<(Hash, Extent)> {
        let (mut out, mut at, mut end) = (Vec::new(), Some(key), i64::MAX);
        while let Some(k) = at {
            out.push((k, Extent::new(i64::MIN, end)));
            let held = self
                .entries
                .get(&k)
                .and_then(|held| held.item.payload.as_ref());
            let Some(Payload::Run(run)) = held else {
                break;
            };
            (at, end) = (run.parent, run.samples.start);
        }
        out
    }

    fn reads(&self, key: Hash) -> Vec<Hash> {
        match self.node(key).and_then(Node::offered) {
            None | Some(Offered::Held(_)) => Vec::new(),
            Some(Offered::Own) => self
                .segments(key)
                .into_iter()
                .skip(1)
                .map(|(k, _)| k)
                .collect(),
            Some(Offered::Moves { of, .. }) => vec![*of],
        }
    }

    fn doomed(&self, key: Hash) -> Vec<Hash> {
        let mut readers: HashMap<Hash, Vec<Hash>> = HashMap::new();
        for at in self.entries.keys() {
            for read in self.reads(*at) {
                readers.entry(read).or_default().push(*at);
            }
        }
        let mut out = vec![key];
        let mut k = 0;
        while k < out.len() {
            for at in readers.get(&out[k]).into_iter().flatten() {
                if !out.contains(at) {
                    out.push(*at);
                }
            }
            k += 1;
        }
        out
    }

    /// `key` and every node reading its samples gone, each dirty node flushed first: a later
    /// keep brings back what it keeps on computing.
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
        self.links.get_mut().evict(key);
        self.bytes -= gone.bytes();
        if let Some(slot) = gone.item.slot
            && self.slots.get(&slot) == Some(&key)
        {
            self.slots.remove(&slot);
        }
    }

    fn unnoded(&mut self, key: Hash) {
        let Some(held) = self.entries.get_mut(&key) else {
            return;
        };
        if held.item.payload.is_none() {
            return self.discard(key);
        }
        let before = held.bytes();
        held.item.node = None;
        self.bytes = self.bytes - before + held.bytes();
        self.links.get_mut().evict(key);
    }

    /// A dirty node's header and what memory holds of it, on their way to the disk.
    fn flush(&mut self, key: Hash) {
        let Some(Node {
            dirty: dirty @ true,
            ..
        }) = self
            .entries
            .get_mut(&key)
            .and_then(|held| held.item.node.as_mut())
        else {
            return;
        };
        *dirty = false;
        if let Some(back) = self.written(key) {
            self.pending.push(back);
        }
    }

    fn written(&self, key: Hash) -> Option<Writeback> {
        let Some(Node {
            source: Source::Offered { stored, offered },
            ..
        }) = self.node(key)
        else {
            return None;
        };
        let head = |samples| Header::new((**stored).clone(), samples);
        let referred = match offered {
            Offered::Moves { of, by } => match self.referred(*of, *by) {
                Some(found) => Some(found),
                None if self.unbound(*of) => None,
                None => return None,
            },
            _ => None,
        };
        Some(match referred {
            Some((of, by)) => Writeback {
                key,
                head: head(Samples::Of { key: of, by }),
                parts: Vec::new(),
            },
            None => Writeback {
                key,
                head: head(Samples::None),
                parts: self
                    .offered_parts(key, offered, Extent::EVERYWHERE)
                    .unwrap_or_default(),
            },
        })
    }

    fn moving(&self, key: Hash) -> Option<(Hash, i64, bool)> {
        let node = self.node(key)?;
        match node.offered()? {
            Offered::Moves { of, by } => Some((*of, *by, node.bound)),
            _ => None,
        }
    }

    /// Walked from `key` to the first link already found, each link walked kept.
    fn link(&self, key: Hash) -> Link {
        let (mut walked, mut seen, mut at) = (Vec::new(), HashSet::new(), key);
        let mut link = loop {
            if let Some((held, _)) = self.links.borrow().found.get(&at) {
                break *held;
            }
            let looped = !seen.insert(at);
            match self.moving(at) {
                Some((of, by, bound)) if !looped => {
                    #[cfg(test)]
                    self.walked.set(self.walked.get() + 1);
                    walked.push((at, of, by, !bound));
                    at = of;
                }
                moving => {
                    break Link {
                        end: at,
                        by: 0,
                        unbound: false,
                        looped: moving.is_some(),
                    };
                }
            }
        };
        for (at, of, by, unbound) in walked.into_iter().rev() {
            link.by += by;
            link.unbound |= unbound;
            if !link.looped {
                self.links.borrow_mut().keep(at, of, link);
            }
        }
        link
    }

    /// The node `stored` names, as `offered` says, in place of the last one under its key, and,
    /// `sole`, with no samples to keep, under its slot; whether memory writes it to the disk. A
    /// node the disk holds stays as the disk holds it.
    fn noded(
        &mut self,
        stored: Stored,
        offered: Offered,
        (slot, sole, facts): (Option<Hash>, bool, Facts),
    ) -> bool {
        let key = stored.key;
        if self.node(key).is_some_and(|node| node.head().is_some()) {
            return true;
        }
        let bound = self.disk && writes(&stored, facts);
        let read = self.tick();
        let same = self.node(key).is_some_and(|node| match &node.source {
            Source::Offered {
                stored: had,
                offered: was,
            } => **had == stored && *was == offered,
            Source::Disk { .. } => false,
        });
        let node = Node {
            source: Source::Offered {
                stored: Box::new(stored),
                offered,
            },
            dirty: bound,
            bound,
        };
        match self.entries.get_mut(&key) {
            Some(held) if same => {
                let node = held.item.node.as_mut().expect("the node held");
                node.dirty |= node.bound;
                held.read = read;
            }
            Some(held) => {
                let before = held.bytes();
                held.item.node = Some(node);
                held.item.slot = slot;
                held.read = read;
                let after = held.bytes();
                self.bytes = self.bytes - before + after;
                self.links.get_mut().evict(key);
            }
            None => {
                let item = Item {
                    payload: None,
                    label: None,
                    slot,
                    node: Some(node),
                };
                let held = Held::admitted(item, read);
                self.bytes += held.bytes();
                self.admit(key, held);
            }
        }
        self.misses.remove(&key);
        if let Some(last) = slot
            .filter(|_| sole)
            .and_then(|slot| self.slots.insert(slot, key))
            && last != key
        {
            self.remove(last);
        }
        bound
    }

    fn admit(&mut self, key: Hash, held: Held) -> Option<Held> {
        self.links.get_mut().evict(key);
        self.entries.insert(key, held)
    }

    fn foot(&self, key: Hash) -> Option<(Hash, i64)> {
        let link = self.link(key);
        (!link.looped).then_some((link.end, link.by))
    }

    /// A link or its end is no node the disk is written, or is only a value.
    fn unbound(&self, key: Hash) -> bool {
        let link = self.link(key);
        let end = self.entries.get(&link.end).map(|held| &held.item.node);
        link.unbound || matches!(end, Some(None | Some(Node { bound: false, .. })))
    }

    fn referred(&self, of: Hash, by: i64) -> Option<(Hash, i64)> {
        let link = self.link(of);
        if link.looped || link.unbound {
            return None;
        }
        let more = link.by;
        let node = self.node(link.end)?;
        match (node.head().map(Header::samples), node.bound) {
            (Some(Samples::Entry { file, shift, .. } | Samples::Staged { file, shift, .. }), _) => {
                Some((*file, by + more - shift))
            }
            (Some(_), _) | (None, false) => None,
            (None, true) => Some((link.end, by + more)),
        }
    }

    fn parts(&self, key: Hash) -> Option<Vec<(Arc<Buffer>, Extent)>> {
        self.entries.get(&key)?;
        let mut out = Vec::new();
        for (k, within) in self.segments(key) {
            let parts = match self
                .entries
                .get(&k)
                .and_then(|held| held.item.payload.as_ref())
            {
                Some(Payload::Segments(parts)) => parts.clone(),
                Some(Payload::Run(run)) => vec![Arc::clone(&run.samples)],
                _ => Vec::new(),
            };
            let met = |part: Arc<Buffer>| {
                let met = part.extent().intersect(within);
                (!met.is_empty()).then_some((part, met))
            };
            out.extend(parts.into_iter().filter_map(met));
        }
        Some(out)
    }

    fn own(&self, key: Hash, over: Extent) -> Option<Vec<Arc<Buffer>>> {
        let met = self.parts(key)?.into_iter();
        let met = met.filter(|(_, held)| !held.intersect(over).is_empty());
        Some(met.map(|(part, held)| clipped(&part, held)).collect())
    }

    fn offered_parts(
        &self,
        key: Hash,
        offered: &Offered,
        over: Extent,
    ) -> Option<Vec<Arc<Buffer>>> {
        let (parts, by): (Vec<Arc<Buffer>>, i64) = match offered {
            Offered::Own => (self.own(key, over)?, 0),
            Offered::Moves { of, by } => {
                let (foot, more) = self.foot(*of)?;
                (self.resident(foot, over.shifted(by + more))?.0, by + more)
            }
            Offered::Held(parts) => (parts.clone(), 0),
        };
        let over = over.shifted(by);
        let meets = |b: &&Arc<Buffer>| !b.extent().intersect(over).is_empty();
        Some(parts.iter().filter(meets).map(|p| moved(p, -by)).collect())
    }

    /// What memory holds of `key` meeting `over`, and for a node off the disk, what of `over`
    /// the disk holds that memory lacks, with its layout.
    fn resident(&self, key: Hash, over: Extent) -> Option<Resident> {
        let source = match &self.entries.get(&key)?.item.node {
            None => return Some((self.own(key, over)?, None)),
            Some(node) => &node.source,
        };
        match source {
            Source::Offered { offered, .. } => {
                Some((self.offered_parts(key, offered, over)?, None))
            }
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

    fn coverage(&self, key: Hash) -> Option<Vec<Extent>> {
        let (foot, by) = self.foot(key)?;
        let node = self.entries.get(&foot)?.item.node.as_ref();
        let held = match (node.and_then(Node::head), node.and_then(Node::offered)) {
            (Some(head), _) => head.stored().extents().to_vec(),
            (None, None | Some(Offered::Own)) => {
                let parts = self.parts(foot)?.into_iter();
                parts.map(|(_, held)| held).collect()
            }
            (None, Some(Offered::Held(parts))) => parts.iter().map(|p| p.extent()).collect(),
            (None, Some(Offered::Moves { .. })) => return None,
        };
        Some(held.into_iter().map(|e| e.shifted(-by)).collect())
    }

    /// A node off the disk keeps its header.
    fn evict(&mut self, key: Hash) {
        let Some(held) = self.entries.get_mut(&key) else {
            return;
        };
        let protected = held.protected;
        let before = held.bytes();
        let gone = match held.item.node.as_mut().map(|node| &mut node.source) {
            Some(Source::Disk { chunks, .. }) => {
                chunks.clear();
                held.item.payload = None;
                self.bytes -= before;
                true
            }
            _ => self.remove(key),
        };
        match (gone, protected) {
            (false, _) => {}
            (true, false) => self.counters.probation_evictions += 1,
            (true, true) => self.counters.protected_evictions += 1,
        }
    }

    /// A hit on `key` and on every entry whose samples it answers with: each moves to the
    /// protected segment, whose least recent move back to probation past its share.
    fn hit(&mut self, key: Hash, round: Option<u64>) {
        let tick = self.tick();
        let mut promoted = false;
        for at in self.under(key) {
            let Some(held) = self.entries.get_mut(&at) else {
                continue;
            };
            held.read = tick;
            if round.is_some_and(|round| held.hit_round == round) {
                continue;
            }
            held.hit_round = round.unwrap_or(0);
            promoted |= !std::mem::replace(&mut held.protected, true);
            self.counters.hits += 1;
        }
        if promoted {
            self.shared();
        }
    }

    fn under(&self, key: Hash) -> Vec<Hash> {
        let mut out = vec![key];
        let mut k = 0;
        while k < out.len() {
            for at in self.reads(out[k]) {
                if !out.contains(&at) {
                    out.push(at);
                }
            }
            k += 1;
        }
        out
    }

    /// Protected within its share: its least recent move back to probation, as its newest.
    fn shared(&mut self) {
        let most =
            (u128::from(self.max_bytes) * u128::from(PROTECTED.0) / u128::from(PROTECTED.1)) as u64;
        let mut protected: Vec<(u64, Hash, u64)> = self
            .entries
            .iter()
            .filter(|(_, held)| held.protected)
            .map(|(key, held)| (held.read, *key, held.bytes()))
            .collect();
        let mut bytes: u64 = protected.iter().map(|(_, _, b)| b).sum();
        protected.sort_unstable();
        for (_, key, held) in protected {
            if bytes <= most {
                break;
            }
            let tick = self.tick();
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.protected = false;
                entry.since = tick;
                bytes -= held;
            }
        }
    }

    /// Under the cap: an entry too large for it goes first, then probation oldest first,
    /// then protected least recently read.
    fn bounded(&mut self) {
        if self.bytes <= self.max_bytes {
            return;
        }
        self.shared();
        let max = self.max_bytes;
        let mut order: Vec<(u8, u64, Hash)> = self
            .entries
            .iter()
            .filter(|(_, held)| held.bytes() > 0)
            .map(|(key, held)| match (held.bytes() > max, held.protected) {
                (true, _) => (0, held.since, *key),
                (false, false) => (1, held.since, *key),
                (false, true) => (2, held.read, *key),
            })
            .collect();
        order.sort_unstable();
        for (_, _, key) in order {
            if self.bytes <= self.max_bytes {
                break;
            }
            self.evict(key);
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

    /// Over a disk: each node a value keeps is written back before it goes.
    pub(crate) fn over_disk(max_bytes: u64) -> Memory {
        let memory = Memory::holding(max_bytes);
        memory.locked().disk = true;
        memory
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.entries.clear();
            *state.links.get_mut() = Links::default();
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

    /// Whether memory takes a value computed: a memory of no bytes takes none.
    pub(crate) fn keeps(&self) -> bool {
        self.locked().max_bytes > 0
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
        if state.node(key).is_some() {
            let covered = state.coverage(key);
            if let Some(held) = covered.clone().filter(|held| !held.is_empty()) {
                state.hit(key, Some(round));
                let node = state.node(key).expect("a node held");
                return Known::Hit(Arc::new(node.stored().holding(held)));
            }
            match covered {
                None => {
                    state.remove(key);
                }
                Some(_) => return Known::Miss,
            }
        }
        match state.misses.get(&key) {
            Some(met) if *met >= round => Known::Miss,
            _ => Known::Unknown,
        }
    }

    /// `answer`, told to `seen` once a hit or a miss.
    pub(crate) fn answered(
        &self,
        (key, round): (Hash, u64),
        (node, seen): (&str, &mut Recording),
    ) -> Known {
        let known = self.answer(key, round);
        let outcome = match &known {
            Known::Hit(_) => Outcome::Hit,
            Known::Miss => Outcome::ComputedNotStored,
            Known::Unknown => return known,
        };
        let state = self.locked();
        let held = state
            .entries
            .get(&key)
            .and_then(|held| held.item.payload.as_ref());
        let kind = held.map_or(PayloadKind::Segments, Payload::kind);
        drop(state);
        seen.answered(key, node, kind, outcome);
        known
    }

    pub(crate) fn miss(&self, key: Hash, round: u64) {
        let mut state = self.locked();
        let met = state.misses.entry(key).or_insert(round);
        *met = (*met).max(round);
    }

    /// A header the disk answered, resident from now on, unless memory took the node meanwhile.
    pub(crate) fn promote(&self, head: Header) {
        let mut state = self.locked();
        let (key, read) = (head.stored().key, state.tick());
        if state.node(key).is_some() {
            return;
        }
        state.misses.remove(&key);
        state.counters.promotions += 1;
        let node = Node {
            source: Source::Disk {
                head: Box::new(head),
                chunks: Vec::new(),
            },
            dirty: false,
            bound: true,
        };
        match state.entries.get_mut(&key) {
            Some(held) => held.item.node = Some(node),
            None => {
                let item = Item {
                    payload: None,
                    label: None,
                    slot: None,
                    node: Some(node),
                };
                state.admit(key, Held::admitted(item, read));
            }
        }
        state.links.get_mut().evict(key);
    }

    pub(crate) fn promote_samples(&self, key: Hash, read: Vec<Buffer>) -> Vec<Arc<Buffer>> {
        let read: Vec<Arc<Buffer>> = read.into_iter().map(Arc::new).collect();
        let mut state = self.locked();
        let tick = state.tick();
        let Some(held) = state.entries.get_mut(&key) else {
            return read;
        };
        let Some(Node {
            source: Source::Disk { chunks, .. },
            ..
        }) = &mut held.item.node
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

    /// A value's samples, joined to what memory holds under `key`, and the node they answer,
    /// in place of the last one under its key or slot: the node first, so samples a node over
    /// the disk stands on are kept to be written back. A node holding none of its samples that
    /// memory will write nowhere goes. What became of the samples is told to `seen`.
    pub(crate) fn keep(&self, key: Hash, keep: Keep, seen: &mut Recording) {
        let Keep {
            samples,
            label,
            slot,
            node,
        } = keep;
        let sole = samples.is_none();
        let noded = node.map(|(stored, offered, facts)| {
            let key = stored.key;
            let bound = self.locked().noded(stored, offered, (slot, sole, facts));
            (key, bound)
        });
        let kept = samples.map(|samples| self.merge(key, samples, label, slot));
        if let Some(kept) = kept {
            seen.kept(key, kept);
        }
        if let Some((key, bound)) = noded {
            let mut state = self.locked();
            let covered = state.coverage(key).is_some_and(|held| !held.is_empty());
            if !covered && !bound {
                state.unnoded(key);
            }
            state.bounded();
        }
    }

    /// The nodes read after `since`, least recent first, and the clock now.
    pub(crate) fn read_since(&self, since: u64) -> (Vec<Hash>, u64) {
        let state = self.locked();
        let mut read: Vec<(u64, Hash)> = state
            .entries
            .iter()
            .filter(|(_, held)| held.read > since && held.item.node.is_some())
            .map(|(key, held)| (held.read, *key))
            .collect();
        read.sort_unstable();
        (read.into_iter().map(|(_, key)| key).collect(), state.clock)
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
                let head = held.item.node.as_ref().and_then(Node::head);
                head.is_some_and(|head| matches!(head.samples(), Samples::Staged { .. }))
            })
            .map(|(key, _)| *key)
            .collect();
        for key in staged {
            state.unnoded(key);
        }
    }

    /// What `key` holds, shared, never copied, told to `seen`.
    pub(crate) fn load(
        &self,
        key: Hash,
        expected: Expected,
        (node, seen): (&str, &mut Recording),
    ) -> Option<Entry> {
        let entry = self.entry(key, expected);
        let outcome = match entry {
            Some(_) => Outcome::Hit,
            None => Outcome::ComputedNotStored,
        };
        seen.answered(key, node, expected.kind(), outcome);
        entry
    }

    /// A run's segments held under `keys` in turn, each handed to `take` until it takes none or
    /// asks no more, told to `seen` as one lookup of the last: a prefix where it took fewer.
    pub(crate) fn runs(
        &self,
        keys: &[Hash],
        expected: Expected,
        (node, seen): (&str, &mut Recording),
        mut take: impl FnMut(usize, Arc<Run>) -> (bool, bool),
    ) {
        let mut taken = 0;
        for (k, key) in keys.iter().enumerate() {
            let Some(run) = self.entry(*key, expected).and_then(|e| e.payload.run()) else {
                break;
            };
            let (took, more) = take(k, run);
            taken += usize::from(took);
            if !(took && more) {
                break;
            }
        }
        let outcome = match taken {
            0 => Outcome::ComputedNotStored,
            n if n == keys.len() => Outcome::Hit,
            _ => Outcome::Prefix,
        };
        let last = *keys.last().expect("a run of a segment or more");
        seen.answered(last, node, PayloadKind::Run, outcome);
    }

    fn entry(&self, key: Hash, expected: Expected) -> Option<Entry> {
        let mut state = self.locked();
        let item = &state.entries.get(&key)?.item;
        let entry = Entry {
            payload: item.payload.clone()?,
            label: item.label.clone(),
        };
        if !entry.payload.answers(expected) {
            state.remove(key);
            return None;
        }
        state.hit(key, None);
        Some(entry)
    }

    /// A value's segments join those held under `key`, and a run continuing the one held
    /// there extends it, each in place; anything else replaces what `key` held.
    fn merge(
        &self,
        key: Hash,
        payload: Payload,
        label: Option<&Label>,
        slot: Option<Hash>,
    ) -> Kept {
        let mut state = self.locked();
        let tick = state.tick();
        let joined = match state.entries.get_mut(&key) {
            Some(held) if held.item.slot == slot && held.item.payload.is_some() => {
                let before = held.bytes();
                let had = held.item.payload.as_mut().expect("a value held");
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
                        let after = held.bytes();
                        Ok((before, after))
                    }
                    Some(payload) => Err(payload),
                }
            }
            _ => Err(payload),
        };
        match joined {
            Ok((before, after)) => {
                state.bytes = state.bytes - before + after;
                state.bounded();
                Kept::Held
            }
            Err(payload) => {
                drop(state);
                self.store(key, payload, label, slot)
            }
        }
    }

    /// A value the disk holds under `key` is held there alone. One too large to stay is
    /// refused, unless a node over the disk stands on it: then it is kept only to be evicted,
    /// and so written back.
    fn store(
        &self,
        key: Hash,
        payload: Payload,
        label: Option<&Label>,
        slot: Option<Hash>,
    ) -> Kept {
        let mut state = self.locked();
        if state.on_disk(key, &payload) {
            return Kept::Held;
        }
        let bytes = payload.bytes() as u64;
        if bytes > state.max_bytes && !state.stands(key) {
            return Kept::Refused;
        }
        let read = state.tick();
        let replaced = match slot.and_then(|slot| state.slots.insert(slot, key)) {
            Some(last) if last != key => state.remove(last),
            _ => false,
        };
        let old = state.entries.remove(&key);
        if let Some(old) = &old {
            state.bytes -= old.bytes();
        }
        let (node, earned) = match old {
            Some(old) => (
                old.item.node,
                Some((old.read, old.since, old.hit_round, old.protected)),
            ),
            None => (None, None),
        };
        let item = Item {
            payload: Some(payload),
            label: label.cloned(),
            slot,
            node,
        };
        let held = match earned {
            Some((read, since, hit_round, protected)) if item.node.is_some() => Held {
                item,
                read,
                since,
                hit_round,
                protected,
            },
            _ => Held::admitted(item, read),
        };
        state.bytes += held.bytes();
        state.admit(key, held);
        state.bounded();
        match replaced {
            true => Kept::Replaced,
            false => Kept::Held,
        }
    }
}

/// A node a later render is answered by, costing a flop per `BYTES_PER_FLOP` bytes or more.
fn writes(stored: &Stored, facts: Facts) -> bool {
    let bytes = u128::from(facts.samples) * u128::from(stored.width) * size_of::<f64>() as u128;
    (stored.readable || facts.target) && stored.priced * BYTES_PER_FLOP >= bytes
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
        let four = Payload::Segments(vec![Arc::new(Buffer::mono(8_000, vec![0.25; 4]))]);
        for (rate, width) in [(48_000, 1), (8_000, 2)] {
            memory.store(key, four.clone(), None, None);
            let asked = Expected::Segments { rate, width };
            assert!(memory.entry(key, asked).is_none());
            assert!(!memory.holds(key));
            assert_eq!(memory.bytes(), 0);
        }
    }

    #[test]
    fn a_load_shares_the_samples_it_holds() {
        let memory = Memory::default();
        let key = Hash(3, 5);
        let part = Arc::new(Buffer::mono(8_000, vec![0.5; 64]));
        memory.store(key, Payload::Segments(vec![Arc::clone(&part)]), None, None);
        let asked = Expected::Segments {
            rate: 8_000,
            width: 1,
        };
        for _ in 0..2 {
            let loaded = memory.entry(key, asked).expect("a hit");
            let Payload::Segments(parts) = loaded.payload else {
                panic!("segments were stored");
            };
            assert!(Arc::ptr_eq(&parts[0], &part), "the stored part itself");
        }
    }
}
