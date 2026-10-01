// Concern: the JS surface — a composition a page fills node by node, renders, warms, streams, persists | Non-concern: the pipeline, the store's medium | IO: (path, text) -> samples, blocks, counters

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

mod opfs;

pub use opfs::DirectoryHandle;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sva_core::{
    Asked, CliError, Diagnostic, Job, Printed, Rendered, Report, Representation, SAMPLE_LIMIT,
    counters_json, error_envelope, execute_over, query_data, stats_json, stream_stats_json,
    work_json,
};
use sva_engine::{
    CachePolicy, CacheStats, DEFAULT_STORE_BYTES, Handle, Placed, PrunePolicy, Store, Tier,
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

    #[wasm_bindgen(js_namespace = console)]
    fn warn(message: &str);
}

/// A failing store fails no call: why goes to the console, and memory runs alone.
fn logged(why: &str) {
    warn(&format!("sva: {why}"));
}

fn unstaged(stats: Option<&CacheStats>) {
    if let Some(why) = stats.and_then(|stats| stats.unstaged.as_deref()) {
        logged(&format!("writing what this call computed stopped: {why}"));
    }
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
    live: bool,
    readings: Vec<String>,
    at: Option<Placed>,
    channels: Option<usize>,
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
            "channels" => held.channels = Some(whole(&key, &value)?),
            "flop_budget" => held.flop_budget = Some(whole(&key, &value)?),
            "until" => held.until = Some(text(&key, &value)?),
            "live" => {
                held.live = value
                    .as_bool()
                    .ok_or_else(|| refuse("`live` is not a boolean".into(), "pass true or false"))?
            }
            "readings" => held.readings = names(&key, &value)?,
            "at" => held.at = Some(placed(&text(&key, &value)?)?),
            _ => held.volatile = names(&key, &value)?,
        }
    }
    Ok(held)
}

