// Concern: where each node's extent ends at the decay floor, decided instant by instant | Non-concern: each class's bound (bound/), what a cut node's readers compute | IO: (&Render, nodes) -> Cuts

mod gain;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use sva_formula::NodeId;
use sva_samples::Extent;

use super::Render;
use super::bound::{Bounds, Forms, Grid, Played, Reach, STEP};
use super::extent::{self, Supports};
use crate::error::{Diagnostic, EngineError, Located};
use gain::{Gain, Gains};

/// One node cut, and the grid sample its extent ends at.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub node: String,
    pub at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    Bound,
    Gain,
}

/// One node left uncut for want of a bound on it or on its gain to the output, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Uncut {
    pub node: String,
    pub missing: Missing,
    pub why: String,
}

/// The precision a render is written at, the floor its decaying nodes are cut at, as linear
/// amplitude, and every node cut or left uncut under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cuts {
    pub bits: i32,
    pub floor: f64,
    pub cut: Vec<Cut>,
    pub uncut: Vec<Uncut>,
}

/// What a decision found: where each cut node ends, and why an endless root has no end.
pub(crate) struct Decided {
    pub(crate) at: extent::Cuts,
    pub(crate) report: Cuts,
    pub(crate) passes: u64,
    pub(crate) ops: u128,
    pub(crate) endless: Option<EngineError>,
    pub(crate) played: BTreeMap<NodeId, Played>,
}

/// The grid cuts are decided on, from the root's demand with nothing cut.
pub(crate) struct Span {
    pub(crate) first: i64,
    pub(crate) root_end: i64,
    /// The latest sample any node is read at.
    pub(crate) horizon: i64,
    /// The latest second a factor's peak is taken over.
    window_end: f64,
}

impl Span {
    pub(crate) fn of(held: &Render, nodes: &[NodeId], start: i64) -> Result<Span, EngineError> {
        let (rate, root) = (held.config.rate, held.root);
        let supports = Supports::new(&held.tys, rate);
        let root_end = held.config.range.end.unwrap_or(supports.of(root).end);
        let demand = Extent::new(start, root_end.max(start));
        let uncut = extent::decide(held, nodes, &[(root, demand)], &extent::Cuts::new())?;
        let extents: Vec<Extent> = nodes
            .iter()
            .map(|id| uncut.of(*id))
            .filter(|e| !e.is_empty())
            .collect();
        let first = extents.iter().map(|e| e.start).fold(start, i64::min);
        let reach = reach(first, &extents);
        let endless = root_end == i64::MAX;
        let horizon = match endless {
            true => reach.max(first.saturating_add(i64::from(rate))),
            false => reach,
        };
        let grid = Grid {
            first,
            rate: f64::from(rate),
        };
        let window_end = match endless {
            true => i64::MAX as f64 / f64::from(rate),
            false => grid.secs(points(first, horizon) - 1),
        };
        Ok(Span {
            first,
            root_end,
            horizon,
            window_end,
        })
    }

    pub(crate) fn endless(&self) -> bool {
        self.root_end == i64::MAX
    }
}

fn points(first: i64, horizon: i64) -> usize {
    ((horizon - first) as usize).div_ceil(STEP) + 1
}

fn reach(first: i64, extents: &[Extent]) -> i64 {
    extents
        .iter()
        .map(|e| e.end)
        .filter(|end| *end != i64::MAX)
        .fold(first + 1, i64::max)
}

/// A node whose bound times gain is under `floor / N` from T on ends at T, `N` the count
/// that could be cut, decided one grid instant after another: an instant decided never
/// changes, so a stream that decides as it plays cuts where a whole render does.
pub(crate) struct Search<'a> {
    pub(crate) bounds: Bounds<'a>,
    audio: Vec<NodeId>,
    root: NodeId,
    floor: f64,
    window: (f64, f64),
    ready: bool,
    gains: BTreeMap<NodeId, Gain>,
    share: f64,
    /// Each node with a bound and a gain, and where its support ends.
    candidates: Vec<(NodeId, f64, i64)>,
    crossed: BTreeSet<NodeId>,
    pub(crate) decided: usize,
    pub(crate) cuts: extent::Cuts,
    budget: u128,
    pub(crate) exhausted: bool,
}

