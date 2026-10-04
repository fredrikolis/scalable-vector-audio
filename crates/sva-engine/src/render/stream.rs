// Concern: opens a target as a stream over memory, editing it and its terms live | Non-concern: pulling its blocks, what an edit carries on | IO: (Graph, target) -> Stream; (Change) -> Changed

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::{Expr, Graph};
use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Extent};

use super::drive::{Block, Driver};
use super::end::{Ending, Fading, Heard, under};
use super::table::support::Supports;
use super::table::{Past, Table};
use super::terms::{Handle, NOTES, Terms, cut, placed};
use super::world::{Plan, Root, STREAMED, Walked, Wanted, World};
use super::{Ends, RenderConfig, range_over};
use crate::cache::{Backend, CacheStats, Counters, Memory, Recording, Stored, Tier};
use crate::error::{Diagnostic, EngineError, Located};
use crate::flops::Work;
use crate::recent::Recent;

#[cfg(test)]
mod rebuilt;

/// The lookups, and the names started silent, a stream keeps of all it made.
pub const LATEST: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct StreamConfig {
    pub block: usize,
    /// The target's own at open where `None`; a mono one plays in each, a wider is refused.
    pub channels: Option<usize>,
    pub render: RenderConfig,
}

/// A target rendered block by block off one table, reading `@notes`, the sum of the terms
/// added. A change builds only what it changed or newly reads; the rest stays as it stood.
pub struct Stream {
    config: StreamConfig,
    world: World,
    driver: Driver,
    expr: Expr,
    terms: Terms,
    width: usize,
    /// Each sounding term as heard at the root, through `gain` from the note sum.
    heard: BTreeMap<Handle, Heard>,
    gain: Option<f64>,
    /// Each term retired before its support ended.
    fading: Vec<Fading>,
    faded: f64,
    /// The first sounding term's end, which `heard` evicts.
    ending: Option<Option<i64>>,
    /// The sample a silence cut ends the stream's root at.
    cut: Option<i64>,
    generation: u64,
    live: bool,
    dropped: Recent<String>,
    late: usize,
    built: Built,
    demands: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub dropped: usize,
    /// Edits that landed past where issued.
    pub late: usize,
    pub terms: usize,
    /// The latest change's, or the open's.
    pub built: Built,
    /// The reads that worked out what the root asks of the note sum.
    pub demands: usize,
    pub tier: Counters,
}

/// What one change did anew, not what it carried over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// Texts it took in: its expression and each node it newly reads.
    pub parsed: usize,
    pub instances: usize,
    pub visited: usize,
    pub typed: usize,
    pub values: usize,
    /// Values it made taking an old one's state.
    pub copied: usize,
    pub lookups: usize,
}

impl Stream {
    pub async fn open<B: Backend>(
        graph: &Graph,
        target: &Expr,
        config: StreamConfig,
        tier: &Tier<B>,
    ) -> Result<Stream, EngineError> {
        let render = blocked(&config)?.render.clone();
        if graph.defines(STREAMED) {
            return Err(refusal(format!(
                "this composition already has a node named `{STREAMED}`"
            )));
        }
        let world = World::over(graph, render.rate, !graph.defines(NOTES));
        let recording = Recording::over(tier.memory()).latest(LATEST);
        let table = Table::new(&render.profile);
        let driver = Driver::new(table, Extent::new(0, 0), config.block, &render, recording);
        let mut stream = Stream {
            world,
            driver,
            expr: target.clone(),
            terms: Terms::default(),
            width: 0,
            heard: BTreeMap::new(),
            gain: None,
            fading: Vec::new(),
            faded: 0.0,
            ending: None,
            cut: None,
            generation: 0,
            live: false,
            dropped: Recent::keeping(LATEST),
            late: 0,
            built: Built::default(),
            demands: 0,
            config,
        };
        let opening = Prospect {
            target: target.clone(),
            terms: Terms::default(),
            term: None,
            from: None,
            answer: Changed::Edited,
            landing: None,
            parsed: 1,
        };
        let mut local = Local::default();
        let round = tier.begin();
        loop {
            match stream.attempt(&opening, &mut local, (tier.memory(), round))? {
                Attempt::Landed(_) => break,
                Attempt::Asks(keys) => local.look(keys, tier, round).await,
                Attempt::Reads(wants) => local.read(wants, tier).await,
                Attempt::Moved => unreachable!("nothing plays a stream before it opens"),
            }
        }
        let mut needs = stream.needs();
        while !needs.is_empty() {
            let fetched = tier.fetch(&needs).await;
            for (key, parts) in fetched.handed {
                stream.driver.table.took(key, &parts);
            }
            needs = fetched.left;
        }
        Ok(stream)
    }

