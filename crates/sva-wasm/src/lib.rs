// Concern: the JS surface — a composition a page fills node by node, and what one renders to | Non-concern: the pipeline (sva-core), the store (sva-engine) | IO: (path, text) -> samples or a reading

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

mod opfs;

use sva_core::{
    CliError, Diagnostic, Job, Rendered, Report, SAMPLE_LIMIT, WindowEdge, error_envelope, execute,
    query_data, representation_for, retired, stats_json, window_for,
};
use sva_engine::{
    Buffer, Cache, CacheStats, Horizon, MemoryCache, PSYCHOACOUSTIC_V1, Pack, Representation,
    Slots, Tiered, answer_buffer, ledger_over,
};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = Error)]
    type JsErr;

    #[wasm_bindgen(constructor, js_class = "Error")]
    fn new(message: &str) -> JsErr;

    #[wasm_bindgen(method, setter)]
    fn set_name(this: &JsErr, name: &str);

    #[wasm_bindgen(method, setter)]
    fn set_refusal(this: &JsErr, refusal: JsValue);

    #[wasm_bindgen(js_namespace = JSON, catch)]
    fn parse(text: &str) -> Result<JsValue, JsValue>;
}

/// One crossing for every refusal: `name` is the CLI's own error code and `refusal` the
/// envelope it would have printed, never null — a page reads a slip as text, not absence.
fn crossed(code: &str, message: &str, diagnostics: &[Diagnostic]) -> JsValue {
    let error = JsErr::new(message);
    error.set_name(code);
    let envelope = error_envelope(code, message, diagnostics);
    error.set_refusal(parse(&envelope).unwrap_or_else(|_| JsValue::from_str(&envelope)));
    error.into()
}

fn thrown(refusal: &CliError) -> JsValue {
    crossed(refusal.code(), &refusal.message(), &refusal.diagnostics())
}

/// A page has no argv and no shell, so a refusal raised here carries its own repair rather
/// than the CLI's "run `sva-cli --help`".
fn refuse(message: String, help: &str) -> JsValue {
    let diagnostic = Diagnostic::new("wasm.bad_argument", message.clone()).helped(help);
    crossed("validation_error", &message, &[diagnostic])
}

/// Nothing in a browser pushes back when a store grows inside the tab's own address space.
const DEFAULT_CACHE_BYTES: u64 = 256 << 20;
const DEFAULT_PERSISTENT_BYTES: u64 = 512 << 20;

enum Store {
    Memory(MemoryCache),
    Persistent(Box<Tiered<opfs::OpfsMedium>>),
}

impl Store {
    fn cache(&self) -> &dyn Cache {
        match self {
            Store::Memory(memory) => memory,
            Store::Persistent(tiered) => tiered.as_ref(),
        }
    }

    fn memory(&self) -> &MemoryCache {
        match self {
            Store::Memory(memory) => memory,
            Store::Persistent(tiered) => &tiered.front,
        }
    }

    fn pack(&self) -> Option<&dyn Cache> {
        match self {
            Store::Memory(_) => None,
            Store::Persistent(tiered) => Some(&tiered.back),
        }
    }

    fn remember_in(&mut self, memory: MemoryCache) {
        match self {
            Store::Memory(held) => *held = memory,
            Store::Persistent(tiered) => tiered.front = memory,
        }
    }
}

/// `sva-cli outline`'s `data`.
#[wasm_bindgen]
pub fn outline(text: &str) -> Result<JsValue, JsValue> {
    parse(&sva_core::outline_data(text).map_err(|e| thrown(&e))?)
}

#[wasm_bindgen]
pub struct Composition {
    inner: sva_ast::Composition,
    store: Store,
    slots: Slots,
}

#[wasm_bindgen]
impl Composition {
    #[wasm_bindgen(constructor)]
    pub fn new(name: Option<String>) -> Composition {
        let inner = sva_ast::Composition::new();
        Composition {
            inner: match name {
                Some(name) => inner.named(name),
                None => inner,
            },
            store: Store::Memory(MemoryCache::holding(DEFAULT_CACHE_BYTES)),
            slots: Slots::default(),
        }
    }