fn names(key: &str, value: &JsValue) -> Result<Vec<String>, JsValue> {
    let names = value.dyn_ref::<js_sys::Array>().ok_or_else(|| {
        refuse(
            format!("`{key}` is not an array"),
            "pass an array of strings",
        )
    })?;
    names
        .iter()
        .map(|name| {
            name.as_string().ok_or_else(|| {
                refuse(
                    format!("`{key}` holds something that is not a string"),
                    "pass an array of strings",
                )
            })
        })
        .collect()
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

/// Nothing in a browser pushes back when memory grows inside the tab's own address space.
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
    tier: Rc<Tier<opfs::Opfs>>,
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
        Composition::over(name, Tier::alone(DEFAULT_CACHE_BYTES))
    }

    fn over(name: Option<String>, tier: Tier<opfs::Opfs>) -> Composition {
        let inner = sva_ast::Composition::new();
        Composition {
            inner: match name {
                Some(name) => inner.named(name),
                None => inner,
            },
            tier: Rc::new(tier),
        }
    }

    /// Memory over the store in `dir`, an origin-private file system directory; alone where
    /// none opens.
    pub async fn open(name: Option<String>, dir: Option<DirectoryHandle>) -> Composition {
        let Some(dir) = dir else {
            return Composition::new(name);
        };
        let backend = opfs::Opfs { dir };
        let tier = match Store::open(backend, DEFAULT_STORE_BYTES).await {
            Ok(store) => Tier::over(store, DEFAULT_CACHE_BYTES),
            Err(why) => {
                logged(&format!("the store would not open, so none is kept: {why}"));
                Tier::alone(DEFAULT_CACHE_BYTES)
            }
        };
        Composition::over(name, tier)
    }

    /// The only commit to the directory `open` was handed.
    pub async fn persist(&self) -> Result<usize, JsValue> {
        let done = self.tier.persist().await.map_err(unstored)?;
        if done.refused > 0 {
            logged(&format!(
                "{} value(s) were larger than the store's whole budget, so none was stored",
                done.refused
            ));
        }
        Ok(done.written)
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.inner.insert(path, text);
    }

    /// `target` as `sva-cli render` takes it, `@piano([0, 2b], f0=C4)`; `representations`
    /// what `representations()` answers, `samples` where unset, each a call as
    /// `--representation` writes it. `options` sets `rate`, `bits`, `flop_budget`, `until` and
    /// `volatile`.
    pub async fn render(
        &self,
        target: &str,
        representations: Option<Vec<String>>,
        options: JsValue,
    ) -> Result<Rendering, JsValue> {
        let options = options_of(
            &options,
            &["rate", "bits", "flop_budget", "until", "volatile"],
        )?;
        let names = representations.unwrap_or_else(|| vec!["samples".to_string()]);
        let asked = representations_of(&names)?;
        let job = Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            asked: &asked,
            flop_budget: options.flop_budget,
            volatile: &options.volatile,
            ..Job::over(&self.inner, target)
        };
        let inner = execute_over(job, &*self.tier)
            .await
            .map_err(|e| thrown(&e))?;
        unstaged(inner.render.cache_stats.as_ref());
        Ok(Rendering { inner, asked })
    }

    /// `target` computed into memory as `render` would; `{ stats, representations }`, the
    /// latter `representations()` for `readings`. Options: `rate`, `bits`, `flop_budget`,
    /// `readings`.
    pub async fn warm(&self, target: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let options = options_of(&options, &["rate", "bits", "flop_budget", "readings"])?;
        let asked = representations_of(&options.readings)?;
        if let Some(samples) = asked
            .iter()
            .find(|asked| asked.representation == Representation::Samples)
        {
            return Err(refuse(
                format!("`{}` reads samples, and `warm` answers none", samples.name),
                "render the target to read its samples",
            ));
        }
        let job = Job {
            rate: options.rate,
            bits: options.bits,
            flop_budget: options.flop_budget,
            asked: &asked,
            ..Job::over(&self.inner, target)
        };
        let warmed = sva_core::warm(job, &*self.tier).await;
        let warmed = warmed.map_err(|e| thrown(&e))?;
        unstaged(Some(&warmed.stats));
        let representations = match &warmed.readings {
            Some(read) => answered(read, &asked)?,
            None => JsValue::NULL,
        };
        let out = js_sys::Object::new();
        let set = |key: &str, value: &JsValue| js_sys::Reflect::set(&out, &key.into(), value);
        set("stats", &parse(&stats_json(&warmed.stats))?)?;
        set("representations", &representations)?;
        Ok(out.into())
    }

    /// `target` block by block. `options`: `rate`, `bits`, `until`, `live`, `channels`.
    pub async fn stream(
        &self,
        target: &str,
        block: usize,
        options: JsValue,
    ) -> Result<Stream, JsValue> {
        let options = options_of(&options, &["rate", "bits", "until", "live", "channels"])?;
        let job = Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            ..Job::over(&self.inner, target)
        };
        let opened = sva_core::stream(&job, (block, options.channels), &*self.tier).await;
        let mut inner = opened.map_err(|e| thrown(&e))?;
        if options.live {
            inner.go_live();
        }
        let edits = Edits {
            inner: Rc::new(RefCell::new(inner)),
            source: Rc::new(self.inner.clone()),
            tier: Rc::clone(&self.tier),
            freed: Rc::default(),
        };
        let scratch = RefCell::new(Vec::with_capacity(edits.inner.borrow().width() * block));
        Ok(Stream { edits, scratch })
    }

    /// Memory's disk traffic since it opened.
    pub fn counters(&self) -> Result<JsValue, JsValue> {
        parse(&counters_json(&self.tier.counters()))
    }

    #[wasm_bindgen(getter)]
    pub fn cache_bytes(&self) -> f64 {
        self.tier.bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_max_bytes(&self) -> f64 {
        self.tier.max_bytes() as f64
    }

    #[wasm_bindgen(setter)]
    pub fn set_cache_max_bytes(&self, max_bytes: f64) {
        self.tier.set_max_bytes(max_bytes.max(0.0) as u64);
    }

    #[wasm_bindgen(getter)]
    pub fn cache_entries(&self) -> usize {
        self.tier.entries()
    }

    #[wasm_bindgen(getter)]
    pub fn cache_evictions(&self) -> f64 {
        self.tier.counters().evictions() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_policy(&self) -> String {
        self.tier.policy().name().to_string()
    }

    pub fn set_cache_policy(&self, policy: &str) -> Result<(), JsValue> {
        self.tier.set_policy(cache_policy(policy)?);
        Ok(())
    }

    /// `"oldest"` evicts what the latest render neither stored nor read, `"forks"` every value
    /// fewer than two nodes read.
    pub fn prune(&self, policy: &str) -> Result<(), JsValue> {
        self.tier.prune(prune_policy(policy)?);
        Ok(())
    }

    pub fn clear_cache(&self) {
        self.tier.clear();
    }
}

fn placement(options: Option<JsValue>) -> Result<Placed, JsValue> {
    let options = options.unwrap_or(JsValue::UNDEFINED);
    Ok(options_of(&options, &["at"])?.at.unwrap_or(Placed::Written))
}

fn placed(name: &str) -> Result<Placed, JsValue> {
    match name {
        "written" => Ok(Placed::Written),
        "landing" => Ok(Placed::Landing),
        _ => Err(refuse(
            format!("`{name}` names no placement"),
            "pass \"written\" or \"landing\"",
        )),
    }
}

fn cache_policy(name: &str) -> Result<CachePolicy, JsValue> {
    CachePolicy::named(name).ok_or_else(|| {
        let names: Vec<String> = CachePolicy::ALL
            .iter()
            .map(|p| format!("\"{}\"", p.name()))
            .collect();
        let (last, rest) = names.split_last().expect("a policy");
        refuse(
            format!("`{name}` names no cache policy"),
            &format!("pass {} or {last}", rest.join(", ")),
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

    /// Every lookup this render made of memory, and what each came to.
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
        answered(&self.inner, &self.asked)
    }
}

fn answered(rendered: &Rendered, asked: &[Asked]) -> Result<JsValue, JsValue> {
    let mut answers = Vec::with_capacity(asked.len());
    for asked in asked {
        let node = asked.node.as_deref().unwrap_or(&rendered.target);
        let answer = rendered
            .answer(node, asked.representation)
            .map_err(|e| thrown(&e))?;
        answers.push(Printed {
            name: asked.name.clone(),
            answer,
            skim: asked.skim,
        });
    }
    let rate = rendered.config.rate;
    let interval = rendered
        .render
        .range
        .map(|r| (r.start_secs(rate), r.end as f64 / f64::from(rate)));
    parse(&query_data(&Report {
        target: &rendered.expression,
        rate,
        bits: Some(rendered.config.profile.precision_bits),
        interval,
        profile: rendered.config.profile.name,
        label: rendered.label(),
        written: &[],
        answers: &answers,
        analyses: &[],
        limit: Some(SAMPLE_LIMIT),
    }))
}

/// A target block by block, reading `@notes` as the sum of its terms, as `@hall(t, x=@notes)`.
/// A key-up replaces a term with one whose release is a number. A term leaves with its handle
/// once the stream passes its support. An edit lands at the first block boundary
/// once memory answered it; `read` and getters answer meanwhile.
#[wasm_bindgen]
pub struct Stream {
    edits: Edits,
    scratch: RefCell<Vec<f32>>,
}

/// What an edit in flight holds: never the page's object, which the page may free meanwhile.
#[derive(Clone)]
struct Edits {
    inner: Rc<RefCell<sva_core::Stream>>,
    source: Rc<sva_ast::Composition>,
    tier: Rc<Tier<opfs::Opfs>>,
    freed: Rc<Cell<bool>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.edits.freed.set(true);
    }
}

impl Edits {
    /// Refused where the page freed the stream meanwhile: no one hears what it did.
    fn answered<T>(&self, done: Result<T, CliError>) -> Result<T, JsValue> {
        if self.freed.get() {
            let message = "the stream was freed while this edit was in flight".to_string();
            let diagnostic = Diagnostic::new("wasm.stream_freed", message.clone())
                .helped("make the edit on the stream that replaced it");
            return Err(crossed("conflict", &message, &[diagnostic]));
        }
        done.map_err(|e| thrown(&e))
    }
}

/// The futures behind the page's promises, holding no borrow of the stream.
impl Stream {
    pub fn edit(&self, expr: &str) -> impl Future<Output = Result<(), JsValue>> + 'static {
        let (edits, expr) = (self.edits.clone(), expr.to_string());
        async move {
            let edited = sva_core::edit(&edits.inner, &*edits.source, &expr, &*edits.tier).await;
            edits.answered(edited)
        }
    }

    pub fn add(
        &self,
        term: &str,
        options: Option<JsValue>,
    ) -> impl Future<Output = Result<u32, JsValue>> + 'static {
        let (edits, term) = (self.edits.clone(), term.to_string());
        async move {
            let at = placement(options)?;
            let (inner, source) = (&edits.inner, &*edits.source);
            let added = sva_core::add(inner, source, (&term, at), &*edits.tier).await;
            edits.answered(added.map(|handle| handle.0))
        }
    }

    pub fn replace(
        &self,
        handle: u32,
        term: &str,
        options: Option<JsValue>,
    ) -> impl Future<Output = Result<bool, JsValue>> + 'static {
        let (edits, term) = (self.edits.clone(), term.to_string());
        async move {
            let replaced = (Handle(handle), term.as_str(), placement(options)?);
            let (inner, source) = (&edits.inner, &*edits.source);
            let replaced = sva_core::replace(inner, source, replaced, &*edits.tier).await;
            edits.answered(replaced)
        }
    }

    pub fn remove(&self, handle: u32) -> impl Future<Output = Result<bool, JsValue>> + 'static {
        let edits = self.edits.clone();
        async move {
            let removed = sva_core::remove(&edits.inner, Handle(handle), &*edits.tier).await;
            edits.answered(removed)
        }
    }

    pub fn fetch(&self) -> impl Future<Output = ()> + 'static {
        let edits = self.edits.clone();
        async move { sva_core::fetch(&edits.inner, &*edits.tier).await }
    }
}

