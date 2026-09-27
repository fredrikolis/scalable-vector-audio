// Concern: the JS surface — a composition a page fills node by node, and what it renders or streams to | Non-concern: the pipeline (sva-core), the store | IO: (path, text) -> samples, blocks, a reading

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

use sva_core::{
    Answer, CliError, Diagnostic, Job, Rendered, Report, SAMPLE_LIMIT, Settings, Shaping,
    error_envelope, execute, query_data, representation_for, retired, stats_json, work_json,
};
use sva_engine::{Buffer, Cache, CachePolicy, CacheStats, PrunePolicy, Representation};
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

#[derive(Default)]
struct Config {
    settings: Settings,
    volatile: Vec<String>,
    cache: Option<CachePolicy>,
}

/// The keys `sva-cli`'s `-c` takes for a render, read by the same parser.
const SETTINGS: [&str; 7] = [
    "flop_budget",
    "proof_limit",
    "node",
    "depth",
    "peaks",
    "oversample",
    "frame",
];

fn config_of(config: &JsValue) -> Result<Config, JsValue> {
    let mut held = Config::default();
    if config.is_undefined() || config.is_null() {
        return Ok(held);
    }
    let object = config.dyn_ref::<js_sys::Object>().ok_or_else(|| {
        refuse(
            "`config` is not an object".into(),
            "pass an object, such as { flop_budget: 1e9 }",
        )
    })?;
    for entry in js_sys::Object::entries(object).iter() {
        let pair = js_sys::Array::from(&entry);
        let key = pair.get(0).as_string().unwrap_or_default();
        let value = pair.get(1);
        match key.as_str() {
            "volatile" => {
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
            "cache" => {
                let named = value.as_string().ok_or_else(|| {
                    refuse(
                        "`cache` is not a policy name".into(),
                        "pass a policy's name",
                    )
                })?;
                held.cache = Some(cache_policy(&named)?);
            }
            key => {
                let raw = match (value.as_f64(), value.as_string()) {
                    (Some(number), _) => number.to_string(),
                    (None, Some(text)) => text,
                    _ => {
                        return Err(refuse(
                            format!("`{key}` is set to neither a number nor a string"),
                            "set it as `sva-cli`'s `-c` would",
                        ));
                    }
                };
                held.settings
                    .set(key, &raw, &SETTINGS)
                    .map_err(|e| thrown(&e))?;
            }
        }
    }
    Ok(held)
}

fn representations_of(
    names: &[String],
    shape: Shaping,
) -> Result<Vec<(String, Representation)>, JsValue> {
    names
        .iter()
        .map(|name| {
            if let Some(write) = retired(name) {
                return Err(refuse(
                    format!("`{name}` left the language"),
                    &format!("ask for `{write}`"),
                ));
            }
            representation_for(name, shape)
                .map(|representation| (name.clone(), representation))
                .ok_or_else(|| {
                    refuse(
                        format!("unknown representation `{name}`"),
                        "ask for one of the readings a render answers",
                    )
                })
        })
        .collect()
}

fn bindings_of(bindings: &JsValue) -> Result<Vec<(String, f64)>, JsValue> {
    if bindings.is_undefined() || bindings.is_null() {
        return Ok(Vec::new());
    }
    let object = bindings.dyn_ref::<js_sys::Object>().ok_or_else(|| {
        refuse(
            "`bindings` is not an object".into(),
            "pass an object of numbers, such as { release: 1.5 }",
        )
    })?;
    js_sys::Object::entries(object)
        .iter()
        .map(|entry| {
            let pair = js_sys::Array::from(&entry);
            let name = pair.get(0).as_string().unwrap_or_default();
            match pair.get(1).as_f64() {
                Some(value) => Ok((name, value)),
                None => Err(refuse(
                    format!("`{name}` is bound to something that is not a number"),
                    "bind each name to a number",
                )),
            }
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
        }
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.inner.insert(path, text);
    }

    /// `target` as `sva-cli render` takes it, `@piano([0, 2b], f0=C4)`; `until` its condition;
    /// `representations` what `readings()` answers, `samples` where unset. `config` sets
    /// `flop_budget`, `proof_limit`, `node`, `depth`, `peaks`, `oversample`, `frame`,
    /// `volatile` and `cache`.
    pub fn render(
        &self,
        target: &str,
        until: Option<String>,
        representations: Option<Vec<String>>,
        rate: Option<u32>,
        config: JsValue,
    ) -> Result<Rendering, JsValue> {
        let config = config_of(&config)?;
        let settings = &config.settings;
        let names = representations.unwrap_or_else(|| vec!["samples".to_string()]);
        let asked = representations_of(&names, settings.shape)?;
        let rendered = execute(Job {
            until: until.as_deref(),
            rate,
            cache: Some(&self.store),
            cache_policy: config.cache,
            reading: settings.node.as_deref(),
            representations: asked.iter().map(|(_, r)| *r).collect(),
            flop_budget: settings.flop_budget,
            proof_limit_secs: settings.proof_limit,
            volatile: &config.volatile,
            ..Job::over(&self.inner, target)
        });
        rendered
            .map(|inner| Rendering {
                inner,
                asked,
                node: settings.node.clone(),
            })
            .map_err(|e| thrown(&e))
    }

    /// `target` block by block over the extents a render takes; each key of `bindings` is a
    /// named argument on its own ref, the ones a resume may move.
    pub fn stream(
        &self,
        target: &str,
        block: usize,
        until: Option<String>,
        rate: Option<u32>,
        bindings: JsValue,
    ) -> Result<Stream, JsValue> {
        let bindings = bindings_of(&bindings)?;
        let job = Job {
            until: until.as_deref(),
            rate,
            ..Job::over(&self.inner, target)
        };
        sva_core::stream(&job, block, &bindings)
            .map(|inner| Stream {
                inner,
                rate: rate.unwrap_or(sva_engine::DEFAULT_SAMPLE_RATE),
            })
            .map_err(|e| thrown(&e))
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
    asked: Vec<(String, Representation)>,
    node: Option<String>,
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
        self.buffer().map_or(0, |b| b.width)
    }

    pub fn samples(&self, channel: usize) -> Result<Vec<f32>, JsValue> {
        let buffer = self.buffer().ok_or_else(|| {
            refuse(
                format!("`{}` read no samples of its root", self.inner.expression),
                "ask for `samples` among the representations",
            )
        })?;
        if channel >= buffer.width {
            return Err(refuse(
                format!(
                    "`{}` is {} component(s), so there is no component {channel}",
                    self.inner.expression, buffer.width
                ),
                "read a component this node holds, counting from zero",
            ));
        }
        Ok(buffer.as_f32(channel))
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
    pub fn readings(&self) -> Result<JsValue, JsValue> {
        let node = self.node.as_deref().unwrap_or(&self.inner.target);
        let mut answers: Vec<(String, Answer)> = Vec::with_capacity(self.asked.len());
        for (name, representation) in &self.asked {
            let answer = self
                .inner
                .answer(node, *representation)
                .map_err(|e| thrown(&e))?;
            answers.push((name.clone(), answer));
        }
        let rate = self.inner.config.rate;
        let range = self
            .inner
            .render
            .range
            .map(|r| (r.start_secs(rate), r.end as f64 / f64::from(rate)));
        parse(&query_data(&Report {
            target: &self.inner.expression,
            rate,
            range,
            profile: self.inner.config.profile.name,
            label: self.inner.label(),
            written: &[],
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
            skim: false,
        }))
    }

    fn buffer(&self) -> Option<Buffer> {
        self.inner.render.output(self.inner.render.root)
    }
}

/// A target block by block, for a player that cannot know how long a note is held.
#[wasm_bindgen]
pub struct Stream {
    inner: sva_core::Stream,
    rate: u32,
}

#[wasm_bindgen]
impl Stream {
    /// Writes the next block into `out`, component `c` from `c * block`, and returns the
    /// samples each component took: `block`, fewer where silence ends inside it, `0` after.
    pub fn next(&mut self, out: &mut [f32]) -> Result<usize, JsValue> {
        let (width, block) = (self.inner.width(), self.inner.config().block);
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
        let Some(held) = self.inner.next_block().map_err(engine)? else {
            return Ok(0);
        };
        for c in 0..width {
            for (slot, value) in out[c * block..].iter_mut().zip(held.plane(c)) {
                *slot = *value as f32;
            }
        }
        Ok(held.len())
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            inner: self.inner.checkpoint(),
        }
    }

    /// `{ samples, proofs, priced_flops, waves }` since it opened or resumed.
    pub fn work(&self) -> Result<JsValue, JsValue> {
        parse(&work_json(&self.inner.work()))
    }

    /// `until` as `stream` takes it, `sva-cli`'s default where unset.
    pub fn resume(
        &self,
        checkpoint: &Checkpoint,
        bindings: JsValue,
        until: Option<String>,
    ) -> Result<Stream, JsValue> {
        let bindings = bindings_of(&bindings)?;
        let text = until.unwrap_or_else(sva_core::default_until);
        let condition = sva_core::until(&text, self.rate, None).map_err(|e| thrown(&e))?;
        self.inner
            .resume(&checkpoint.inner, &bindings, Some(condition))
            .map(|inner| Stream {
                inner,
                rate: self.rate,
            })
            .map_err(|e| thrown(&CliError::Engine(e)))
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.inner.width()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    #[wasm_bindgen(getter)]
    pub fn block(&self) -> usize {
        self.inner.config().block
    }

    #[wasm_bindgen(getter)]
    pub fn position(&self) -> f64 {
        self.inner.position() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn end(&self) -> Option<f64> {
        self.inner.end().map(|end| end as f64)
    }
}

#[wasm_bindgen]
pub struct Checkpoint {
    inner: sva_core::Checkpoint,
}

#[wasm_bindgen]
impl Checkpoint {
    #[wasm_bindgen(getter)]
    pub fn position(&self) -> f64 {
        self.inner.position() as f64
    }
}
