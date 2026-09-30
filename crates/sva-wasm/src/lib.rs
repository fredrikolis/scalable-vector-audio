// Concern: the JS surface — a composition a page fills node by node, renders, streams, persists | Non-concern: the pipeline (sva-core), the store's medium | IO: (path, text) -> samples, blocks, stores

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

mod opfs;

pub use opfs::DirectoryHandle;

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use sva_core::{
    Asked, CliError, Diagnostic, Job, Printed, Rendered, Report, SAMPLE_LIMIT, error_envelope,
    execute, execute_through, query_data, stats_json, stream_stats_json, work_json,
};
use sva_engine::{
    Buffer, Cache, CachePolicy, CacheStats, DEFAULT_STORE_BYTES, Extent, Handle, Hash, PrunePolicy,
    Store, Stored, Through,
};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};

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
/// envelope it would have printed, never null.
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

#[derive(Default)]
struct Options {
    rate: Option<u32>,
    bits: Option<i32>,
    flop_budget: Option<u128>,
    until: Option<String>,
    volatile: Vec<String>,
    cache: Option<CachePolicy>,
    live: bool,
}

/// `keys` are the options this call reads; any other is refused by name.
fn options_of(options: &JsValue, keys: &[&str]) -> Result<Options, JsValue> {
    let mut held = Options::default();
    if options.is_undefined() || options.is_null() {
        return Ok(held);
    }
    let object = options.dyn_ref::<js_sys::Object>().ok_or_else(|| {
        refuse(
            "`options` is not an object".into(),
            "pass an object, such as { rate: 48000 }",
        )
    })?;
    for entry in js_sys::Object::entries(object).iter() {
        let pair = js_sys::Array::from(&entry);
        let key = pair.get(0).as_string().unwrap_or_default();
        let value = pair.get(1);
        if !keys.contains(&key.as_str()) {
            return Err(refuse(
                format!("`{key}` is no option here"),
                &format!("the options are {}", keys.join(", ")),
            ));
        }
        match key.as_str() {
            "rate" => held.rate = Some(whole(&key, &value)?),
            "bits" => held.bits = Some(whole(&key, &value)?),
            "flop_budget" => held.flop_budget = Some(whole(&key, &value)?),
            "until" => held.until = Some(text(&key, &value)?),
            "cache" => held.cache = Some(cache_policy(&text(&key, &value)?)?),
            "live" => {
                held.live = value
                    .as_bool()
                    .ok_or_else(|| refuse("`live` is not a boolean".into(), "pass true or false"))?
            }
            _ => {
                let names = value.dyn_ref::<js_sys::Array>().ok_or_else(|| {
                    refuse(
                        "`volatile` is not an array".into(),
                        "pass an array of names",
                    )
                })?;
                for name in names.iter() {
                    held.volatile.push(name.as_string().ok_or_else(|| {
                        refuse(
                            "`volatile` holds something that is not a name".into(),
                            "pass an array of names",
                        )
                    })?);
                }
            }
        }
    }
    Ok(held)
}

fn whole<N: TryFrom<u64>>(key: &str, value: &JsValue) -> Result<N, JsValue> {
    value
        .as_f64()
        .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= 2f64.powi(53))
        .and_then(|v| N::try_from(v as u64).ok())
        .ok_or_else(|| {
            refuse(
                format!("`{key}` is not a whole number in range"),
                "pass a whole number",
            )
        })
}

fn text(key: &str, value: &JsValue) -> Result<String, JsValue> {
    value.as_string().ok_or_else(|| {
        refuse(
            format!("`{key}` is not a string"),
            "pass it as a string, as `sva-cli` writes it",
        )
    })
}

/// Each entry one call, as `sva-cli`'s `--representation` writes it: `spectrum(peaks=8)`.
/// A page holds no file to write one to.
fn representations_of(names: &[String]) -> Result<Vec<Asked>, JsValue> {
    names
        .iter()
        .map(|name| {
            let call = sva_core::call(name).map_err(|e| thrown(&e))?;
            if call.dest.is_some() {
                return Err(refuse(
                    format!("`{name}` names a destination, and a page writes no file"),
                    "drop the `=path`, and read it from `representations()`",
                ));
            }
            sva_core::asked(&call).map_err(|e| thrown(&e))
        })
        .collect()
}