fn promised<T: Into<JsValue>>(
    done: impl Future<Output = Result<T, JsValue>> + 'static,
) -> js_sys::Promise {
    wasm_bindgen_futures::future_to_promise(async move { done.await.map(Into::into) })
}

#[wasm_bindgen]
impl Stream {
    /// Writes frames from sample `at` into `out` (a SharedArrayBuffer view too) interleaved,
    /// `out[i * channels + c]`, and returns how many: `out.length / channels`, fewer at the end.
    /// A later `at` skips; live, what needed the span starts silent, in `stats().dropped`; only
    /// `until` ends it.
    pub fn read(&self, at: f64, out: &js_sys::Float32Array) -> Result<usize, JsValue> {
        if !(at.is_finite() && at >= 0.0 && at.fract() == 0.0) {
            return Err(refuse(
                format!("`at` is {at}, not a whole sample"),
                "pass a whole sample, the stream's `position` or later",
            ));
        }
        let inner = &mut self.edits.inner.borrow_mut();
        let (width, room) = (inner.width(), out.length() as usize);
        if room % width != 0 {
            return Err(refuse(
                format!("`out` holds {room} samples, not whole frames of {width} component(s)"),
                "pass a Float32Array of channels * frames samples",
            ));
        }
        let Some(held) = inner
            .read(at as i64, room / width)
            .map_err(|e| thrown(&CliError::Engine(e)))?
        else {
            return Ok(0);
        };
        let scratch = &mut self.scratch.borrow_mut();
        scratch.clear();
        for i in 0..held.len() {
            for c in 0..width {
                let sample = held.plane(c)[i];
                let sample = sva_core::encode::float32_sample(sva_engine::STREAMED, sample);
                scratch.push(sample.map_err(|e| thrown(&e))?);
            }
        }
        out.subarray(0, scratch.len() as u32).copy_from(scratch);
        Ok(held.len())
    }

