// Concern: folds a source's node texts into a graph, desugars repeat/concat | Non-concern: where the texts come from (source.rs), one file's own content (ingest.rs) | IO: (&dyn Source) -> Graph

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::diag::{ByteSpan, DiagCode};
use crate::expr::{
    Address, Arg, BinOp, Binds, Expr, Literal, children, map_children, map_children_ok,
};
use crate::filename::{FileSpan, SpanUnit};
use crate::ingest::{base_name, parse_file};
use crate::refusal::Refusal;
use crate::skipped::{Skip, Skipped};
use crate::source::Source;
use crate::tsv::Grid;

/// The one reserved folder name; its contents are ordinary nodes, so `@variables/bpm` refs.
pub const VARIABLES: &str = "variables";

#[derive(Clone, Debug)]
pub struct Graph {
    root: String,
    nodes: BTreeMap<String, Arc<Defined>>,
    grids: HashMap<String, Grid>,
    spans: HashMap<String, Option<FileSpan>>,
    per_bar: Option<PerBar>,
    skipped: Vec<Skipped>,
    /// Each node's text as its source answered it, or as a defined one prints.
    texts: HashMap<String, String>,
    /// The edit of its source each node's text was read at, where the source counts them.
    generations: HashMap<String, u64>,
}

/// A node's body and the default lines written above it, held whole so a reader may keep it.
#[derive(Clone, Debug, PartialEq)]
pub struct Defined {
    pub body: Expr,
    pub defaults: Vec<(String, Expr)>,
}

/// A node's parse with its text, span and grid, as one graph hands it to another.
#[derive(Clone, Debug)]
pub struct Held {
    defined: Arc<Defined>,
    text: String,
    generation: Option<u64>,
    span: Option<FileSpan>,
    grid: Option<Grid>,
}

impl Held {
    pub fn body(&self) -> &Expr {
        &self.defined.body
    }
}

fn has_bar_literal(e: &Expr) -> bool {
    match e {
        Expr::Lit(Literal::Bars(_)) => true,
        _ => children(e, Binds::Substitute)
            .into_iter()
            .any(has_bar_literal),
    }
}

/// Seconds per bar as the quotient `beats * 60 / bpm`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerBar {
    pub seconds: f64,
    pub per: f64,
}

impl PerBar {
    pub fn secs(self) -> f64 {
        self.seconds / self.per
    }
}

fn resolve_bar_literals(e: &Expr, per_bar: PerBar) -> Expr {
    let num = |v: f64| Box::new(Expr::Lit(Literal::Num(v)));
    match e {
        Expr::Lit(Literal::Bars(bars)) => Expr::Bin(
            BinOp::Mul,
            num(*bars),
            Box::new(Expr::Bin(
                BinOp::Div,
                num(per_bar.seconds),
                num(per_bar.per),
            )),
        ),
        _ => map_children_ok(e, Binds::Substitute, |c| resolve_bar_literals(c, per_bar)),
    }
}

fn rebind(e: &Expr, to: &dyn Fn(&str) -> Option<Expr>) -> Expr {
    match e {
        Expr::Ref {
            path,
            arg,
            binds,
            address,
            span,
        } => Expr::Ref {
            path: path.clone(),
            arg: Box::new(rebind(arg, to)),
            binds: binds
                .iter()
                .map(|(k, v)| (k.clone(), to(k).unwrap_or_else(|| rebind(v, to))))
                .collect(),
            address: *address,
            span: *span,
        },
        _ => map_children_ok(e, Binds::Substitute, |c| rebind(c, to)),
    }
}

impl Graph {
    /// Every binding of a parameter `to` answers, by a ref or a default, bound to that answer.
    pub fn rebound(&self, to: &dyn Fn(&str) -> Option<Expr>) -> Graph {
        let mut out = self.clone();
        for defined in out.nodes.values_mut() {
            let defaults = defined.defaults.iter();
            let defaults =
                defaults.map(|(k, v)| (k.clone(), to(k).unwrap_or_else(|| rebind(v, to))));
            *defined = Arc::new(Defined {
                body: rebind(&defined.body, to),
                defaults: defaults.collect(),
            });
        }
        out
    }

    pub fn defines(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
    }

    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    pub fn expr(&self, path: &str) -> Option<&Expr> {
        self.nodes.get(path).map(|defined| &defined.body)
    }

    pub fn defined(&self, path: &str) -> Option<&Arc<Defined>> {
        self.nodes.get(path)
    }