impl<'a> Search<'a> {
    pub(crate) fn new(
        forms: Rc<Forms<'a>>,
        audio: &[NodeId],
        root: NodeId,
        span: &Span,
    ) -> Result<Search<'a>, EngineError> {
        let config = &forms.config;
        let resolution = config.profile.half_lsb();
        let floor = config.decay_floor.unwrap_or(resolution);
        if !(floor >= resolution && floor.is_finite()) {
            let name = forms.tys.name(root).to_string();
            let bits = config.profile.precision_bits;
            return Err(below_resolution(&name, bits, floor, resolution));
        }
        let grid = Grid {
            first: span.first,
            rate: f64::from(config.rate),
        };
        let budget = config.flop_budget;
        let level = floor / audio.len().max(1) as f64;
        let window = (grid.secs(0), span.window_end);
        Ok(Search {
            bounds: Bounds::new(forms, grid, level),
            audio: audio.to_vec(),
            root,
            floor,
            window,
            ready: false,
            gains: BTreeMap::new(),
            share: floor,
            candidates: Vec::new(),
            crossed: BTreeSet::new(),
            decided: 0,
            cuts: extent::Cuts::new(),
            budget,
            exhausted: false,
        })
    }

    /// Each node's first instant states its bound over all time, which gains read.
    fn prepare(&mut self) -> Result<Reach, EngineError> {
        let mut reached = Vec::new();
        gain::topological(
            self.bounds.tys(),
            self.root,
            &mut BTreeSet::new(),
            &mut reached,
        );
        for id in self.audio.iter().chain(&reached) {
            if let Reach::Wait(node, chunk) = self.bounds.extend(*id, 1)? {
                return Ok(Reach::Wait(node, chunk));
            }
        }
        let perturbable: BTreeSet<NodeId> = self
            .audio
            .iter()
            .filter(|id| self.bounds.peak(**id).is_some() && **id != self.root)
            .copied()
            .collect();
        let (tys, rate) = (self.bounds.tys(), self.bounds.config().rate);
        let supports = Supports::new(tys, rate);
        let gains =
            Gains::new(tys, &self.bounds, &supports, &perturbable, self.window).from(self.root)?;
        let candidates = self
            .audio
            .iter()
            .filter(|id| self.bounds.peak(**id).is_some())
            .filter_map(|id| {
                let gain = *gains.get(id)?.as_ref().ok()?;
                Some((*id, gain, supports.of(*id).end))
            })
            .collect();
        self.candidates = candidates;
        self.share = self.floor / self.candidates.len().max(1) as f64;
        for id in self.bounds.solvers() {
            let level = match gains.get(&id) {
                Some(Ok(g)) if *g > 1.0 => self.share / g,
                _ => self.bounds.level,
            };
            if let Some(tail) = self.bounds.solver_mut(id) {
                tail.walk.set_level(level);
            }
        }
        self.gains = gains;
        self.ready = true;
        Ok(Reach::Ready)
    }

    /// Nothing is left to decide.
    pub(crate) fn done(&self) -> bool {
        self.exhausted || (self.ready && self.crossed.len() == self.candidates.len())
    }

    /// Every grid instant through `through`, until the budget is spent: the one way a render,
    /// a stream and lint decide a cut. A walk waits on its solver's machine, from `sites`.
    pub(crate) fn decide(&mut self, through: usize, sites: &dyn Sites) -> Result<(), EngineError> {
        loop {
            match self.advance(through)? {
                Reach::Ready => return Ok(()),
                Reach::Wait(solver, chunk) => {
                    let fresh = || sites.player(solver);
                    self.bounds.play(solver, chunk, &fresh);
                }
            }
        }
    }

    pub(crate) fn played(&mut self) -> BTreeMap<NodeId, Played> {
        self.bounds
            .solvers()
            .into_iter()
            .filter_map(|id| Some((id, self.bounds.played(id)?)))
            .collect()
    }

    /// A node past its crossing is read on only where another node reads it.
    fn advance(&mut self, through: usize) -> Result<Reach, EngineError> {
        if !self.ready
            && let Reach::Wait(node, chunk) = self.prepare()?
        {
            return Ok(Reach::Wait(node, chunk));
        }
        while self.decided <= through && !self.done() {
            let j = self.decided;
            for &(id, ..) in &self.candidates {
                if self.crossed.contains(&id) {
                    continue;
                }
                if let Reach::Wait(node, chunk) = self.bounds.extend(id, j + 1)? {
                    return Ok(Reach::Wait(node, chunk));
                }
            }
            for &(id, gain, end) in &self.candidates {
                let crossed = self.crossed.contains(&id) || self.bounds.len(id) <= j;
                if crossed || self.bounds.at(id, j) * gain > self.share {
                    continue;
                }
                self.crossed.insert(id);
                let at = self.bounds.grid.sample(j);
                if at < end {
                    self.cuts.insert(id, at);
                }
            }
            self.decided += 1;
            self.exhausted = self.bounds.ops > self.budget;
        }
        Ok(Reach::Ready)
    }

    pub(crate) fn report(&self) -> Cuts {
        let tys = self.bounds.tys();
        let config = self.bounds.config();
        let mut uncut: Vec<Uncut> = self
            .audio
            .iter()
            .filter_map(|id| {
                let node = tys.name(*id).to_string();
                match (self.bounds.unbounded(*id), self.gains.get(id)) {
                    (Some(unbounded), _) => Some(Uncut {
                        node,
                        missing: Missing::Bound,
                        why: format!("`{}` is {}", unbounded.node, unbounded.class),
                    }),
                    (None, Some(Err(why))) => Some(Uncut {
                        node,
                        missing: Missing::Gain,
                        why: why.clone(),
                    }),
                    _ => None,
                }
            })
            .collect();
        uncut.sort_by(|a, b| a.node.cmp(&b.node));
        Cuts {
            bits: config.profile.precision_bits,
            floor: self.floor,
            cut: self
                .cuts
                .iter()
                .map(|(id, at)| Cut {
                    node: tys.name(*id).to_string(),
                    at: *at,
                })
                .collect(),
            uncut,
        }
    }

    fn hopeless(&self) -> bool {
        self.bounds.unbounded(self.root).is_some() || self.bounds.floor(self.root) >= self.share
    }

    /// Why an endless root has no end: no bound on it, a level it holds forever, or a bound
    /// still over the floor where the search stopped.
    fn no_end(&self) -> EngineError {
        let name = self.bounds.tys().name(self.root).to_string();
        let (code, message, help) = match self.bounds.unbounded(self.root) {
            Some(unbounded) => (
                "render.no_bound",
                format!(
                    "`{}` is {}, and no bound is derived for it, so `{name}` is never shown \
                     to end",
                    unbounded.node, unbounded.class
                ),
                "give the interval an end, as `[0, 2s]`, or crop it",
            ),
            None if !self.bounds.tail(self.root).is_finite() => (
                "render.no_bound",
                format!("`{name}` has no finite bound, so it is never shown to end"),
                "give the interval an end, as `[0, 2s]`, or crop it",
            ),
            None if self.bounds.floor(self.root) >= self.share => (
                "render.never_ends",
                format!(
                    "`{name}` returns to {} forever, at or above the decay floor's share {}",
                    dbfs(self.bounds.floor(self.root)),
                    dbfs(self.share)
                ),
                "crop it, give it a release, or give the interval an end",
            ),
            None => (
                "render.no_end",
                format!(
                    "`{name}`'s bound at {:.3}s is {}, over the decay floor's share {}, and \
                     the budget ends the search there",
                    self.bounds.grid.secs(self.decided.saturating_sub(1)),
                    dbfs(self.bounds.tail(self.root)),
                    dbfs(self.share)
                ),
                "raise --flop-budget to look further, raise --decay-floor, or give the \
                 interval an end",
            ),
        };
        EngineError::refused(Diagnostic {
            code: code.to_string(),
            message,
            location: Located::at(&name, None),
            help: help.to_string(),
        })
    }
}