    pub fn graph(&self) -> &Graph {
        &self.world.graph
    }

    fn prospect(&self, change: Change) -> Result<Prospect, Changed> {
        let mut landing = None;
        let prospect = |terms, term: Option<(Handle, Expr)>, from: Option<Graph>, answer| {
            let roots = |(handle, term): &(Handle, Expr)| sva_ast::reads_of(&handle.node(), term);
            Prospect {
                target: self.expr.clone(),
                terms,
                from: from.map(|graph| (graph, term.as_ref().map(roots).unwrap_or_default())),
                term,
                answer,
                landing: None,
                parsed: 1,
            }
        };
        Ok(match change {
            Change::Target(graph, target) => {
                let roots = sva_ast::reads_of(STREAMED, &target);
                Prospect {
                    target,
                    from: Some((graph, roots)),
                    ..prospect(self.terms.clone(), None, None, Changed::Edited)
                }
            }
            Change::Add(graph, term, at) => {
                let term = match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, *landing.insert(self.driver.at)),
                };
                let (terms, handle) = self.terms.added();
                Prospect {
                    landing,
                    ..prospect(
                        terms,
                        Some((handle, term)),
                        Some(graph),
                        Changed::Added(handle),
                    )
                }
            }
            Change::Replace(handle, graph, term, at) => {
                let landed = self.terms.landed(handle).ok_or(Changed::Held(false))?;
                let term = match at {
                    Placed::Written => term,
                    Placed::Landing => placed(&term, landed),
                };
                let terms = self.terms.clone();
                prospect(
                    terms,
                    Some((handle, term)),
                    Some(graph),
                    Changed::Held(true),
                )
            }
            Change::Remove(handle) => {
                let at = self.driver.at;
                let terms = self.terms.removed(handle).ok_or(Changed::Held(false))?;
                let held = self.world.graph.expr(&handle.node());
                let term = cut(held.expect("a held term's node"), at);
                Prospect {
                    parsed: 0,
                    ..prospect(terms, Some((handle, term)), None, Changed::Held(true))
                }
            }
        })
    }

    /// One try at `prospect`: planned, typed and built beside what plays, then held, or undone
    /// where memory must answer first.
    fn attempt(
        &mut self,
        prospect: &Prospect,
        local: &mut Local,
        (memory, round): (&Memory, u64),
    ) -> Result<Attempt, EngineError> {
        if prospect.landing.is_some_and(|at| at != self.driver.at) {
            return Ok(Attempt::Moved);
        }
        let wanted = Wanted {
            root: Root::Streamed(&prospect.target),
            terms: &prospect.terms,
            term: prospect.term.as_ref().map(|(h, e)| (*h, e)),
            from: prospect.from.as_ref().map(|(g, roots)| (g, roots.clone())),
            whole: false,
        };
        let found = |key: Hash| memory.answer(key, round);
        let walked = self.world.plan(&wanted, &self.config.render, &found)?;
        let mut plan = match walked {
            Walked::Asks(keys) => return Ok(Attempt::Asks(keys)),
            Walked::Planned(plan) => plan,
        };
        let built = self.built(&mut plan);
        let (root, range, cut) = match built {
            Ok(held) => held,
            Err(e) => {
                let freed = self.world.abort();
                self.driver.table.abort(&freed);
                return Err(e);
            }
        };
        let from = self.driver.at.max(range.start);
        let future = Extent::new(from, self.last(range.end).max(from));
        let table = &mut self.driver.table;
        let short = table.short((root, future, Past::Stored));
        let opened = short
            .into_iter()
            .try_for_each(|at| table.read_on(&self.world.typing, at));
        if let Err(e) = opened {
            let freed = self.world.abort();
            self.driver.table.abort(&freed);
            return Err(e);
        }
        let window = ahead(from, self.config.render.rate);
        let wants = self.driver.table.needs_made(root, window);
        let wants: Vec<(Hash, Extent)> = wants
            .into_iter()
            .filter(|(key, over)| !local.holds(*key, *over))
            .collect();
        if !wants.is_empty() {
            let freed = self.world.abort();
            self.driver.table.abort(&freed);
            return Ok(Attempt::Reads(wants));
        }
        self.cut = cut;
        Ok(Attempt::Landed(self.land(
            prospect,
            (plan, root, range),
            local,
        )))
    }

    /// The plan typed and built: the root's value, its range, and where a cut ends it.
    fn built(&mut self, plan: &mut Plan) -> Result<(usize, Extent, Option<i64>), EngineError> {
        let typing = &mut self.world.typing;
        let id = typing
            .id(STREAMED)
            .ok_or_else(|| EngineError::UnknownNode(STREAMED.to_string()))?;
        let hits: BTreeMap<NodeId, Arc<Stored>> = plan
            .stored
            .iter()
            .filter_map(|(path, stored)| Some((typing.id(path)?, Arc::clone(stored))))
            .collect();
        let table = &mut self.driver.table;
        let root = table.grow(typing, id, &hits)?;
        let plays = table.values[root].width;
        let mut render = self.config.render.clone();
        match self.width {
            0 if self.config.channels == Some(0) => {
                return Err(refusal("a stream of no channels".to_string()));
            }
            0 => widens(plays, self.config.channels.unwrap_or(plays))?,
            width => {
                widens(plays, width)?;
                render.range.start = Some(self.driver.start);
            }
        }
        let supports = Supports::over(typing, Some(&table.supports));
        let ending = Ending::new(typing, &render.profile, &supports);
        let end = match (render.range.end, self.live) {
            (None, false) => ending.of(id),
            _ => ending.exact(id),
        };
        let range = range_over((&render, STREAMED), end.support, Ends::Pulled)?;
        Ok((root, range, end.cut))
    }

    /// The change held, each value it made carrying on what it continues.
    fn land(
        &mut self,
        prospect: &Prospect,
        (mut plan, root, range): (Plan, usize, Extent),
        local: &mut Local,
    ) -> Changed {
        let freed = self.world.commit(std::mem::take(&mut plan.found));
        let now = self.driver.at;
        let made = self.driver.table.made().to_vec();
        let carried = self.driver.table.settled(root, &freed, (now, self.live));
        self.driver.table.priced(range, &made);
        self.driver.table.offers(&self.world.typing, range);
        for (key, parts) in &local.fetched {
            self.driver.table.took(*key, parts);
        }
        for at in &carried.silent {
            self.dropped
                .push(self.driver.table.values[*at].name.clone());
        }
        match self.width {
            0 => {
                let plays = self.driver.table.values[root].width;
                self.width = self.config.channels.unwrap_or(plays);
                (self.driver.start, self.driver.at) = (range.start, range.start);
            }
            _ => self.late += usize::from(self.driver.at > local.issued),
        }
        let last = self.last(range.end);
        self.driver.bound(last);
        self.driver.recording.found(std::mem::take(&mut plan.hits));
        self.expr = prospect.target.clone();
        self.terms = prospect.terms.clone();
        if let Changed::Added(handle) = prospect.answer {
            self.terms.land(handle, self.driver.at);
        }
        self.hear();
        self.built = Built {
            parsed: prospect.parsed + plan.adopted,
            instances: plan.named,
            visited: plan.visited,
            typed: self.world.typing.lowered().len(),
            values: self.driver.table.built,
            copied: carried.taken,
            lookups: local.lookups,
        };
        self.generation += 1;
        self.prune();
        prospect.answer
    }

    /// A term is its own sound, judged at the root through the note sum's gain to it.
    fn hear(&mut self) {
        let typing = &self.world.typing;
        let supports = Supports::over(typing, Some(&self.driver.table.supports));
        let ending = Ending::new(typing, &self.config.render.profile, &supports);
        let gain = match (typing.id(STREAMED), typing.id(NOTES)) {
            (Some(root), Some(notes)) => ending.gain(root, notes),
            _ => None,
        };
        let moved = gain != std::mem::replace(&mut self.gain, gain);
        let lowered: BTreeSet<&str> = typing.lowered().iter().map(String::as_str).collect();
        for handle in self.terms.handles() {
            let node = handle.node();
            if !moved && !lowered.contains(node.as_str()) && self.heard.contains_key(&handle) {
                continue;
            }
            let id = typing.id(&node).expect("a sounding term is typed");
            self.heard.insert(handle, ending.heard(id, gain));
        }
        self.ending = None;
    }

    fn needs(&self) -> Vec<(Hash, Extent)> {
        let next = ahead(self.driver.at, self.config.render.rate);
        self.driver.table.needs(next)
    }

    /// `n` samples from `at`, `None` past the end. An `at` behind is refused; one ahead skips
    /// there, computing through the span, or, live, as `go_live` says.
    pub fn read(&mut self, at: i64, n: usize) -> Result<Option<Block>, EngineError> {
        let now = self.driver.at;
        if at < now {
            return Err(refused(
                "engine.stream_behind",
                format!("sample {at} is before sample {now}, where the stream stands"),
                "read from the stream's position or later",
            ));
        }
        if n == 0 {
            return Err(refused(
                "engine.empty_read",
                format!("a read of no samples at sample {at}"),
                "read one sample or more",
            ));
        }
        match self.live {
            true if at > now => {
                for silenced in self.driver.skip(at)? {
                    self.dropped
                        .push(self.driver.table.values[silenced].name.clone());
                }
            }
            _ => {
                let block = self.config.block;
                while self.driver.at < at {
                    let step = block.min((at - self.driver.at) as usize);
                    self.reads_on(step)?;
                    if !self.driver.pulled(step)? {
                        return Ok(None);
                    }
                    self.prune();
                }
            }
        }
        self.reads_on(n)?;
        let block = self.driver.read(n)?;
        self.prune();
        Ok(block.map(|b| b.widened(self.width)))
    }

    /// Each node memory answered that the next `n` samples ask past what it holds reads on from
    /// its own value, made as the change that made it would have: live, a stateful one already
    /// sounding then starts silent.
    fn reads_on(&mut self, n: usize) -> Result<(), EngineError> {
        let at = self.driver.at;
        let range = Extent::new(self.driver.start, self.driver.last());
        let window = Extent::new(at, at.saturating_add(n as i64).min(range.end).max(at));
        let table = &mut self.driver.table;
        if window.is_empty() {
            return Ok(());
        }
        for short in table.short((table.root, window, Past::Held)) {
            table.read_on(&self.world.typing, short)?;
            let made = table.made().to_vec();
            let landed = table.landed(short);
            let carried = table.settled(table.root, &[], (landed, self.live));
            for silent in carried.silent {
                self.dropped.push(table.values[silent].name.clone());
            }
            table.priced(range, &made);
            table.offers(&self.world.typing, range);
        }
        Ok(())
    }

    /// An edited node with no state, or one a read skips past, starts silent, never computing
    /// its past, named in `dropped`; a formula reads on exactly.
    pub fn go_live(&mut self) {
        self.live = true;
        let last = self.last(self.driver.last());
        self.driver.bound(last);
    }

    fn last(&self, range_end: i64) -> i64 {
        match self.live {
            true => self.config.render.range.end.unwrap_or(i64::MAX),
            false => range_end,
        }
    }

    /// The latest nodes a live edit started silent, of `counts().dropped`.
    pub fn dropped(&self) -> Vec<&str> {
        self.dropped.iter().map(String::as_str).collect()
    }

    pub fn counts(&self) -> Counts {
        Counts {
            dropped: self.dropped.made(),
            late: self.late,
            terms: self.terms.count(),
            built: self.built,
            demands: self.demands,
            tier: self.driver.recording.since(),
        }
    }

    /// Retires every term silent at the root by now and from the first sample of `notes` the
    /// root asks, while all it retired early, summed through the gain, stay under the level.
    fn prune(&mut self) {
        let now = self.driver.at;
        let heard = &self.heard;
        let ending = *self
            .ending
            .get_or_insert_with(|| heard.values().map(|h| h.support.end).min());
        if ending.is_none_or(|end| end > now) {
            return;
        }
        let (table, tys) = (&self.driver.table, &self.world.typing);
        let last = self.driver.last();
        let asked = match tys.id(NOTES).and_then(|notes| table.of(notes)) {
            Some(notes) if now < last => {
                self.demands += 1;
                let needs = table.demand(Extent::new(now, last));
                needs[notes].hold.iter().next().map(|asked| asked.start)
            }
            Some(_) => None,
            None => Some(i64::MIN),
        };
        let level = self.config.render.profile.prune_level();
        let gain = self.gain.unwrap_or(f64::INFINITY);
        if let Some(from) = asked {
            // Far under the level, a bound joins one sum, so the list stays short.
            let let_go = level * 2f64.powi(-30);
            let mut faded = self.faded;
            self.fading.retain(|f| match f.from(from) {
                b if b <= let_go => {
                    faded = (faded + b) * (1.0 + f64::EPSILON);
                    false
                }
                _ => true,
            });
            self.faded = faded;
        }
        let mut gone = BTreeSet::new();
        for (handle, h) in &self.heard {
            let end = h.support.end;
            if end > now || asked.is_some_and(|from| end > from) {
                continue;
            }
            let (Some(from), Some(fading)) = (asked, &h.fading) else {
                gone.insert(*handle);
                continue;
            };
            let own = fading.from(from);
            if own > 0.0 {
                let left: f64 = self.fading.iter().map(|f| f.from(from)).sum();
                let ops = self.fading.len() as f64 + 3.0;
                let left = (self.faded + left + own) * (1.0 + ops * f64::EPSILON);
                if !under(gain, left, level) {
                    continue;
                }
                self.fading.push(fading.clone());
            }
            gone.insert(*handle);
        }
        let heard = &self.heard;
        let support = |handle: Handle| heard.get(&handle).map(|h| h.support);
        let went = self
            .terms
            .retire(&|handle| gone.contains(&handle), &support);
        if !went.is_empty() {
            self.generation += 1;
        }
        for handle in went {
            self.heard.remove(&handle);
            self.ending = None;
        }
    }

    /// Every segment of its own clock the stream computed of `node`'s value, in order.
    pub fn evaluated(&self, node: &str) -> Vec<sva_samples::Extent> {
        let table = &self.driver.table;
        self.world
            .typing
            .id(node)
            .and_then(|id| table.of(id))
            .map_or(Vec::new(), |at| table.values[at].evaluated.clone())
    }

    pub fn pruned(&self) -> sva_samples::Pruned {
        sva_samples::Pruned {
            db: self.config.render.profile.prune_db,
            cuts: self
                .cut
                .map(|at| (STREAMED.to_string(), at))
                .into_iter()
                .collect(),
        }
    }

    pub fn landed(&self, handle: Handle) -> Option<i64> {
        self.terms.landed(handle)
    }

    pub fn position(&self) -> i64 {
        self.driver.at
    }

    pub fn work(&self) -> Work {
        self.driver.work
    }

    pub fn stats(&self) -> CacheStats {
        self.driver.recording.stats()
    }

    /// The bytes its values hold, samples and state.
    pub fn held_bytes(&self) -> usize {
        self.driver.table.bytes()
    }

    pub fn end(&self) -> Option<i64> {
        self.driver.end()
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn config(&self) -> &StreamConfig {
        &self.config
    }
}

