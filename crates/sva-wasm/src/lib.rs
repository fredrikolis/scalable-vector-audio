// Concern: the JS surface — a composition a page fills node by node, and what one renders to | Non-concern: the pipeline (sva-core), the store (sva-engine) | IO: (path, text) -> samples or a reading

//! A page holds no directory: nodes arrive one at a time, so a composition is BUILT rather
//! than read. A refusal crosses as a thrown `Error`: `name` is the CLI's error code,
//! `refusal` the envelope it would print.

use sva_core::{
    CliError, Diagnostic, Job, Rendered, Report, SAMPLE_LIMIT, WindowEdge, error_envelope, execute,
    query_data, representation_for, retired, window_for,
};
use sva_engine::{Buffer, Cache, Horizon, MemoryCache, PSYCHOACOUSTIC_V1, answer_buffer};
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

#[wasm_bindgen]
pub struct Composition {
    inner: sva_ast::Composition,
    cache: MemoryCache,
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
            cache: MemoryCache::holding(DEFAULT_CACHE_BYTES),
        }
    }

    pub fn insert(&mut self, path: &str, text: &str) {
        self.inner.insert(path, text);
    }

    /// Unset, `target` is `master`; `seconds` is the horizon and `rate` the observation rate.
    pub fn render(
        &self,
        target: Option<String>,
        rate: Option<u32>,
        seconds: Option<f64>,
    ) -> Result<Rendering, JsValue> {
        execute(Job {
            target: target.as_deref(),
            until: seconds.map(WindowEdge::Secs),
            sample_rate: rate,
            reaching: true,
            cache: Some(&self.cache),
            ..Job::over(&self.inner)
        })
        .map(|inner| Rendering { inner })
        .map_err(|e| thrown(&e))
    }

    #[wasm_bindgen(getter)]
    pub fn cache_bytes(&self) -> f64 {
        self.cache.held_bytes() as f64
    }

    #[wasm_bindgen(getter)]
    pub fn cache_max_bytes(&self) -> f64 {
        self.cache.max_bytes() as f64
    }

    pub fn bound_cache(&mut self, max_bytes: f64) {
        self.cache = MemoryCache::holding(max_bytes.max(0.0) as u64);
    }

    pub fn clear_cache(&mut self) {
        self.cache = MemoryCache::holding(self.cache.max_bytes());
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
        Ok(held[range(buffer, over)].to_vec())
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
        let (answer, over) = match narrowed {
            None => (
                self.inner
                    .answer(&self.inner.target, wanted)
                    .map_err(|e| thrown(&e))?,
                whole,
            ),
            Some((buffer, over)) => (
                answer_buffer(&self.inner.target, &buffer, wanted, PSYCHOACOUSTIC_V1.name)
                    .map_err(|e| thrown(&sva_core::CliError::Engine(e)))?,
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
        let taken = range(buffer, over);
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

fn range(buffer: &Buffer, over: Horizon) -> std::ops::Range<usize> {
    let rate = f64::from(buffer.rate);
    let at = |secs: f64| {
        (((secs - buffer.origin_secs) * rate).round().max(0.0) as usize).min(buffer.len())
    };
    let start = at(over.start_secs);
    let end = match over.end_secs.is_finite() {
        true => at(over.end_secs).max(start),
        false => buffer.len(),
    };
    start..end
}