    /// A node's whole text, comments and all, before any parse.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.texts.get(path).map(String::as_str)
    }

    pub fn generation(&self, path: &str) -> Option<u64> {
        self.generations.get(path).copied()
    }

    /// What an invocation binding nothing for a name gets, in written order. A caller's own
    /// binding wins, so this is only ever consulted for names the invocation left out.
    pub fn defaults(&self, path: &str) -> &[(String, Expr)] {
        self.nodes
            .get(path)
            .map_or(&[], |defined| defined.defaults.as_slice())
    }

    /// A global by name, `variables/` first and the composition root second — where nothing
    /// distinguishes a global from a signal, and which is transitional.
    pub fn global(&self, name: &str) -> Option<&Expr> {
        self.expr(&format!("{VARIABLES}/{name}"))
            .or_else(|| self.expr(name))
    }

    pub fn span(&self, path: &str) -> Option<FileSpan> {
        self.spans.get(path).copied().flatten()
    }

    /// `None` for a node no TSV file backs — every other node reads as a plain expression.
    pub fn grid(&self, path: &str) -> Option<&Grid> {
        self.grids.get(path)
    }

    /// Every TSV grid node is re-derived from its own cells against the span it now carries,
    /// so a pattern's row spacing and the `crop`/`repeat` placing it share one resolved
    /// number. Re-deriving changes only shift literals, so the checks already run stay valid.
    pub fn resolve_bar_spans(&mut self, per_bar: PerBar) {
        let seconds_per_bar = per_bar.secs();
        for span in self.spans.values_mut().flatten() {
            if span.unit == SpanUnit::Bars {
                span.amount *= seconds_per_bar;
                span.unit = SpanUnit::Seconds;
            }
        }
        for (path, grid) in &self.grids {
            let Some(Some(span)) = self.spans.get(path) else {
                continue;
            };
            if let Some(defined) = self.nodes.get_mut(path) {
                Arc::make_mut(defined).body = crate::tsv::materialize(grid, span.amount);
            }
        }
        self.per_bar = Some(per_bar);
        for defined in self.nodes.values_mut() {
            let defined = Arc::make_mut(defined);
            defined.body = resolve_bar_literals(&defined.body, per_bar);
            for value in &mut defined.defaults {
                value.1 = resolve_bar_literals(&value.1, per_bar);
            }
        }
    }

    /// Nodes still holding a `b` literal — no tempo has been resolved against them, and the
    /// engine is unit-free, so a caller refuses rather than picking a seconds-per-bar.
    pub fn unresolved_bar_literals(&self) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|(_, defined)| {
                has_bar_literal(&defined.body)
                    || defined.defaults.iter().any(|(_, v)| has_bar_literal(v))
            })
            .map(|(path, _)| path.as_str())
            .collect()
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.nodes.keys().map(String::as_str)
    }

    pub fn seconds_per_bar(&self) -> Option<f64> {
        self.per_bar.map(PerBar::secs)
    }

    pub fn per_bar(&self) -> Option<PerBar> {
        self.per_bar
    }

    /// Adds a node no file backs — an ad-hoc expression read in this graph's namespace.
    /// `false` when `path` is taken, so a probe can never shadow a composition's own file.
    pub fn define(&mut self, path: &str, expr: Expr) -> bool {
        if self.nodes.contains_key(path) {
            return false;
        }
        let held = self.defining(expr);
        self.set(path, Some(held));
        true
    }

    /// A node no file backs, its bar literals resolved here.
    pub fn defining(&self, expr: Expr) -> Held {
        let body = match self.per_bar {
            Some(per_bar) => resolve_bar_literals(&expr, per_bar),
            None => expr,
        };
        Held {
            text: crate::print::render(&body),
            generation: None,
            defined: Arc::new(Defined {
                body,
                defaults: Vec::new(),
            }),
            span: None,
            grid: None,
        }
    }

    pub fn held(&self, path: &str) -> Option<Held> {
        Some(Held {
            defined: Arc::clone(self.nodes.get(path)?),
            text: self
                .texts
                .get(path)
                .cloned()
                .expect("every node holds its text"),
            generation: self.generations.get(path).copied(),
            span: self.spans.get(path).copied().flatten(),
            grid: self.grids.get(path).cloned(),
        })
    }

    /// What `path` held before.
    pub fn set(&mut self, path: &str, held: Option<Held>) -> Option<Held> {
        let before = self.held(path);
        match held {
            Some(held) => {
                self.nodes.insert(path.to_string(), held.defined);
                self.texts.insert(path.to_string(), held.text);
                match held.generation {
                    Some(at) => self.generations.insert(path.to_string(), at),
                    None => self.generations.remove(path),
                };
                self.spans.insert(path.to_string(), held.span);
                match held.grid {
                    Some(grid) => self.grids.insert(path.to_string(), grid),
                    None => self.grids.remove(path),
                };
            }
            None => {
                self.nodes.remove(path);
                self.texts.remove(path);
                self.generations.remove(path);
                self.spans.remove(path);
                self.grids.remove(path);
            }
        }
        before
    }

    /// Whether `other` holds `path` as this graph does: the same text under the same name.
    pub fn holds_as(&self, path: &str, other: &Graph) -> bool {
        match (self.nodes.get(path), other.nodes.get(path)) {
            (Some(a), Some(b)) if Arc::ptr_eq(a, b) => true,
            (Some(_), Some(_)) => self.texts.get(path) == other.texts.get(path),
            (None, None) => true,
            _ => false,
        }
    }

    /// Each node `path` reads directly, by its body or a default.
    fn reaching_from(&self, path: &str) -> Vec<String> {
        let Some(defined) = self.nodes.get(path) else {
            return Vec::new();
        };
        let mut out = reads_of(path, &defined.body);
        for (_, value) in &defined.defaults {
            out.extend(reads_of(path, value));
        }
        out
    }

    /// Each node `roots` reach here, following each one's reads.
    pub fn reaching(&self, roots: &[String]) -> BTreeSet<String> {
        let (mut seen, mut open) = (BTreeSet::new(), roots.to_vec());
        while let Some(path) = open.pop() {
            let Some(defined) = self.nodes.get(&path) else {
                continue;
            };
            if seen.insert(path.clone()) {
                open.extend(reads_of(&path, &defined.body));
                for (_, value) in &defined.defaults {
                    open.extend(reads_of(&path, value));
                }
            }
        }
        seen
    }

    /// Each node `roots` reach through `from` that this graph lacks, taken.
    pub fn adopt(&mut self, from: &Graph, roots: &[String]) -> Vec<String> {
        let mut taken = Vec::new();
        let mut open: Vec<String> = roots.iter().rev().cloned().collect();
        while let Some(path) = open.pop() {
            if self.defines(&path) {
                continue;
            }
            let Some(held) = from.held(&path) else {
                continue;
            };
            open.extend(reads_of(&path, &held.defined.body));
            for (_, value) in &held.defined.defaults {
                open.extend(reads_of(&path, value));
            }
            self.set(&path, Some(held));
            taken.push(path);
        }
        taken
    }

    /// Expands every `repeat`/`concat` call into `crop` + shifted-ref + sum (FORMAT.md sugar).
    /// Must run AFTER [`Self::resolve_bar_spans`]; a still-`Bars` span here is refused.
    pub fn desugar_arrangement(&mut self) -> Result<(), Vec<Refusal>> {
        self.desugared(None)
    }

    /// The same, a span this graph lacks read in `beside`.
    pub fn desugar_arrangement_beside(&mut self, beside: &Graph) -> Result<(), Vec<Refusal>> {
        self.desugared(Some(beside))
    }

    fn desugared(&mut self, beside: Option<&Graph>) -> Result<(), Vec<Refusal>> {
        let paths: Vec<String> = self.nodes.keys().cloned().collect();
        let mut refusals = Vec::new();
        let mut rewritten = Vec::new();

        for path in &paths {
            let expr = self.expr(path).expect("path came from nodes.keys()");
            match self.rewrite_arrangement(path, expr, beside) {
                Ok(new_expr) => rewritten.push((path.clone(), new_expr)),
                Err(r) => refusals.push(r),
            }
        }

        if !refusals.is_empty() {
            return Err(refusals);
        }
        for (path, new_expr) in rewritten {
            if let Some(defined) = self.nodes.get_mut(&path)
                && defined.body != new_expr
            {
                Arc::make_mut(defined).body = new_expr;
            }
        }
        Ok(())
    }

    pub fn define_arranged(&mut self, path: &str, expr: Expr) -> Result<bool, Refusal> {
        let arranged = self.rewrite_arrangement(path, &written(path, &expr)?, None)?;
        Ok(self.define(path, arranged))
    }

    /// The same, a span this graph lacks read in `beside`.
    pub fn define_arranged_beside(
        &mut self,
        beside: &Graph,
        path: &str,
        expr: Expr,
    ) -> Result<bool, Refusal> {
        let arranged = self.rewrite_arrangement(path, &written(path, &expr)?, Some(beside))?;
        Ok(self.define(path, arranged))
    }

    fn rewrite_arrangement(
        &self,
        referencing: &str,
        e: &Expr,
        beside: Option<&Graph>,
    ) -> Result<Expr, Refusal> {
        let rebuilt = map_children(e, Binds::Substitute, |c| {
            self.rewrite_arrangement(referencing, c, beside)
        })?;
        // Bottom-up: a `repeat`/`concat` expands only once its own arguments are rewritten.
        match &rebuilt {
            Expr::Call { name, args, span } if name == "repeat" => {
                self.expand_repeat((referencing, beside), args, *span)
            }
            Expr::Call { name, args, span } if name == "concat" => {
                self.expand_concat((referencing, beside), args, *span)
            }
            _ => Ok(rebuilt),
        }
    }

    /// `repeat(@x, n)` desugars to `concat(@x, @x, ..., @x)`, `n` copies of the same argument.
    fn expand_repeat(
        &self,
        (referencing, beside): (&str, Option<&Graph>),
        args: &[Arg],
        span: ByteSpan,
    ) -> Result<Expr, Refusal> {
        let (x, n_expr) = match (arg_expr(args, 0), arg_expr(args, 1)) {
            (Some(x), Some(n)) if args.len() == 2 => (x, n),
            _ => {
                return Err(Refusal::new(
                    referencing,
                    span,
                    DiagCode::BadArrangementArg,
                    "`repeat` takes exactly two arguments: a ref and an integer copy count",
                ));
            }
        };

        let n = match n_expr {
            Expr::Lit(Literal::Num(n)) if *n >= 0.0 && n.fract() == 0.0 => *n as usize,
            _ => {
                return Err(Refusal::new(
                    referencing,
                    span,
                    DiagCode::BadArrangementArg,
                    "`repeat`'s second argument must be a non-negative whole-number literal",
                ));
            }
        };

        let concat_args: Vec<Arg> = (0..n).map(|_| Arg::Pos(x.clone())).collect();
        self.expand_concat((referencing, beside), &concat_args, span)
    }

    /// `concat(@a, @b, ...)` desugars to a sum of each argument, cropped to its own
    /// filename-declared span and time-shifted by the cumulative duration of the args before
    /// it — sequential placement, zero manual offset arithmetic from the composition author.
    fn expand_concat(
        &self,
        (referencing, beside): (&str, Option<&Graph>),
        args: &[Arg],
        span: ByteSpan,
    ) -> Result<Expr, Refusal> {
        let mut terms = Vec::with_capacity(args.len());
        let mut offset = 0.0f64;

        for a in args {
            let e = match a {
                Arg::Pos(e) | Arg::Named(_, e) => e,
            };
            let Expr::Ref {
                path,
                arg,
                binds,
                address,
                span: ref_span,
            } = e
            else {
                return Err(Refusal::new(
                    referencing,
                    span,
                    DiagCode::BadArrangementArg,
                    "`concat`'s arguments must each be a plain `@path` ref to a file with a \
                     declared span",
                ));
            };
            if **arg != Expr::Var("t".to_string()) || *address != Address::Time {
                return Err(Refusal::new(
                    referencing,
                    *ref_span,
                    DiagCode::BadArrangementArg,
                    format!(
                        "`@{path}` inside `concat`/`repeat` must be a bare ref (no custom \
                         time argument); a shift is added automatically"
                    ),
                ));
            }

            let Some(target) = resolve_ref_path(referencing, path) else {
                return Err(above_root(referencing, *ref_span, path, &self.root));
            };
            let held = self.span(&target);
            let file_span = match held.or_else(|| beside?.span(&target)) {
                Some(fs) if fs.unit == SpanUnit::Seconds => fs,
                Some(_) => {
                    return Err(Refusal::new(
                        referencing,
                        *ref_span,
                        DiagCode::BadArrangementArg,
                        format!(
                            "`@{path}`'s span is still in bars — resolve_bar_spans must run \
                             before desugar_arrangement"
                        ),
                    ));
                }
                None => {
                    return Err(Refusal::new(
                        referencing,
                        *ref_span,
                        DiagCode::BadArrangementArg,
                        format!(
                            "`@{path}` (resolved to `{target}`) has no declared span; `concat` \
                             needs one to know its duration"
                        ),
                    ));
                }
            };

            let start = offset;
            let end = offset + file_span.amount;
            let shifted_ref = Expr::Ref {
                path: path.clone(),
                arg: Box::new(Expr::Bin(
                    BinOp::Sub,
                    Box::new(Expr::Var("t".to_string())),
                    Box::new(Expr::Lit(Literal::Num(start))),
                )),
                binds: binds.clone(),
                address: Address::Time,
                span: *ref_span,
            };
            terms.push(Expr::Call {
                name: "crop".to_string(),
                args: vec![
                    Arg::Pos(shifted_ref),
                    Arg::Pos(Expr::Lit(Literal::Num(start))),
                    Arg::Pos(Expr::Lit(Literal::Num(end))),
                ],
                span,
            });
            offset = end;
        }

        Ok(terms
            .into_iter()
            .reduce(|acc, t| Expr::Bin(BinOp::Add, Box::new(acc), Box::new(t)))
            .unwrap_or(Expr::Lit(Literal::Num(0.0))))
    }
}