/// An edit, each with the graph that reaches it and all the stream plays.
pub enum Change {
    Target(Graph, Expr),
    Add(Graph, Expr, Placed),
    Replace(Handle, Graph, Expr, Placed),
    /// A sounding term is cut where the stream stands as the edit is built; what played stays.
    Remove(Handle),
}

/// Where a term's sample 0 sits: the stream's own, or the sample its add lands at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Written,
    Landing,
}

/// A replace or remove answers whether the stream still held its handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    Edited,
    Added(Handle),
    Held(bool),
}

/// What a change wants played, and what it answers once it lands.
struct Prospect {
    target: Expr,
    terms: Terms,
    term: Option<(Handle, Expr)>,
    from: Option<(Graph, Vec<String>)>,
    answer: Changed,
    landing: Option<i64>,
    /// Its own expression, where it brought one.
    parsed: usize,
}

/// What one try at a change came to.
enum Attempt {
    Landed(Changed),
    Asks(Vec<Hash>),
    Reads(Vec<(Hash, Extent)>),
    /// The stream left the sample it was placed at.
    Moved,
}

/// What one change has looked up and read, across its tries.
#[derive(Default)]
struct Local {
    fetched: Vec<(Hash, Vec<Arc<Buffer>>)>,
    asked: Vec<(Hash, Extent)>,
    unread: BTreeSet<Hash>,
    lookups: usize,
    issued: i64,
}

