// Concern: the JS surface — a composition a page fills node by node, and what it renders or streams to | Non-concern: the pipeline (sva-core), the store | IO: (path, text) -> samples, blocks, a reading

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

use sva_core::{
    CliError, Diagnostic, Job, Rendered, Report, SAMPLE_LIMIT, Silent, WindowEdge, error_envelope,
    execute, query_data, representation_for, retired, silence, stats_json, window_for, work_json,
};
use sva_engine::{
    Buffer, Cache, CacheStats, Horizon, PSYCHOACOUSTIC_V1, PrunePolicy, Representation,
    answer_buffer, ledger_over,
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

fn ending(
    until: &JsValue,
    bits: Option<u32>,
    max_secs: Option<f64>,
) -> Result<(Option<f64>, Option<Silent>), JsValue> {
    let shaped = bits.is_some() || max_secs.is_some();
    if let Some("silent") = until.as_string().as_deref() {
        return silence(bits, max_secs)
            .map(|silent| (None, Some(silent)))
            .map_err(|e| thrown(&e));
    }
    if shaped {
        return Err(refuse(
            "`bits` and `max_secs` shape a silent render, and `until` is not \"silent\"".into(),
            "pass \"silent\" as `until`",
        ));
    }
    match until.as_f64() {
        Some(seconds) => Ok((Some(seconds), None)),
        None if until.is_undefined() || until.is_null() => Ok((None, None)),
        None => Err(refuse(
            "`until` is a number of seconds or \"silent\"".into(),
            "pass seconds, \"silent\", or nothing",
        )),
    }
}

fn stream_ending(
    until: &JsValue,
    bits: Option<u32>,
    max_secs: Option<f64>,
) -> Result<Option<Silent>, JsValue> {
    match ending(until, bits, max_secs)? {
        (None, silent) => Ok(silent),
        (Some(_), _) => Err(refuse(
            "a stream ends where silence is proven, or runs on; it has no stated end".into(),
            "pass \"silent\" as `until`, or nothing",
        )),
    }
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

    /// Unset, `target` is `master`; `until` is the horizon in seconds, or `"silent"` to end
    /// where every later sample is provably under `2^-bits` (default the profile's own), by
    /// `max_secs` at the latest. `rate` is the observation rate. What reads a `volatile`
    /// parameter keeps one value in the store, its last.
    pub fn render(
        &self,
        target: Option<String>,
        rate: Option<u32>,
        until: JsValue,
        volatile: Option<Vec<String>>,
        bits: Option<u32>,
        max_secs: Option<f64>,
    ) -> Result<Rendering, JsValue> {
        let volatile = volatile.unwrap_or_default();
        let (seconds, silent) = ending(&until, bits, max_secs)?;
        let rendered = execute(Job {
            target: target.as_deref(),
            until: seconds.map(WindowEdge::Secs),
            silent,
            sample_rate: rate,
            reaching: true,
            cache: Some(&self.store),
            volatile: &volatile,
            ..Job::over(&self.inner)
        });
        rendered
            .map(|inner| Rendering { inner })
            .map_err(|e| thrown(&e))
    }

    /// A stream of `target` (`master` unset) at `rate`, `block` samples a block, each key of
    /// `bindings` a named argument on the target. `until: "silent"` ends it at the first block
    /// whose end proves every later sample under `2^-bits`, throwing past `max_secs`.
    #[allow(clippy::too_many_arguments)]
    pub fn stream(
        &self,
        target: Option<String>,
        rate: Option<u32>,
        block: usize,
        bindings: JsValue,
        until: JsValue,
        bits: Option<u32>,
        max_secs: Option<f64>,
    ) -> Result<Stream, JsValue> {
        let silent = stream_ending(&until, bits, max_secs)?;
        let bindings = bindings_of(&bindings)?;
        let job = Job {
            target: target.as_deref(),
            sample_rate: rate,
            reaching: true,
            silent,
            ..Job::over(&self.inner)
        };
        sva_core::stream(&job, block, &bindings)
            .map(|inner| Stream { inner })
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

/// A target block by block, for a player that cannot know how long a note is held.
#[wasm_bindgen]
pub struct Stream {
    inner: sva_core::Stream,
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

    /// `until` as `stream` takes it, `max_secs` counted from the checkpoint.
    pub fn resume(
        &self,
        checkpoint: &Checkpoint,
        bindings: JsValue,
        until: JsValue,
        bits: Option<u32>,
        max_secs: Option<f64>,
    ) -> Result<Stream, JsValue> {
        let silent = stream_ending(&until, bits, max_secs)?;
        let bindings = bindings_of(&bindings)?;
        self.inner
            .resume(&checkpoint.inner, &bindings, silent)
            .map(|inner| Stream { inner })
            .map_err(|e| thrown(&CliError::Engine(e)))
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.inner.width()
    }

    #[wasm_bindgen(getter)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.config().rate
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