fn arg_expr(args: &[Arg], idx: usize) -> Option<&Expr> {
    args.get(idx).map(|a| match a {
        Arg::Pos(e) | Arg::Named(_, e) => e,
    })
}

/// Every node the source names. What a whole composition can be told about itself — `lint`, an
/// entry-point set — needs all of them, so this is what a directory load is.
pub fn load(source: &dyn Source) -> Result<Graph, Vec<Refusal>> {
    let mut loading = Loading::new(source.name());
    let listing = source
        .paths()
        .map_err(|reason| vec![loading.directory(reason)])?;
    for path in &listing.found {
        match names_a_node(path) {
            true => {
                loading.pull(source, path);
            }
            false => loading.skipped.push(Skipped {
                path: path.clone(),
                reason: Skip::Unnameable,
            }),
        }
    }
    for path in &listing.unreadable {
        loading.skipped.push(Skipped {
            path: path.clone(),
            reason: Skip::Special,
        });
    }
    loading.finish(None)
}

/// Whether anything names this path: a ref path, or one an instance's binds follow, which
/// a render still addresses by name though no `@ref` spells it.
pub fn names_a_node(path: &str) -> bool {
    let bare = match path.split_once('(') {
        Some((before, binds)) if binds.ends_with(')') => before,
        _ => path,
    };
    !bare.is_empty() && crate::lexer::whole_ref_path(bare)
}

/// The root set [`load_reaching`] follows, for a caller whose expression is not in the source
/// yet. A bareword call names a file too, so every call name is named speculatively.
pub fn reads_of(referencing: &str, expr: &Expr) -> Vec<String> {
    let mut named = Vec::new();
    collect_named(expr, &mut named);
    named
        .iter()
        .filter_map(|name| resolve_ref_path(referencing, name))
        .collect()
}