impl Local {
    async fn look<B: Backend>(&mut self, keys: Vec<Hash>, tier: &Tier<B>, round: u64) {
        for key in keys {
            self.lookups += 1;
            tier.lookup(key, round).await;
        }
    }

    /// What `wants` asks, off memory; one a fetch's reads did not reach is asked again.
    async fn read<B: Backend>(&mut self, wants: Vec<(Hash, Extent)>, tier: &Tier<B>) {
        let fetched = tier.fetch(&wants).await;
        for (key, over) in wants {
            if fetched.left.contains(&(key, over)) {
                continue;
            }
            self.asked.push((key, over));
            if !fetched.handed.iter().any(|(held, _)| *held == key) {
                self.unread.insert(key);
            }
        }
        self.fetched.extend(fetched.handed);
    }

    /// Whether this change already read `key` over `over`, or could not.
    fn holds(&self, key: Hash, over: Extent) -> bool {
        self.unread.contains(&key)
            || self
                .asked
                .iter()
                .any(|(k, e)| *k == key && e.intersect(over) == over)
    }
}

/// `build`'s change, the stream held only while each try runs, between two blocks: an exact
/// stream pulled once its edit is done plays it where issued. `build` runs again whenever the
/// stream changed under it.
pub async fn change<E: From<EngineError>, B: Backend>(
    stream: &RefCell<Stream>,
    mut build: impl FnMut(&Stream) -> Result<Change, E>,
    tier: &Tier<B>,
) -> Result<Changed, E> {
    let mut local = Local {
        issued: stream.borrow().driver.at,
        ..Local::default()
    };
    let round = tier.begin();
    loop {
        let change = build(&stream.borrow())?;
        let prospect = match stream.borrow().prospect(change) {
            Ok(prospect) => prospect,
            Err(answer) => return Ok(answer),
        };
        let generation = stream.borrow().generation;
        loop {
            if stream.borrow().generation != generation {
                break;
            }
            let memory = (tier.memory(), round);
            let attempt = stream.borrow_mut().attempt(&prospect, &mut local, memory)?;
            match attempt {
                Attempt::Landed(answer) => return Ok(answer),
                Attempt::Moved => break,
                Attempt::Asks(keys) => local.look(keys, tier, round).await,
                Attempt::Reads(wants) => local.read(wants, tier).await,
            }
        }
    }
}