/// Nothing in a browser pushes back when a store grows inside the tab's own address space.
const DEFAULT_CACHE_BYTES: u64 = 256 << 20;

#[wasm_bindgen]
pub fn builtins() -> Result<JsValue, JsValue> {
    parse(&sva_core::builtins_data(&sva_core::builtins()))
}

/// `sva-cli outline`'s `data`.
#[wasm_bindgen]
pub fn outline(text: &str) -> Result<JsValue, JsValue> {
    parse(&sva_core::outline_data(text).map_err(|e| thrown(&e))?)
}

#[wasm_bindgen]
pub struct Composition {
    inner: sva_ast::Composition,
    store: Cache,
    persistent: Page,
}

/// The store a page opened its composition over, if any: what a render or a stream reads.
#[derive(Clone, Default)]
struct Page(Option<Rc<Store<opfs::Opfs>>>);

impl Through for Page {
    async fn lookup(&self, key: Hash) -> Option<Stored> {
        match &self.0 {
            Some(store) => Through::lookup(&**store, key).await,
            None => None,
        }
    }

    async fn read(&self, stored: &Stored, over: Extent) -> Option<Vec<Buffer>> {
        match &self.0 {
            Some(store) => Through::read(&**store, stored, over).await,
            None => None,
        }
    }
}