/// Only what `roots` reach, following each parsed node's own reads: an unreferenced node is
/// never asked for, and one the source cannot answer for is a located dangling-ref refusal.
pub fn load_reaching(source: &dyn Source, roots: &[&str]) -> Result<Graph, Vec<Refusal>> {
    let mut loading = Loading::new(source.name());
    let mut work: Vec<String> = roots.iter().rev().map(|r| (*r).to_string()).collect();
    while let Some(path) = work.pop() {
        if loading.holds(&path) || !loading.pull(source, &path) {
            continue;
        }
        work.extend(loading.reads(&path));
    }
    loading.finish(None)
}

/// What `roots` reach that `base` does not hold, or holds at an earlier edit of `source`:
/// each node read and parsed once, a ref into `base` resolving there. A node `base` holds as
/// `source` still does is walked through, never read.
pub fn load_beside(
    source: &dyn Source,
    roots: &[&str],
    base: &Graph,
) -> Result<Graph, Vec<Refusal>> {
    let mut loading = Loading::new(source.name());
    let mut walked = BTreeSet::new();
    let mut work: Vec<String> = roots.iter().rev().map(|r| (*r).to_string()).collect();
    while let Some(path) = work.pop() {
        let current = base.defines(&path)
            && source
                .generation(&path)
                .is_none_or(|at| base.generation(&path) == Some(at));
        if current {
            if walked.insert(path.clone()) {
                work.extend(base.reaching_from(&path));
            }
            continue;
        }
        if loading.holds(&path) || !loading.pull(source, &path) {
            continue;
        }
        work.extend(loading.reads(&path));
    }
    loading.finish(Some(base))
}

/// One parsed node at a time, so pulling everything and pulling a closure are the same walk.
struct Loading {
    name: String,
    nodes: BTreeMap<String, Expr>,
    defaults: BTreeMap<String, Vec<(String, Expr)>>,
    grids: HashMap<String, Grid>,
    spans: HashMap<String, Option<FileSpan>>,
    missing: BTreeSet<String>,
    refusals: Vec<Refusal>,
    skipped: Vec<Skipped>,
    texts: HashMap<String, String>,
    generations: HashMap<String, u64>,
}

impl Loading {
    fn new(name: String) -> Loading {
        Loading {
            name,
            nodes: BTreeMap::new(),
            defaults: BTreeMap::new(),
            grids: HashMap::new(),
            spans: HashMap::new(),
            missing: BTreeSet::new(),
            refusals: Vec::new(),
            skipped: Vec::new(),
            texts: HashMap::new(),
            generations: HashMap::new(),
        }
    }

    fn directory(&self, reason: String) -> Refusal {
        Refusal::new(self.name.clone(), ByteSpan::at(0), DiagCode::Io, reason)
    }

    /// A miss counts as settled: a builtin name every node calls would otherwise cost the
    /// source one failed read per referencing node.
    fn holds(&self, path: &str) -> bool {
        self.nodes.contains_key(path)
            || self.spans.contains_key(path)
            || self.missing.contains(path)
    }

    /// `false` where nothing was added: the source does not hold this path, or its text is not
    /// a node — and the second is already recorded as its own located refusal.
    fn pull(&mut self, source: &dyn Source, path: &str) -> bool {
        let text = match source.get(path) {
            Ok(Some(text)) => text,
            Ok(None) => {
                self.missing.insert(path.to_string());
                return false;
            }
            Err(reason) => {
                self.refusals
                    .push(Refusal::new(path, ByteSpan::at(0), DiagCode::Io, reason));
                self.spans.insert(path.to_string(), None);
                return false;
            }
        };
        let (_, span) = crate::filename::parse_filename(base_name(path));
        self.spans.insert(path.to_string(), span);
        self.texts.insert(path.to_string(), text.to_string());
        if let Some(at) = source.generation(path) {
            self.generations.insert(path.to_string(), at);
        }
        match parse_file(base_name(path), &text) {
            Ok(parsed) => {
                self.nodes.insert(path.to_string(), parsed.expr);
                if !parsed.defaults.is_empty() {
                    self.defaults.insert(path.to_string(), parsed.defaults);
                }
                if let Some(grid) = parsed.grid {
                    self.grids.insert(path.to_string(), grid);
                }
                true
            }
            Err(d) => {
                self.refusals
                    .push(Refusal::new(path, d.span, d.code, d.message));
                false
            }
        }
    }

    fn reads(&self, path: &str) -> Vec<String> {
        match self.nodes.get(path) {
            Some(expr) => reads_of(path, expr),
            None => Vec::new(),
        }
    }

    /// Every ref resolving here or in `base`.
    fn finish(self, base: Option<&Graph>) -> Result<Graph, Vec<Refusal>> {
        if !self.refusals.is_empty() {
            return Err(self.refusals);
        }
        let held =
            |path: &str| self.nodes.contains_key(path) || base.is_some_and(|b| b.defines(path));
        let dangling = check_refs((&self.nodes, &held), &self.texts, &self.name);
        if !dangling.is_empty() {
            return Err(dangling);
        }
        let cycles = check_cycles(&self.nodes);
        if !cycles.is_empty() {
            return Err(cycles);
        }
        let mut defaults = self.defaults;
        let nodes = self.nodes.into_iter().map(|(path, body)| {
            let defaults = defaults.remove(&path).unwrap_or_default();
            (path, Arc::new(Defined { body, defaults }))
        });
        Ok(Graph {
            root: self.name,
            nodes: nodes.collect(),
            grids: self.grids,
            spans: self.spans,
            per_bar: None,
            skipped: self.skipped,
            texts: self.texts,
            generations: self.generations,
        })
    }
}