/// What the next second of `stream` reads off memory, as far as one fetch reaches; a block
/// reading samples not ready computes them, or, live, starts them silent.
pub async fn fetch<B: Backend>(stream: &RefCell<Stream>, tier: &Tier<B>) {
    let needs = stream.borrow().needs();
    let fetched = tier.fetch(&needs).await;
    let mut stream = stream.borrow_mut();
    for (key, parts) in fetched.handed {
        stream.driver.table.took(key, &parts);
    }
}

fn ahead(at: i64, rate: u32) -> Extent {
    Extent::new(at, at.saturating_add(i64::from(rate)))
}

fn widens(plays: usize, width: usize) -> Result<(), EngineError> {
    match plays == width || plays == 1 {
        true => Ok(()),
        false => Err(refused(
            "engine.stream_width",
            format!("this plays {plays} channel(s), and the stream plays {width}"),
            "play as many channels as the stream, or one, or open a new stream for it",
        )),
    }
}

fn blocked(config: &StreamConfig) -> Result<&StreamConfig, EngineError> {
    match config.block {
        0 => Err(refusal("a block of no samples".to_string())),
        _ => Ok(config),
    }
}

pub(super) fn refusal(what: String) -> EngineError {
    refused(
        "engine.no_stream",
        format!("this target opens no stream: {what}"),
        "stream an expression over the nodes the composition defines",
    )
}

fn refused(code: &str, message: String, help: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(STREAMED, None),
        help: help.to_string(),
    })
}