/// Where each solver's own machine stands, to play on from.
pub(crate) trait Sites {
    fn player(&self, solver: NodeId) -> Option<Played>;
}

/// No sample yet: each solver's machine plays from rest, and plays its samples.
struct Rest<'r>(&'r Render);

impl Sites for Rest<'_> {
    fn player(&self, solver: NodeId) -> Option<Played> {
        let held = self.0;
        let program = super::sampled::program(held, solver).ok()?;
        let rate = held.config.rate;
        let mut machine =
            sva_samples::Machine::open(&program.renderer, &program.layout, rate).ok()?;
        machine.hear(STEP as u64);
        let tape = sva_samples::Tape::new(machine.width(), 0, 0);
        Some(Played { machine, tape })
    }
}

/// Every instant through the horizon decided before any sample, since a render prices,
/// keys and computes each node over its whole extent before its readers. With no end, the
/// search looks twice as far each pass until the root is cut and every node read before its
/// end is decided there, the root is shown never to fall, or the budget is spent.
pub(crate) fn decide(held: &Render, nodes: &[NodeId], start: i64) -> Result<Decided, EngineError> {
    let span = Span::of(held, nodes, start)?;
    let forms = Rc::new(Forms::new(
        Cow::Borrowed(&held.tys),
        Cow::Borrowed(&held.config),
    ));
    let mut search = Search::new(forms, nodes, held.root, &span)?;
    let (root, first) = (held.root, span.first);
    let mut horizon = span.horizon;
    loop {
        search.decide(points(first, horizon) - 1, &Rest(held))?;
        if !span.endless() || search.exhausted {
            break;
        }
        let next = match search.cuts.get(&root) {
            Some(end) => {
                let over = Extent::new(start, (*end).max(start));
                let decided = extent::decide(held, nodes, &[(root, over)], &search.cuts)?;
                let needed: Vec<Extent> = nodes.iter().map(|id| decided.of(*id)).collect();
                Some(reach(first, &needed)).filter(|need| *need > horizon)
            }
            None if search.hopeless() || search.done() => None,
            None => Some(first + (horizon - first).saturating_mul(2)),
        };
        match next.filter(|_| next_fits(first, horizon)) {
            Some(next) => horizon = next,
            None => break,
        }
    }
    let endless = (span.endless() && !search.cuts.contains_key(&root)).then(|| search.no_end());
    Ok(Decided {
        at: search.cuts.clone(),
        report: search.report(),
        passes: search.decided as u64,
        ops: search.bounds.ops,
        endless,
        played: search.played(),
    })
}

pub(crate) fn under(tys: &crate::typing::Typing, id: NodeId) -> BTreeSet<NodeId> {
    let (mut seen, mut work) = (BTreeSet::new(), vec![id]);
    while let Some(next) = work.pop() {
        if seen.insert(next) {
            work.extend(gain::operands(tys, next));
        }
    }
    seen
}

/// The most instants one grid holds, a few hours at any audio rate.
const MAX_POINTS: i64 = 1 << 22;

fn next_fits(first: i64, horizon: i64) -> bool {
    (horizon - first) / STEP as i64 <= MAX_POINTS / 2
}

fn below_resolution(name: &str, bits: i32, floor: f64, resolution: f64) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "render.floor_below_resolution".to_string(),
        message: format!(
            "the decay floor {} is under the {} a {bits}-bit sample resolves",
            dbfs(floor),
            dbfs(resolution),
        ),
        location: Located::at(name, None),
        help: "raise --decay-floor to the resolution or above, or raise --bits".to_string(),
    })
}

pub(crate) fn dbfs(v: f64) -> String {
    match v.is_finite() {
        true => format!("{:.1} dBFS", 20.0 * v.log10()),
        false => "unbounded".to_string(),
    }
}