    /// `expr` in place of the target. Each edit's promise rejects as a `conflict` where the
    /// page frees the stream first.
    #[wasm_bindgen(js_name = edit, unchecked_return_type = "Promise<void>")]
    pub fn edit_promise(&self, expr: String) -> js_sys::Promise {
        let edited = self.edit(&expr);
        promised(async move { edited.await.map(|()| JsValue::UNDEFINED) })
    }

    /// `term` summed into `@notes`. `options.at`: `"written"` (the stream's `t`) or
    /// `"landing"`, its sample 0 the sample it lands at.
    #[wasm_bindgen(js_name = add, unchecked_return_type = "Promise<number>")]
    pub fn add_promise(&self, term: String, options: Option<JsValue>) -> js_sys::Promise {
        promised(self.add(&term, options))
    }

    /// False where `handle` is gone. `"landing"`: where its add landed.
    #[wasm_bindgen(js_name = replace, unchecked_return_type = "Promise<boolean>")]
    pub fn replace_promise(
        &self,
        handle: u32,
        term: String,
        options: Option<JsValue>,
    ) -> js_sys::Promise {
        promised(self.replace(handle, &term, options))
    }

    pub fn landed(&self, handle: u32) -> Option<f64> {
        let landed = self.edits.inner.borrow().landed(Handle(handle));
        landed.map(|at| at as f64)
    }