    /// `name` labels the composition as `new` does; renders persist under `cache` in a dedicated
    /// worker's private file system, and anywhere none opens, or `cache` is held, memory-only.
    #[wasm_bindgen(js_name = persistent)]
    pub async fn open_persistent(
        name: Option<String>,
        cache: String,
        max_bytes: Option<f64>,
    ) -> Result<Composition, JsValue> {
        if cache.is_empty() || cache == "." || cache == ".." || cache.contains(['/', '\\']) {
            return Err(refuse(
                format!("`{cache}` cannot name a directory"),
                "name the cache with one path component",
            ));
        }
        let mut held = Composition::new(name);
        if let Some(medium) = opfs::open(&cache).await {
            let cap = max_bytes.map_or(DEFAULT_PERSISTENT_BYTES, |b| b.max(0.0) as u64);
            held.store = Store::Persistent(Box::new(Tiered::new(
                MemoryCache::holding(DEFAULT_CACHE_BYTES),
                Pack::open(medium, cap),
            )));
        }
        Ok(held)
    }

    #[wasm_bindgen(getter = persistent)]
    pub fn is_persistent(&self) -> bool {
        matches!(self.store, Store::Persistent(_))
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.inner.insert(path, text);
    }

    /// Unset, `target` is `master`; `seconds` is the horizon and `rate` the observation rate.
    /// What reads a `volatile` parameter is kept in a slot, never in either store.
    pub fn render(
        &self,
        target: Option<String>,
        rate: Option<u32>,
        seconds: Option<f64>,
        volatile: Option<Vec<String>>,
    ) -> Result<Rendering, JsValue> {
        let volatile = volatile.unwrap_or_default();
        let rendered = execute(Job {
            target: target.as_deref(),
            until: seconds.map(WindowEdge::Secs),
            sample_rate: rate,
            reaching: true,
            cache: Some(self.store.cache()),
            volatile: &volatile,
            slots: Some(&self.slots),
            ..Job::over(&self.inner)
        });
        rendered
            .map(|inner| Rendering { inner })
            .map_err(|e| thrown(&e))
    }

