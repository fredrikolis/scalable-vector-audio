// Concern: the JS surface — a composition a page fills node by node, and what it renders or streams to | Non-concern: the pipeline (sva-core), the store | IO: (path, text) -> samples, blocks, a reading

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

use sva_core::{
    Asked, CliError, Diagnostic, Job, Printed, Rendered, Report, SAMPLE_LIMIT, error_envelope,
    execute, query_data, stats_json, stream_stats_json, work_json,
};
use sva_engine::{Buffer, Cache, CachePolicy, CacheStats, Handle, PrunePolicy};
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

    /// `target` as `sva-cli render` takes it, `@piano([0, 2b], f0=C4)`; `representations`
    /// what `representations()` answers, `samples` where unset, each a call as
    /// `--representation` writes it. `options` sets `rate`, `bits`, `flop_budget`, `until`,
    /// `volatile` and `cache`.
    pub fn render(
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
        let rendered = execute(Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            cache: Some(&self.store),
            cache_policy: options.cache,
            asked: &asked,
            flop_budget: options.flop_budget,
            volatile: &options.volatile,
            ..Job::over(&self.inner, target)
        });
        rendered
            .map(|inner| Rendering { inner, asked })
            .map_err(|e| thrown(&e))
    }

    /// `target` block by block, through this store. `options`: `rate`, `bits`, `until`, `cache`,
    /// `live`.
    pub fn stream(&self, target: &str, block: usize, options: JsValue) -> Result<Stream, JsValue> {
        let options = options_of(&options, &["rate", "bits", "until", "cache", "live"])?;
        let job = Job {
            until: options.until.as_deref(),
            rate: options.rate,
            bits: options.bits,
            cache: Some(&self.store),
            cache_policy: options.cache,
            ..Job::over(&self.inner, target)
        };
        let mut inner = sva_core::stream(&job, block).map_err(|e| thrown(&e))?;
        if options.live {
            inner.go_live();
        }
        Ok(Stream {
            inner,
            source: self.inner.clone(),
        })
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
        self.inner.render.output.map_or(0.0, |r| r.start_secs(rate))
    }

    #[wasm_bindgen(getter)]
    pub fn duration_secs(&self) -> f64 {
        let rate = self.inner.config.rate;
        self.inner.render.output.map_or(0.0, |r| r.span_secs(rate))
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
            .output
            .map(|r| (r.start_secs(rate), r.end as f64 / f64::from(rate)));
        let bounds = self.inner.render.reconstructions();
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
            bounds: &bounds,
        }))
    }

    fn buffer(&self) -> Option<Buffer> {
        self.inner.render.output(self.inner.render.root)
    }
}

/// A target block by block, reading `@notes` as the sum of its terms, as `@hall(t, x=@notes)`.
/// A key-up replaces a term with one whose release is a number. A term leaves with its handle
/// once its node ends, bar the last.
#[wasm_bindgen]
pub struct Stream {
    inner: sva_core::Stream,
    source: sva_ast::Composition,
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

    /// `expr` in place of the target.
    pub fn edit(&mut self, expr: &str) -> Result<(), JsValue> {
        sva_core::edit(&mut self.inner, &self.source, expr).map_err(|e| thrown(&e))
    }

    /// `term` summed into `@notes`.
    pub fn add(&mut self, term: &str) -> Result<u32, JsValue> {
        sva_core::add(&mut self.inner, &self.source, term)
            .map(|handle| handle.0)
            .map_err(|e| thrown(&e))
    }

    /// False where the stream no longer holds `handle`.
    pub fn replace(&mut self, handle: u32, term: &str) -> Result<bool, JsValue> {
        sva_core::replace(&mut self.inner, &self.source, Handle(handle), term)
            .map_err(|e| thrown(&e))
    }

    pub fn remove(&mut self, handle: u32) -> Result<bool, JsValue> {
        self.inner
            .remove(Handle(handle))
            .map_err(|e| thrown(&CliError::Engine(e)))
    }

    /// `{ samples, priced_flops, waves }` since it opened.
    pub fn work(&self) -> Result<JsValue, JsValue> {
        parse(&work_json(&self.inner.work()))
    }

    pub fn stats(&self) -> Result<JsValue, JsValue> {
        parse(&stream_stats_json(
            &self.inner.stats(),
            self.inner.dropped(),
        ))
    }

    #[wasm_bindgen(getter)]
    pub fn held_bytes(&self) -> f64 {
        self.inner.held_bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.inner.width()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.config().render.rate
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