fn unstored(why: String) -> JsValue {
    let message = format!("the store could not be read or written: {why}");
    let diagnostic = Diagnostic::new("wasm.store", message.clone())
        .helped("check the directory handle is writable, or open with none for memory only");
    crossed("internal_error", &message, &[diagnostic])
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
            store: Cache::holding(DEFAULT_CACHE_BYTES),
            persistent: Page::default(),
        }
    }

    /// Over the store in `dir`, an origin-private file system directory, or memory alone
    /// where there is none.
    pub async fn open(
        name: Option<String>,
        dir: Option<DirectoryHandle>,
    ) -> Result<Composition, JsValue> {
        let mut held = Composition::new(name);
        if let Some(dir) = dir {
            let backend = opfs::Opfs { dir };
            let store = Store::open(backend, DEFAULT_STORE_BYTES).await;
            held.persistent = Page(Some(Rc::new(store.map_err(unstored)?)));
        }
        Ok(held)
    }

    /// The only write to the directory `open` was handed: every value computed since the last.
    pub async fn persist(&self) -> Result<usize, JsValue> {
        let Some(store) = &self.persistent.0 else {
            return Ok(0);
        };
        store
            .persist()
            .await
            .map(|done| done.written)
            .map_err(unstored)
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.inner.insert(path, text);
    }

    /// `target` as `sva-cli render` takes it, `@piano([0, 2b], f0=C4)`; `representations`
    /// what `representations()` answers, `samples` where unset, each a call as
    /// `--representation` writes it. `options` sets `rate`, `bits`, `flop_budget`, `until`,
    /// `volatile` and `cache`.
    pub async fn render(
        &self,
        target: &str,
        representations: Option<Vec<String>>,
        options: JsValue,
    ) -> Result<Rendering, JsValue> {
        let options = options_of(
            &options,
            &["rate", "bits", "flop_budget", "until", "volatile", "cache"],
        )?;
        let names = representations.unwrap_or_else(|| vec!["samples".to_string()]);
        let asked = representations_of(&names)?;
        let job = Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            cache: Some(&self.store),
            cache_policy: options.cache,
            asked: &asked,
            flop_budget: options.flop_budget,
            volatile: &options.volatile,
            ..Job::over(&self.inner, target)
        };
        let rendered = match self.persistent.0.as_deref() {
            Some(store) => execute_through(job, store).await,
            None => execute(job),
        };
        rendered
            .map(|inner| Rendering { inner, asked })
            .map_err(|e| thrown(&e))
    }

    /// `target` block by block, through this store. `options`: `rate`, `bits`, `until`, `cache`,
    /// `live`. Opening and every edit look the store on disk up, so each is awaited.
    pub async fn stream(
        &self,
        target: &str,
        block: usize,
        options: JsValue,
    ) -> Result<Stream, JsValue> {
        let options = options_of(&options, &["rate", "bits", "until", "cache", "live"])?;
        let job = Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            cache: Some(&self.store),
            cache_policy: options.cache,
            ..Job::over(&self.inner, target)
        };
        let opened = sva_core::stream(&job, block, &self.persistent).await;
        let mut inner = opened.map_err(|e| thrown(&e))?;
        if options.live {
            inner.go_live();
        }
        Ok(Stream(RefCell::new(Some(Playing {
            inner,
            source: self.inner.clone(),
            store: self.persistent.clone(),
        }))))
    }

    #[wasm_bindgen(getter)]
    pub fn cache_bytes(&self) -> f64 {
        self.store.bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_max_bytes(&self) -> f64 {
        self.store.max_bytes() as f64
    }

    #[wasm_bindgen(setter)]
    pub fn set_cache_max_bytes(&self, max_bytes: f64) {
        self.store.set_max_bytes(max_bytes.max(0.0) as u64);
    }

    #[wasm_bindgen(getter)]
    pub fn cache_entries(&self) -> usize {
        self.store.entries()
    }

    #[wasm_bindgen(getter)]
    pub fn cache_evictions(&self) -> f64 {
        self.store.evictions() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_policy(&self) -> String {
        self.store.policy().name().to_string()
    }

    pub fn set_cache_policy(&self, policy: &str) -> Result<(), JsValue> {
        self.store.set_policy(cache_policy(policy)?);
        Ok(())
    }

    #[wasm_bindgen(getter)]
    pub fn prune_policy(&self) -> String {
        self.store.prune_policy().name().to_string()
    }

    pub fn set_prune_policy(&self, policy: &str) -> Result<(), JsValue> {
        self.store.set_prune_policy(prune_policy(policy)?);
        Ok(())
    }

    /// `"oldest"` evicts what the latest render neither stored nor read, `"forks"` every value
    /// fewer than two nodes read.
    pub fn prune(&self, policy: &str) -> Result<(), JsValue> {
        self.store.prune(prune_policy(policy)?);
        Ok(())
    }

    pub fn clear_cache(&self) {
        self.store.clear();
    }
}

fn cache_policy(name: &str) -> Result<CachePolicy, JsValue> {
    CachePolicy::named(name).ok_or_else(|| {
        refuse(
            format!("`{name}` names no cache policy"),
            "pass \"all\", \"forks\", \"target\" or \"none\"",
        )
    })
}

fn prune_policy(name: &str) -> Result<PrunePolicy, JsValue> {
    PrunePolicy::named(name).ok_or_else(|| {
        refuse(
            format!("`{name}` names no prune policy"),
            "pass \"oldest\" or \"forks\"",
        )
    })
}

#[wasm_bindgen]
pub struct Rendering {
    inner: Rendered,
    asked: Vec<Asked>,
}

#[wasm_bindgen]
impl Rendering {
    #[wasm_bindgen(getter)]
    pub fn target(&self) -> String {
        self.inner.expression.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.config.rate
    }

    #[wasm_bindgen(getter)]
    pub fn start_secs(&self) -> f64 {
        let rate = self.inner.config.rate;
        self.inner.render.range.map_or(0.0, |r| r.start_secs(rate))
    }

    #[wasm_bindgen(getter)]
    pub fn duration_secs(&self) -> f64 {
        let rate = self.inner.config.rate;
        self.inner.render.range.map_or(0.0, |r| r.span_secs(rate))
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        let render = &self.inner.render;
        render.buffer(render.root).map_or(0, |b| b.width)
    }

    pub fn samples(&self, channel: usize) -> Result<Vec<f32>, JsValue> {
        let render = &self.inner.render;
        if render.buffer(render.root).is_none() {
            return Err(refuse(
                format!("`{}` read no samples of its root", self.inner.expression),
                "ask for `samples` among the representations",
            ));
        }
        let buffer = render
            .output(render.root)
            .map_err(|e| thrown(&CliError::Engine(e)))?;
        if channel >= buffer.width {
            return Err(refuse(
                format!(
                    "`{}` is {} component(s), so there is no component {channel}",
                    self.inner.expression, buffer.width
                ),
                "read a component this node holds, counting from zero",
            ));
        }
        sva_core::encode::float32(&self.inner.expression, buffer.plane(channel))
            .map_err(|e| thrown(&e))
    }

    /// Every lookup this render made of the composition's store, and what each came to.
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let none = CacheStats::default();
        parse(&stats_json(
            self.inner.render.cache_stats.as_ref().unwrap_or(&none),
        ))
    }

    pub fn work(&self) -> Result<JsValue, JsValue> {
        parse(&work_json(&self.inner.render.work()))
    }

    /// The object `sva-cli render` puts under `data`, one reading per representation asked,
    /// arrays capped as it caps.
    pub fn representations(&self) -> Result<JsValue, JsValue> {
        let mut answers = Vec::with_capacity(self.asked.len());
        for asked in &self.asked {
            let node = asked.node.as_deref().unwrap_or(&self.inner.target);
            let answer = self
                .inner
                .answer(node, asked.representation)
                .map_err(|e| thrown(&e))?;
            answers.push(Printed {
                name: asked.name.clone(),
                answer,
                skim: asked.skim,
            });
        }
        let rate = self.inner.config.rate;
        let interval = self
            .inner
            .render
            .range
            .map(|r| (r.start_secs(rate), r.end as f64 / f64::from(rate)));
        parse(&query_data(&Report {
            target: &self.inner.expression,
            rate,
            bits: Some(self.inner.config.profile.precision_bits),
            interval,
            profile: self.inner.config.profile.name,
            label: self.inner.label(),
            written: &[],
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
        }))
    }
}