    #[wasm_bindgen(js_name = remove, unchecked_return_type = "Promise<boolean>")]
    pub fn remove_promise(&self, handle: u32) -> js_sys::Promise {
        promised(self.remove(handle))
    }

    /// Reads what the next second plays, a few disk reads a call at most.
    #[wasm_bindgen(js_name = fetch, unchecked_return_type = "Promise<void>")]
    pub fn fetch_promise(&self) -> js_sys::Promise {
        let fetched = self.fetch();
        promised(async move {
            fetched.await;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// `{ samples, priced_flops, waves }` since it opened.
    pub fn work(&self) -> Result<JsValue, JsValue> {
        parse(&work_json(&self.edits.inner.borrow().work()))
    }

    /// Each list the latest; its `pagination.count` counts all. `pruned`: the level a term under
    /// which for good leaves the sum, and each node cut, at the sample it is zero from.
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let inner = self.edits.inner.borrow();
        let made = inner.counts().dropped;
        parse(&stream_stats_json(
            &inner.stats(),
            (&inner.dropped(), made),
            &inner.pruned(),
        ))
    }

    /// `late`: edits landed late; `demands`: reads working out the note sum's asks; `built`:
    /// what the latest change did anew; `tier`: memory's disk traffic since it opened.
    pub fn counts(&self) -> Result<JsValue, JsValue> {
        let counts = self.edits.inner.borrow().counts();
        let whole = |pairs: &[(&str, usize)]| -> Result<js_sys::Object, JsValue> {
            let out = js_sys::Object::new();
            for (key, value) in pairs {
                js_sys::Reflect::set(&out, &(*key).into(), &(*value as f64).into())?;
            }
            Ok(out)
        };
        let built = counts.built;
        let out = whole(&[
            ("dropped", counts.dropped),
            ("late", counts.late),
            ("terms", counts.terms),
            ("demands", counts.demands),
        ])?;
        let built = whole(&[
            ("parsed", built.parsed),
            ("instances", built.instances),
            ("visited", built.visited),
            ("typed", built.typed),
            ("values", built.values),
            ("copied", built.copied),
            ("lookups", built.lookups),
        ])?;
        js_sys::Reflect::set(&out, &"built".into(), &built)?;
        let tier = parse(&counters_json(&counts.tier))?;
        js_sys::Reflect::set(&out, &"tier".into(), &tier)?;
        Ok(out.into())
    }

    #[wasm_bindgen(getter)]
    pub fn held_bytes(&self) -> f64 {
        self.edits.inner.borrow().held_bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.edits.inner.borrow().width()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.edits.inner.borrow().config().render.rate
    }

    #[wasm_bindgen(getter)]
    pub fn block(&self) -> usize {
        self.edits.inner.borrow().config().block
    }

    #[wasm_bindgen(getter)]
    pub fn position(&self) -> f64 {
        self.edits.inner.borrow().position() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn end(&self) -> Option<f64> {
        self.edits.inner.borrow().end().map(|end| end as f64)
    }
}