/// Resolves against the REFERENCING file's own directory, `./` and a bare path alike. A `.`
/// or `..` normalizes anywhere, so `@a/x/../b` and `@a/b` are one node. `None` above the root,
/// and for an absolute path, which only a command line resolves.
pub fn resolve_ref_path(referencing: &str, ref_path: &str) -> Option<String> {
    if ref_path.starts_with('/') {
        return None;
    }
    let dir = match referencing.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let mut segments: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in ref_path.split('/') {
        match segment {
            "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// A probe is parsed whole rather than as a file, so its sums are written out here.
fn written(path: &str, expr: &Expr) -> Result<Expr, Refusal> {
    crate::series::written_out(expr).map_err(|d| Refusal::new(path, d.span, d.code, d.message))
}

fn above_root(referencing: &str, span: ByteSpan, ref_path: &str, root: &str) -> Refusal {
    Refusal::new(
        referencing,
        span,
        DiagCode::RefAboveRoot,
        format!(
            "`@{ref_path}` in `{referencing}` walks above the composition root `{root}`; a \
             ref may reach anywhere inside the root and nowhere outside it"
        ),
    )
}

/// One `@path(...)` read, and whether it reads at the referencing node's own `t`.
struct RefSite {
    path: String,
    span: ByteSpan,
    at_t: bool,
}

fn collect_refs(e: &Expr, out: &mut Vec<RefSite>) {
    if let Expr::Ref {
        path, arg, span, ..
    } = e
    {
        out.push(RefSite {
            path: path.clone(),
            span: *span,
            at_t: **arg == Expr::Var("t".to_string()),
        });
    }
    for c in children(e, Binds::Substitute) {
        collect_refs(c, out);
    }
}

/// Every path this expression names, `@ref` or bareword call alike.
fn collect_named(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Call { name, .. } => out.push(name.clone()),
        Expr::Ref { path, .. } => out.push(path.clone()),
        _ => {}
    }
    for c in children(e, Binds::Substitute) {
        collect_named(c, out);
    }
}

type Holds<'a> = &'a dyn Fn(&str) -> bool;

fn check_refs(
    (nodes, held): (&BTreeMap<String, Expr>, Holds),
    texts: &HashMap<String, String>,
    root: &str,
) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    for (path, expr) in nodes {
        let mut refs = Vec::new();
        collect_refs(expr, &mut refs);
        for site in refs {
            let Some(target) = resolve_ref_path(path, &site.path) else {
                refusals.push(above_root(path, site.span, &site.path, root));
                continue;
            };
            if !held(&target) {
                let hint = texts
                    .get(path)
                    .and_then(|text| digit_segment_hint(text, site.span.end))
                    .map(|seg| {
                        format!(
                            "; a path segment cannot begin with a digit, so `{seg}` was not \
                             read as part of this reference"
                        )
                    })
                    .unwrap_or_default();
                refusals.push(Refusal::new(
                    path.clone(),
                    site.span,
                    DiagCode::DanglingRef,
                    format!(
                        "`@{}` does not resolve to any file (looked for `{target}`){hint}",
                        site.path
                    ),
                ));
            }
        }
    }
    refusals
}

/// `end` is where the lexer's digit-segment rule stopped: on a `/` whose next byte is a digit.
/// Re-inspecting the source there recovers the segment the path lost, e.g. `/1-raw`.
fn digit_segment_hint(text: &str, end: usize) -> Option<String> {
    let b = text.as_bytes();
    if b.get(end) != Some(&b'/') || !b.get(end + 1).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut j = end + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'-')) {
        j += 1;
    }
    Some(text[end..j].to_string())
}

#[derive(Clone, Copy, PartialEq)]
enum Color {
    White,
    Gray,
    Black,
}