    /// The memory tier's, as `cache_max_bytes` is; `persistent_bytes` is the pack's.
    #[wasm_bindgen(getter)]
    pub fn cache_bytes(&self) -> f64 {
        self.store.memory().held_bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_max_bytes(&self) -> f64 {
        self.store.memory().max_bytes() as f64
    }

    /// Zero on a memory-only composition, as `persistent_max_bytes` is.
    #[wasm_bindgen(getter)]
    pub fn persistent_bytes(&self) -> f64 {
        self.store.pack().map_or(0.0, |p| p.held_bytes() as f64)
    }

    #[wasm_bindgen(getter)]
    pub fn persistent_max_bytes(&self) -> f64 {
        self.store.pack().map_or(0.0, |p| p.max_bytes() as f64)
    }

    pub fn bound_cache(&mut self, max_bytes: f64) {
        self.store
            .remember_in(MemoryCache::holding(max_bytes.max(0.0) as u64));
    }

    #[wasm_bindgen(getter)]
    pub fn volatile_bytes(&self) -> f64 {
        self.slots.held_bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn volatile_max_bytes(&self) -> f64 {
        self.slots.max_bytes() as f64
    }

    #[wasm_bindgen(setter)]
    pub fn set_volatile_max_bytes(&self, max_bytes: f64) {
        self.slots.bound(max_bytes.max(0.0) as u64);
    }

    pub fn clear_volatile(&self) {
        self.slots.clear();
    }

    /// Empties the memory tier only: a persistent pack outlives it by design.
    pub fn clear_cache(&mut self) {
        let max_bytes = self.store.memory().max_bytes();
        self.store.remember_in(MemoryCache::holding(max_bytes));
    }
}

#[wasm_bindgen]
pub struct Rendering {
    inner: Rendered,
}

#[wasm_bindgen]
impl Rendering {
    #[wasm_bindgen(getter)]
    pub fn target(&self) -> String {
        self.inner.target.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.config.rate
    }

    #[wasm_bindgen(getter)]
    pub fn duration_secs(&self) -> f64 {
        self.inner.config.horizon.span()
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.buffer().map_or(0, |b| b.width)
    }

    pub fn samples(
        &self,
        channel: usize,
        from_secs: Option<f64>,
        to_secs: Option<f64>,
    ) -> Result<Vec<f32>, JsValue> {
        let over = self.checked(from_secs, to_secs)?;
        let buffer = self.buffer().ok_or_else(|| {
            refuse(
                format!(
                    "`{}` answered `samples` with something that is not audio",
                    self.inner.target
                ),
                "ask for a node that renders to samples",
            )
        })?;
        if channel >= buffer.width {
            return Err(refuse(
                format!(
                    "`{}` is {} component(s), so there is no component {channel}",
                    self.inner.target, buffer.width
                ),
                "read a component this node holds, counting from zero",
            ));
        }
        let held = buffer.as_f32(channel);
        Ok(held[buffer.span_of(over)].to_vec())
    }

    /// The object `sva-cli render` puts under `data.cache.stats`.
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let none = CacheStats::default();
        parse(&stats_json(
            self.inner.render.cache_stats.as_ref().unwrap_or(&none),
        ))
    }

    /// The object `sva-cli render --as <name>` puts under `data`, arrays capped as it caps.
    pub fn query(
        &self,
        representation: &str,
        from_secs: Option<f64>,
        to_secs: Option<f64>,
    ) -> Result<JsValue, JsValue> {
        let asked = self.checked(from_secs, to_secs)?;
        if let Some(write) = retired(representation) {
            return Err(refuse(
                format!("`{representation}` left the language"),
                &format!("ask for `{write}`"),
            ));
        }
        let wanted =
            representation_for(representation, sva_core::Shaping::default()).ok_or_else(|| {
                refuse(
                    format!("unknown representation `{representation}`"),
                    "ask for one of the readings this rendering answers",
                )
            })?;
        let whole = self.inner.config.horizon;
        let narrowed = self.narrowed(asked);
        let whole_horizon = narrowed.is_none();
        let engine = |e| thrown(&CliError::Engine(e));
        let (answer, over) = match (narrowed, wanted) {
            (None, _) => (
                self.inner
                    .answer(&self.inner.target, wanted)
                    .map_err(|e| thrown(&e))?,
                whole,
            ),
            (Some((_, over)), Representation::Ledger { depth }) => {
                let node = self.inner.render.node(&self.inner.target).map_err(engine)?;
                (
                    ledger_over(&self.inner.render, node, depth, over).map_err(engine)?,
                    over,
                )
            }
            (Some((buffer, over)), _) => (
                answer_buffer(&self.inner.target, &buffer, wanted, PSYCHOACOUSTIC_V1.name)
                    .map_err(engine)?,
                over,
            ),
        };
        let answers = [(representation.to_string(), answer)];
        parse(&query_data(&Report {
            target: &self.inner.target,
            rate: self.inner.config.rate,
            horizon: over,
            profile: self.inner.config.profile.name,
            label: self.inner.label().filter(|_| whole_horizon),
            written: &[],
            cache: None,
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
            skim: false,
        }))
    }

    /// The window a reading is narrowed to, where it narrows the render's own at all. A closed form
    /// has no window, so it answers over the whole horizon and this is `None`.
    fn narrowed(&self, asked: Horizon) -> Option<(Buffer, Horizon)> {
        let whole = self.inner.config.horizon;
        let end = match asked.end_secs.is_finite() {
            true => asked.end_secs.min(whole.end_secs),
            false => whole.end_secs,
        };
        let start = asked.start_secs.max(whole.start_secs);
        if start <= whole.start_secs && end >= whole.end_secs {
            return None;
        }
        let over = Horizon::secs(start, end);
        let buffer = self.buffer()?;
        let taken = buffer.span_of(over);
        let mut held = Buffer::of_planes(
            buffer.rate,
            (0..buffer.width)
                .map(|c| buffer.plane(c)[taken.clone()].to_vec())
                .collect(),
        );
        held.origin_secs = over.start_secs;
        Some((held, over))
    }

    /// The window a caller named, held to what this rendering actually covers: an inverted
    /// one and one past the horizon are refused here exactly as they are on argv.
    fn checked(&self, from_secs: Option<f64>, to_secs: Option<f64>) -> Result<Horizon, JsValue> {
        let asked = window_for(
            from_secs.map(WindowEdge::Secs),
            to_secs.map(WindowEdge::Secs),
            None,
        )
        .map_err(|e| thrown(&e))?;
        let whole = self.inner.config.horizon;
        let end = match asked.end_secs.is_finite() {
            true => asked.end_secs,
            false => whole.end_secs,
        };
        if end <= asked.start_secs {
            return Err(refuse(
                format!("a window from {}s to {end}s is no window", asked.start_secs),
                "give an end past the start",
            ));
        }
        if asked.start_secs < whole.start_secs || end > whole.end_secs {
            return Err(refuse(
                format!(
                    "`{}` runs {}s to {}s, and the window asked for runs {}s to {end}s",
                    self.inner.target, whole.start_secs, whole.end_secs, asked.start_secs
                ),
                "render for longer, or ask for a window inside this one",
            ));
        }
        Ok(Horizon::secs(asked.start_secs, end))
    }

    fn buffer(&self) -> Option<&Buffer> {
        let id = self.inner.render.id(&self.inner.target)?;
        self.inner.render.buffer(id)
    }
}