/// A target block by block, reading `@notes` as the sum of its terms, as `@hall(t, x=@notes)`.
/// A key-up replaces a term with one whose release is a number. A term leaves with its handle
/// once its node ends, bar the last.
#[wasm_bindgen]
pub struct Stream(RefCell<Option<Playing>>);

struct Playing {
    inner: sva_core::Stream,
    source: sva_ast::Composition,
    store: Page,
}

/// An edit takes the stream out while it awaits the store; any call on it meanwhile is refused.
fn busy() -> JsValue {
    let message = "the stream is mid-edit, awaiting its store".to_string();
    let diagnostic = Diagnostic::new("wasm.stream_busy", message.clone())
        .helped("await each edit, add, replace or remove before the stream's next call");
    crossed("validation_error", &message, &[diagnostic])
}

#[wasm_bindgen]
impl Stream {
    fn held(&self) -> Result<RefMut<'_, Playing>, JsValue> {
        RefMut::filter_map(self.0.borrow_mut(), Option::as_mut).map_err(|_| busy())
    }

    fn seen(&self) -> Result<Ref<'_, Playing>, JsValue> {
        Ref::filter_map(self.0.borrow(), Option::as_ref).map_err(|_| busy())
    }

    async fn edited<T>(
        &self,
        edit: impl AsyncFnOnce(&mut Playing) -> Result<T, CliError>,
    ) -> Result<T, JsValue> {
        let mut playing = self.0.borrow_mut().take().ok_or_else(busy)?;
        let done = edit(&mut playing).await;
        *self.0.borrow_mut() = Some(playing);
        done.map_err(|e| thrown(&e))
    }

    /// Writes the next block into `out`, component `c` from `c * block`, and returns the
    /// samples each component took: `block`, fewer where silence ends inside it, `0` after.
    pub fn next(&self, out: &mut [f32]) -> Result<usize, JsValue> {
        let inner = &mut self.held()?.inner;
        let (width, block) = (inner.width(), inner.config().block);
        if out.len() < width * block {
            return Err(refuse(
                format!(
                    "`out` holds {} samples, and a block is {width} component(s) of {block}",
                    out.len()
                ),
                "pass a Float32Array of channels * block samples",
            ));
        }
        let engine = |e| thrown(&CliError::Engine(e));
        let Some(held) = inner.next_block().map_err(engine)? else {
            return Ok(0);
        };
        let planes: Vec<Vec<f32>> = (0..width)
            .map(|c| sva_core::encode::float32(sva_engine::STREAMED, held.plane(c)))
            .collect::<Result<_, _>>()
            .map_err(|e| thrown(&e))?;
        for (c, plane) in planes.iter().enumerate() {
            out[c * block..c * block + plane.len()].copy_from_slice(plane);
        }
        Ok(held.len())
    }

    /// `expr` in place of the target.
    pub async fn edit(&self, expr: &str) -> Result<(), JsValue> {
        self.edited(async |p: &mut Playing| {
            sva_core::edit(&mut p.inner, &p.source, expr, &p.store).await
        })
        .await
    }

    /// `term` summed into `@notes`.
    pub async fn add(&self, term: &str) -> Result<u32, JsValue> {
        self.edited(async |p: &mut Playing| {
            let handle = sva_core::add(&mut p.inner, &p.source, term, &p.store).await;
            handle.map(|handle| handle.0)
        })
        .await
    }

    /// False where the stream no longer holds `handle`.
    pub async fn replace(&self, handle: u32, term: &str) -> Result<bool, JsValue> {
        self.edited(async |p: &mut Playing| {
            let replaced = (Handle(handle), term);
            sva_core::replace(&mut p.inner, &p.source, replaced, &p.store).await
        })
        .await
    }

    pub async fn remove(&self, handle: u32) -> Result<bool, JsValue> {
        self.edited(async |p: &mut Playing| {
            let removed = p.inner.remove(Handle(handle), &p.store).await;
            removed.map_err(CliError::Engine)
        })
        .await
    }

    /// Reads the stored samples the next second of blocks plays, and the stream plays on
    /// meanwhile. A block reading stored samples not yet read computes them, or, live, starts
    /// the note silent where its live remainder is not ready.
    pub async fn fetch(&self) -> Result<(), JsValue> {
        let (wanted, store) = {
            let playing = self.seen()?;
            (playing.inner.wanted(), playing.store.clone())
        };
        for (stored, over) in wanted {
            let Some(samples) = store.read(&stored, over).await else {
                continue;
            };
            if let Ok(mut playing) = self.held() {
                playing.inner.took(stored.key, samples);
            }
        }
        Ok(())
    }

    /// `{ samples, priced_flops, waves }` since it opened.
    pub fn work(&self) -> Result<JsValue, JsValue> {
        parse(&work_json(&self.seen()?.inner.work()))
    }

    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let inner = &self.seen()?.inner;
        parse(&stream_stats_json(&inner.stats(), inner.dropped()))
    }

    #[wasm_bindgen(getter)]
    pub fn held_bytes(&self) -> Result<f64, JsValue> {
        Ok(self.seen()?.inner.held_bytes() as f64)
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> Result<usize, JsValue> {
        Ok(self.seen()?.inner.width())
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> Result<u32, JsValue> {
        Ok(self.seen()?.inner.config().render.rate)
    }

    #[wasm_bindgen(getter)]
    pub fn block(&self) -> Result<usize, JsValue> {
        Ok(self.seen()?.inner.config().block)
    }

    #[wasm_bindgen(getter)]
    pub fn position(&self) -> Result<f64, JsValue> {
        Ok(self.seen()?.inner.position() as f64)
    }

    #[wasm_bindgen(getter)]
    pub fn end(&self) -> Result<Option<f64>, JsValue> {
        Ok(self.seen()?.inner.end().map(|end| end as f64))
    }
}