/// A node's resolved, in-graph edges (dangling refs are already refused before this runs).
fn resolved_refs<'a>(node: &str, nodes: &'a BTreeMap<String, Expr>) -> Vec<Edge<'a>> {
    let mut raw = Vec::new();
    if let Some(expr) = nodes.get(node) {
        collect_refs(expr, &mut raw);
    }
    raw.into_iter()
        .filter_map(|site| {
            let target = resolve_ref_path(node, &site.path)?;
            nodes.get_key_value(&target).map(|(k, _)| Edge {
                target: k.as_str(),
                span: site.span,
                at_t: site.at_t,
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
struct Edge<'a> {
    target: &'a str,
    span: ByteSpan,
    at_t: bool,
}

struct Frame<'a> {
    node: &'a str,
    refs: Vec<Edge<'a>>,
    idx: usize,
}

/// An explicit-stack DFS: a hostile, arbitrarily long acyclic ref chain walks in a heap-sized
/// `Vec`, never the OS call stack. It walks ONLY the reads written at bare `t`, so a cycle in
/// it carries no delay at any sample rate; one holding a shifted read is the engine's to settle.
fn check_cycles(nodes: &BTreeMap<String, Expr>) -> Vec<Refusal> {
    let mut color: HashMap<&str, Color> =
        nodes.keys().map(|k| (k.as_str(), Color::White)).collect();
    let mut refusals = Vec::new();
    let undelayed = |node: &str| -> Vec<Edge<'_>> {
        resolved_refs(node, nodes)
            .into_iter()
            .filter(|e| e.at_t)
            .collect()
    };

    for start in nodes.keys().map(String::as_str) {
        if color[start] != Color::White {
            continue;
        }
        color.insert(start, Color::Gray);
        let mut stack = vec![Frame {
            node: start,
            refs: undelayed(start),
            idx: 0,
        }];

        while let Some(frame) = stack.last_mut() {
            if frame.idx >= frame.refs.len() {
                color.insert(frame.node, Color::Black);
                stack.pop();
                continue;
            }
            let edge = frame.refs[frame.idx];
            let from = frame.node;
            frame.idx += 1;
            match color.get(edge.target) {
                Some(Color::Gray) => refusals.push(Refusal::new(
                    from,
                    edge.span,
                    DiagCode::RefCycle,
                    format!(
                        "a delay-free ref cycle: `{from}` reads `@{}` at its own `t`, and the \
                         reads leading back to `{from}` are all at `t` too, so this value is its \
                         own argument. A loop is well-founded only when some read on it reaches \
                         strictly into the past",
                        edge.target
                    ),
                )),
                Some(Color::White) => {
                    color.insert(edge.target, Color::Gray);
                    stack.push(Frame {
                        node: edge.target,
                        refs: undelayed(edge.target),
                        idx: 0,
                    });
                }
                _ => {}
            }
        }
    }
    refusals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    /// The graph's own semantics have nothing to do with a filesystem, so they are stated over
    /// the in-memory form and a directory is tested where it is read (`fixtures.rs`).
    fn of(files: &[(&str, &str)]) -> crate::source::Composition {
        files.iter().copied().collect()
    }

    /// A source that counts what was asked of it: the pull property is not observable from the
    /// graph, only from what the source was never opened for.
    struct Counting {
        held: crate::source::Composition,
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl Source for Counting {
        fn paths(&self) -> Result<crate::source::Listing, String> {
            panic!("a pull load must never enumerate the source")
        }
        fn get(&self, path: &str) -> Result<Option<std::borrow::Cow<'_, str>>, String> {
            self.asked.borrow_mut().push(path.to_string());
            self.held.get(path)
        }
    }

    #[test]
    fn a_pull_load_never_asks_for_a_node_the_roots_do_not_reach() {
        let source = Counting {
            held: of(&[
                (
                    "song",
                    "sin(2*pi*t)*0.1 + @drums/kick*0.5 + gain(@drums/kick, k=2)\n",
                ),
                ("drums/kick", "sin(2*pi*50*t)\n"),
                ("gain", "x*k*sin(2*pi*t)\n"),
                ("zombie", "@nowhere\n"),
            ]),
            asked: std::cell::RefCell::new(Vec::new()),
        };
        let g = load_reaching(&source, &["song"]).expect("the reached set is whole");
        let mut reached: Vec<&str> = g.paths().collect();
        reached.sort();
        assert_eq!(reached, ["drums/kick", "gain", "song"]);
        let asked = source.asked.borrow().clone();
        assert!(
            !asked.contains(&"zombie".to_string()),
            "an unreferenced node was read: {asked:?}"
        );
        assert!(
            asked.contains(&"gain".to_string()),
            "a bareword invocation names a file too: {asked:?}"
        );
        let mut once = asked.clone();
        once.sort();
        once.dedup();
        assert_eq!(
            once.len(),
            asked.len(),
            "`sin` is called from two root nodes and must cost one read: {asked:?}"
        );
    }

    /// Nodes arriving one at a time is the browser's shape, and a set still short of whole is
    /// the dangling-ref refusal it already was, not a new failure mode.
    #[test]
    fn a_composition_built_node_by_node_refuses_until_the_reached_set_is_whole() {
        let mut c = crate::source::Composition::new();
        c.insert("song", "@drums/kick*0.5\n");
        let errs = load_reaching(&c, &["song"]).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::DanglingRef));

        c.insert("drums/kick", "sin(2*pi*50*t)\n");
        let g = load_reaching(&c, &["song"]).expect("the last node arrived");
        assert!(g.expr("drums/kick").is_some());
    }

    /// The lexer's digit-segment rule truncates `@../growl/1-raw` to `@../growl`, and the
    /// dangling ref must say so, not just "looked for `growl`".
    #[test]
    fn a_dangling_ref_truncated_by_the_digit_segment_rule_names_what_fell_off() {
        let g = of(&[
            ("tracks/melody", "@../growl/1-raw\n"),
            ("growl/other", "0.2\n"),
        ]);
        let errs = load(&g).unwrap_err();
        let err = errs
            .iter()
            .find(|r| r.code == DiagCode::DanglingRef)
            .expect("the truncated ref must dangle");
        assert!(err.reason.contains("looked for `growl`"), "{}", err.reason);
        assert!(
            err.reason.contains("digit") && err.reason.contains("/1-raw"),
            "must explain the truncation, not just report `growl`: {}",
            err.reason
        );
    }

    #[test]
    fn cross_file_refs_resolve() {
        let files: Vec<(&str, &str)> = vec![("kick", "sin(2*pi*50*t)\n"), ("lead", "@kick*0.5\n")];
        let g = load(&of(&files)).unwrap();
        assert!(g.expr("kick").is_some());
        assert!(g.defines("lead"));
    }

    #[test]
    fn same_file_cycle_is_refused() {
        let files: Vec<(&str, &str)> = vec![("a", "@a + 1\n")];
        let errs = load(&of(&files)).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));
    }

    #[test]
    fn a_node_merely_referencing_a_cycle_member_is_not_itself_flagged() {
        let files: Vec<(&str, &str)> = vec![("a", "@b\n"), ("b", "@a\n"), ("c", "@b\n")];
        let errs = load(&of(&files)).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));
        assert!(
            !errs.iter().any(|r| r.at.path == "c"),
            "c only reaches into the cycle, it does not loop back to itself: {errs:?}"
        );
    }

    /// The relaxation this file owns: a loop with a shifted read on it is left for the engine,
    /// which alone knows how many samples that shift is worth.
    #[test]
    fn a_mutual_cycle_with_a_shifted_read_on_it_parses() {
        let files: Vec<(&str, &str)> =
            vec![("a", "@b(t - 0.01s)*0.5\n"), ("b", "@a*0.5 + sin(t)\n")];
        let g = load(&of(&files)).unwrap();
        assert!(g.expr("a").is_some() && g.expr("b").is_some());

        let files: Vec<(&str, &str)> = vec![("ring", "@ring(t - 0.002s)*0.9 + sin(t)\n")];
        assert!(load(&of(&files)).is_ok());
    }

    /// And the half that still refuses, located, whatever the sample rate turns out to be.
    #[test]
    fn a_cycle_read_only_at_bare_t_is_refused_where_the_read_is_written() {
        let files: Vec<(&str, &str)> =
            vec![("a", "@b(t - 0.01s)\n"), ("b", "@c\n"), ("c", "@b + 1\n")];
        let errs = load(&of(&files)).unwrap_err();
        let r = errs
            .iter()
            .find(|r| r.code == DiagCode::RefCycle)
            .expect("b <-> c carries no delay");
        assert!(r.at.path == "b" || r.at.path == "c", "{:?}", r.at.path);
        assert!(r.at.span.end > r.at.span.start);
    }

    /// Reaching a node down a delayed leg first must not settle it: only the undelayed reads
    /// are a graph here, and `a -> c -> d -> a` is a loop in it whichever way `d` was found.
    #[test]
    fn a_delay_free_leg_is_found_even_when_a_delayed_one_reaches_it_first() {
        let mut files: Vec<(&str, &str)> = vec![
            ("a", "@b + @c\n"),
            ("b", "@d(t - 0.01s)\n"),
            ("c", "@d\n"),
            ("d", "@a\n"),
        ];
        let errs = load(&of(&files)).unwrap_err();
        assert!(
            errs.iter().any(|r| r.code == DiagCode::RefCycle),
            "a -> c -> d -> a is delay-free: {errs:?}"
        );

        files.push(("c", "@d(t - 0.01s)\n"));
        assert!(
            load(&of(&files)).is_ok(),
            "every leg now carries a shift, so the engine sizes it"
        );
    }

    #[test]
    fn deep_ref_chain_cycle_is_refused() {
        let files: Vec<(&str, &str)> =
            vec![("a", "@b\n"), ("b", "@c\n"), ("c", "@d\n"), ("d", "@a\n")];
        let errs = load(&of(&files)).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));
    }

    /// A parameter list does not make a self-reference well-founded: an invocation is still a
    /// ref, and a ref cycle is still a cycle. `self(...)` stays the one exemption.
    #[test]
    fn a_function_invoking_itself_is_refused_but_self_inside_one_is_not() {
        let files: Vec<(&str, &str)> = vec![("f", "x + @f(t, x=x*0.5)\n")];
        let errs = load(&of(&files)).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));

        let files: Vec<(&str, &str)> = vec![("a", "@b(t, x=x)\n"), ("b", "@a(t, x=x)\n")];
        let errs = load(&of(&files)).unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::RefCycle));

        let files: Vec<(&str, &str)> = vec![("comb", "x + g*self(t - delay)\n")];
        assert!(load(&of(&files)).is_ok());
    }

    #[test]
    fn self_ref_is_not_flagged_a_cycle() {
        let files: Vec<(&str, &str)> = vec![
            ("filt", "0.3*@dry(t) + 0.7*self(t - 1sp)\n"),
            ("dry", "sin(2*pi*220*t)\n"),
        ];
        let g = load(&of(&files)).unwrap();
        assert!(g.expr("filt").is_some());
    }

    #[test]
    fn two_graphs_over_the_same_text_hold_structurally_equal_expressions() {
        let mut files_a: Vec<(&str, &str)> = Vec::new();
        let mut files_b: Vec<(&str, &str)> = Vec::new();
        files_a.push(("kick", "sin(2*pi*50*t)\n"));
        files_b.push(("kick", "sin(2*pi*50*t)\n"));
        let ga = load(&of(&files_a)).unwrap();
        let gb = load(&of(&files_b)).unwrap();

        assert_eq!(ga.expr("kick"), gb.expr("kick"));
    }

    /// A bar literal resolves in the same pass as a filename bar-span, so the two always agree
    /// and the engine below stays unit-free.
    #[test]
    fn resolve_bar_spans_rewrites_bar_literals_everywhere_they_appear() {
        let files: Vec<(&str, &str)> = vec![
            ("kick", "sin(2*pi*50*t)\n"),
            ("swung", "@kick(t - 0.02b)*0.9\n"),
            ("tiled", "crop(@kick(t % 1b), 0s, 16b)\n"),
            ("grid-1b", "@kick\t@kick(t - 0.01b)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();

        assert_eq!(
            g.unresolved_bar_literals(),
            vec!["grid-1b", "swung", "tiled"]
        );

        g.resolve_bar_spans(PerBar {
            seconds: 2.0,
            per: 1.0,
        });
        assert!(g.unresolved_bar_literals().is_empty());
        assert_eq!(
            g.expr("swung"),
            Some(&parse("@kick(t - 0.02*(2/1))*0.9").unwrap())
        );
        assert_eq!(
            g.expr("tiled"),
            Some(&parse("crop(@kick(t % (1*(2/1))), 0s, 16*(2/1))").unwrap())
        );
        assert_eq!(
            g.expr("grid-1b"),
            Some(&parse("@kick + @kick(t - 0.01*(2/1))").unwrap()),
            "a grid's cells are re-materialized, then their bar literals resolved"
        );
    }

    #[test]
    fn a_global_is_read_from_variables_first_and_the_root_second() {
        let files: Vec<(&str, &str)> = vec![
            ("bpm", "120\n"),
            ("variables/bpm", "140\n"),
            ("meter", "4/4\n"),
        ];
        let g = load(&of(&files)).unwrap();

        assert_eq!(g.global("bpm"), Some(&Expr::Lit(Literal::Num(140.0))));
        assert_eq!(
            g.global("meter"),
            Some(&Expr::Lit(Literal::Str("4/4".to_string()))),
            "the root spelling still answers while a composition has not moved"
        );
        assert_eq!(g.global("swing"), None);
        assert!(
            g.expr("variables/bpm").is_some(),
            "a global is an ordinary node, referenceable as @variables/bpm"
        );
    }

    #[test]
    fn resolve_bar_spans_converts_only_bars_units_to_seconds() {
        let files: Vec<(&str, &str)> = vec![
            ("drums/kick", "sin(2*pi*50*t)\n"),
            ("drums/pattern-2b", "@kick\n"),
            ("drums/tail-1.5s", "@kick\n"),
            ("drums/plain", "@kick\n"),
        ];
        let mut g = load(&of(&files)).unwrap();

        g.resolve_bar_spans(PerBar {
            seconds: 0.5,
            per: 1.0,
        });

        assert_eq!(
            g.span("drums/pattern-2b"),
            Some(crate::filename::FileSpan {
                amount: 1.0,
                unit: crate::filename::SpanUnit::Seconds,
            })
        );
        assert_eq!(
            g.span("drums/tail-1.5s"),
            Some(crate::filename::FileSpan {
                amount: 1.5,
                unit: crate::filename::SpanUnit::Seconds,
            })
        );
        assert_eq!(g.span("drums/plain"), None);
    }

    /// The defect this pins: row shifts once baked in at parse time as raw bar-numbers while
    /// the outer `crop` window was tempo-scaled, so the two disagreed by `seconds_per_bar`.
    #[test]
    fn resolve_bar_spans_rescales_a_grids_row_shifts_to_match_its_crop_window() {
        let files: Vec<(&str, &str)> = vec![
            ("kick", "sin(2*pi*50*t)\n"),
            ("pattern-1b", "@kick\n@kick\n@kick\n@kick\n"),
            ("track", "repeat(@pattern-1b, 2)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();

        g.resolve_bar_spans(PerBar {
            seconds: 2.0,
            per: 1.0,
        });
        let mut rows = Vec::new();
        flatten_sum(g.expr("pattern-1b").unwrap(), &mut rows);
        let shifts: Vec<f64> = rows.iter().map(ref_shift).collect();
        assert_eq!(
            shifts,
            vec![0.0, 0.5, 1.0, 1.5],
            "four rows must span the bar's full 2 resolved seconds"
        );

        g.desugar_arrangement().unwrap();
        let mut copies = Vec::new();
        flatten_sum(g.expr("track").unwrap(), &mut copies);
        let Expr::Call { args, .. } = &copies[1] else {
            panic!("expected a crop() call")
        };
        assert_eq!(
            literal_num(&args[1]),
            2.0,
            "the second copy starts where the grid's own rows end"
        );
    }

    fn ref_shift(e: &Expr) -> f64 {
        match e {
            Expr::Ref { arg, .. } => match &**arg {
                Expr::Var(_) => 0.0,
                Expr::Bin(BinOp::Sub, _, r) => match &**r {
                    Expr::Lit(Literal::Num(n)) => *n,
                    other => panic!("expected a literal shift, got {other:?}"),
                },
                other => panic!("expected `t` or `t - k`, got {other:?}"),
            },
            other => panic!("expected a Ref, got {other:?}"),
        }
    }

    #[test]
    fn a_ref_resolves_relative_to_its_own_files_directory() {
        let files: Vec<(&str, &str)> = vec![
            ("drums/kick", "sin(2*pi*50*t)\n"),
            ("drums/pattern-1b", "@kick\n"),
        ];
        let g = load(&of(&files)).unwrap();
        assert!(g.expr("drums/pattern-1b").is_some());
    }

    /// A bare path is exactly what it always was; `./` spells that out and `../` walks up.
    #[test]
    fn a_dot_segment_resolves_against_the_referencing_files_own_directory() {
        let at = |file: &str, path: &str| resolve_ref_path(file, path).unwrap();
        assert_eq!(at("lead", "kick"), "kick");
        assert_eq!(at("piano/bench", "partials"), "piano/partials");
        assert_eq!(at("piano/bench", "./partials"), "piano/partials");
        assert_eq!(at("piano/bench", "../variables/bpm"), "variables/bpm");
        assert_eq!(at("a/b/c/d", "../../x"), "a/x");
        assert_eq!(at("song", "./drums/kick"), "drums/kick");
    }

    /// One rule wherever `..` appears, so two spellings of a node are one node.
    #[test]
    fn a_dot_segment_normalizes_in_the_middle_of_a_path_too() {
        let at = |file: &str, path: &str| resolve_ref_path(file, path).unwrap();
        assert_eq!(at("song", "a/x/../b"), at("song", "a/b"));
        assert_eq!(at("song", "a/./b"), "a/b");
        assert_eq!(at("drums/bus", "../lead/./pad/../pad"), "lead/pad");
    }

    #[test]
    fn a_walk_above_the_composition_root_resolves_to_nothing() {
        assert_eq!(resolve_ref_path("piano/bench", "../../x"), None);
        assert_eq!(resolve_ref_path("song", "../x"), None);
        assert_eq!(resolve_ref_path("a/b", "../../../x"), None);
    }

    #[test]
    fn a_ref_reaching_a_sibling_directory_resolves_and_renders_as_one_graph() {
        let files: Vec<(&str, &str)> = vec![
            ("variables/bpm", "120\n"),
            ("piano/partials", "sin(2*pi*440*t)\n"),
            ("piano/bench", "@./partials*0.5 + @../variables/bpm*0\n"),
            ("song", "@piano/bench\n"),
        ];
        let g = load(&of(&files)).unwrap();
        assert!(g.expr("piano/bench").is_some());
    }

    #[test]
    fn a_ref_walking_above_the_root_is_a_located_refusal_naming_it() {
        let files: Vec<(&str, &str)> = vec![("piano/bench", "@../../secrets\n")];
        let errs = load(&of(&files).named("./song1")).unwrap_err();
        let r = errs
            .iter()
            .find(|r| r.code == DiagCode::RefAboveRoot)
            .expect("walking above the root must refuse");
        assert_eq!(r.at.path, "piano/bench");
        assert!(r.reason.contains("@../../secrets"), "{}", r.reason);
        assert!(r.reason.contains("./song1"), "{}", r.reason);
    }

    #[test]
    fn repeat_produces_n_back_to_back_crop_shifted_copies() {
        let files: Vec<(&str, &str)> = vec![
            ("loop-2s", "sin(2*pi*220*t)\n"),
            ("track", "repeat(@loop-2s, 3)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        g.desugar_arrangement().unwrap();

        let expr = g.expr("track").unwrap().clone();
        let mut terms = Vec::new();
        flatten_sum(&expr, &mut terms);
        assert_eq!(terms.len(), 3, "expected 3 distinct copies, got {terms:?}");

        for (idx, term) in terms.iter().enumerate() {
            let Expr::Call { name, args, .. } = term else {
                panic!("expected each term to be a crop() call: {term:?}");
            };
            assert_eq!(name, "crop");
            let start = literal_num(&args[1]);
            let end = literal_num(&args[2]);
            assert_eq!(start, idx as f64 * 2.0);
            assert_eq!(end, (idx as f64 + 1.0) * 2.0);
        }
    }

    #[test]
    fn concat_places_differently_spanned_args_sequentially_with_no_manual_offsets() {
        let files: Vec<(&str, &str)> = vec![
            ("intro-3s", "sin(2*pi*220*t)\n"),
            ("drop-5s", "sin(2*pi*110*t)\n"),
            ("track", "concat(@intro-3s, @drop-5s)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        g.desugar_arrangement().unwrap();

        let expr = g.expr("track").unwrap().clone();
        let mut terms = Vec::new();
        flatten_sum(&expr, &mut terms);
        assert_eq!(terms.len(), 2);

        let Expr::Call { args: a0, .. } = &terms[0] else {
            panic!()
        };
        let Expr::Call { args: a1, .. } = &terms[1] else {
            panic!()
        };
        assert_eq!(literal_num(&a0[1]), 0.0);
        assert_eq!(literal_num(&a0[2]), 3.0);
        assert_eq!(literal_num(&a1[1]), 3.0);
        assert_eq!(literal_num(&a1[2]), 8.0, "3 + 5 total duration");
    }

    #[test]
    fn repeat_with_a_non_integer_count_is_refused_not_guessed() {
        let files: Vec<(&str, &str)> = vec![
            ("loop-1s", "sin(2*pi*220*t)\n"),
            ("track", "repeat(@loop-1s, 2.5)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        let errs = g.desugar_arrangement().unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::BadArrangementArg));
    }

    #[test]
    fn concat_with_a_non_ref_argument_is_refused_not_guessed() {
        let files: Vec<(&str, &str)> = vec![("track", "concat(1 + 2, 3)\n")];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        let errs = g.desugar_arrangement().unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::BadArrangementArg));
    }

    #[test]
    fn concat_before_resolve_bar_spans_is_refused_not_guessed() {
        let files: Vec<(&str, &str)> = vec![
            ("loop-2b", "sin(2*pi*220*t)\n"),
            ("track", "concat(@loop-2b, @loop-2b)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        // Deliberately not calling resolve_bar_spans: the span is still in Bars.
        let errs = g.desugar_arrangement().unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::BadArrangementArg));
    }

    #[test]
    fn concat_with_a_custom_time_arg_ref_is_refused_not_silently_dropped() {
        let files: Vec<(&str, &str)> = vec![
            ("a-1s", "sin(2*pi*220*t)\n"),
            ("b-1s", "sin(2*pi*440*t)\n"),
            ("track", "concat(@a-1s(t*2), @b-1s)\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        let errs = g.desugar_arrangement().unwrap_err();
        assert!(errs.iter().any(|r| r.code == DiagCode::BadArrangementArg));
    }

    #[test]
    fn a_zero_count_repeat_and_an_argument_less_concat_are_silence() {
        let files: Vec<(&str, &str)> = vec![
            ("loop-1s", "sin(2*pi*220*t)\n"),
            ("repeated", "repeat(@loop-1s, 0)\n"),
            ("concatenated", "concat()\n"),
        ];
        let mut g = load(&of(&files)).unwrap();
        g.resolve_bar_spans(PerBar {
            seconds: 1.0,
            per: 1.0,
        });
        g.desugar_arrangement().unwrap();

        assert_eq!(
            *g.expr("repeated").unwrap(),
            Expr::Lit(Literal::Num(0.0)),
            "0 copies is silence, same as an empty TSV grid"
        );
        assert_eq!(
            *g.expr("concatenated").unwrap(),
            Expr::Lit(Literal::Num(0.0))
        );
    }

    fn flatten_sum(e: &Expr, out: &mut Vec<Expr>) {
        match e {
            Expr::Bin(BinOp::Add, l, r) => {
                flatten_sum(l, out);
                flatten_sum(r, out);
            }
            other => out.push(other.clone()),
        }
    }

    fn literal_num(a: &Arg) -> f64 {
        match a {
            Arg::Pos(Expr::Lit(Literal::Num(n))) => *n,
            other => panic!("expected a numeric literal arg, got {other:?}"),
        }
    }
}
