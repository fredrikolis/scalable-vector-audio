// Concern: drives every JS export through real wasm | Non-concern: what a sample is worth (sva-engine), the pipeline (sva-cli) | IO: (a built composition) -> assertions
#![cfg(target_arch = "wasm32")]

//! The gap a native test cannot see: `wasm32-unknown-unknown` has no clock and no threads, and
//! reaching for either compiles clean and traps only at RUNTIME. Hence one session for the
//! whole surface. Run it with `wasm-pack test --node crates/sva-wasm`.

use std::task::{Context, Poll, Waker};
use sva_wasm::{Composition, Rendering, Stream, builtins, outline};

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

/// Through js-sys, not a local `extern "C"`: an import naming `JSON.parse` a second time inside
/// this package collides with the crate's own shim at link time.
fn as_text(value: &JsValue) -> String {
    js_sys::JSON::stringify(value)
        .map(String::from)
        .unwrap_or_else(|_| unreachable!("the value is JSON"))
}

/// A collection's own list: every array this tool answers sits under `items`.
fn items(value: &JsValue, name: &str) -> js_sys::Array {
    js_sys::Array::from(&field(&field(value, name), "items"))
}

fn field(value: &JsValue, name: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(name))
        .unwrap_or_else(|_| unreachable!("{name} is readable"))
}

/// `{ rate: 8000 }` and each of `more`, as a page writes its options.
fn options(more: &[(&str, JsValue)]) -> JsValue {
    let held = js_sys::Object::new();
    for (key, value) in [("rate", JsValue::from(8000))].iter().chain(more) {
        js_sys::Reflect::set(&held, &(*key).into(), value)
            .unwrap_or_else(|_| unreachable!("an object takes a key"));
    }
    held.into()
}

/// A render with no persistent store never waits, so one poll finishes it.
trait Now {
    fn rendered(
        &self,
        target: &str,
        representations: Option<Vec<String>>,
        options: JsValue,
    ) -> Result<Rendering, JsValue>;
}

impl Now for Composition {
    fn rendered(
        &self,
        target: &str,
        representations: Option<Vec<String>>,
        options: JsValue,
    ) -> Result<Rendering, JsValue> {
        let mut render = std::pin::pin!(self.render(target, representations, options));
        let mut context = Context::from_waker(Waker::noop());
        match render.as_mut().poll(&mut context) {
            Poll::Ready(done) => done,
            Poll::Pending => unreachable!("a render in memory alone never waits"),
        }
    }
}

/// A stream in memory alone never waits either.
fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(done) => done,
        Poll::Pending => unreachable!("a stream in memory alone never waits"),
    }
}

fn page() -> Composition {
    let mut held = Composition::new(Some("a-registry".to_string()));
    held.insert("master", "@partials/one*0.5\n");
    held.insert("partials/one", "sin(2*pi*100*t)\n");
    held.insert("wide", "join(sin(2*pi*100*t), sin(2*pi*200*t))\n");
    held.insert("unreached", "this is not an expression (\n");
    held
}

/// `@<node>` over its first second, read for `representations`.
fn asking(of: &Composition, node: &str, representations: &[&str]) -> Rendering {
    let asked = representations.iter().map(|r| (*r).to_string()).collect();
    of.rendered(&format!("@{node}([0, 1s])"), Some(asked), options(&[]))
        .unwrap_or_else(|_| unreachable!("`{node}` renders"))
}

fn render(of: &Composition, node: &str) -> Rendering {
    asking(of, node, &["samples"])
}

fn plane(of: &Rendering) -> Vec<f32> {
    of.samples(0)
        .unwrap_or_else(|_| unreachable!("one component"))
}

fn readings(of: &Rendering) -> JsValue {
    of.representations().unwrap_or_else(|e| {
        unreachable!("representations answer: {}", as_text(&field(&e, "refusal")))
    })
}

#[wasm_bindgen_test]
fn every_export_survives_the_boundary() {
    let held = page();

    let one = render(&held, "partials/one");
    assert_eq!(
        one.target(),
        "@partials/one([0, 1s])",
        "the target as written"
    );
    assert_eq!(one.sample_rate(), 8000);
    assert_eq!(one.channels(), 1);
    assert_eq!(one.duration_secs(), 1.0);

    let drawn = plane(&one);
    assert_eq!(drawn.len(), 8000, "a typed array of every sample");
    let want = (2.0 * std::f64::consts::PI * 100.0 * (2.0 / 8000.0)).sin() as f32;
    assert!((drawn[2] - want).abs() < 1e-4, "{} vs {want}", drawn[2]);

    let part = held
        .rendered("@partials/one([0.25s, 0.5s])", None, options(&[]))
        .unwrap_or_else(|_| unreachable!("an interval renders"));
    assert_eq!(part.start_secs(), 0.25);
    assert_eq!(plane(&part).len(), 2000, "an interval narrows the array");
    let read = field(&readings(&part), "representations");
    let component = items(&field(&field(&read, "samples"), "value"), "components").get(0);
    let paging = field(&field(&component, "values"), "pagination");
    assert_eq!(
        field(&paging, "count").as_f64(),
        Some(2000.0),
        "and the reading the CLI prints"
    );
    assert_eq!(
        plane(&part)[..],
        drawn[2000..4000],
        "and trims the output alone"
    );

    let wide = render(&held, "wide");
    assert_eq!(wide.channels(), 2);
    assert!(
        wide.samples(2).is_err(),
        "a component that is not there refuses, and does not trap"
    );

    assert!(held.cache_max_bytes() > 0.0, "a default budget is held");
}

#[wasm_bindgen_test]
fn a_representation_crosses_as_the_object_the_cli_puts_under_data() {
    let one = asking(&page(), "partials/one", &["envelope", "samples"]);

    let asked = readings(&one);
    let spelled = as_text(&asked);
    assert!(
        spelled.contains("\"target\":\"@partials/one([0, 1s])\"")
            && spelled.contains("\"envelope\":"),
        "the CLI's own keys: {spelled}"
    );
    assert_eq!(field(&asked, "sample_rate").as_f64(), Some(8000.0));

    let read = field(&asked, "representations");
    let component = items(&field(&field(&read, "samples"), "value"), "components").get(0);
    let values = field(&component, "values");
    assert_eq!(
        js_sys::Array::from(&field(&values, "items")).length(),
        4096,
        "capped exactly as stdout caps it, where the typed array is not"
    );
    let paging = field(&values, "pagination");
    assert_eq!(
        field(&paging, "count").as_f64(),
        Some(8000.0),
        "the count is the whole reading's, not the page's"
    );
    assert_eq!(
        field(&paging, "has_more").as_bool(),
        Some(true),
        "and it says so rather than reading as the whole"
    );
}

/// FORMAT 14: every name the CLI answers is a name this surface recognizes. Some of them
/// refuse on a mono closed form — a stereo image needs two components — but none is unknown here.
#[wasm_bindgen_test]
fn every_representation_name_the_cli_answers_crosses_and_the_removed_ones_refuse() {
    let held = page();
    for name in [
        "lines",
        "atoms",
        "samples",
        "ledger",
        "loudness",
        "envelope",
        "arguments",
    ] {
        let answered = readings(&asking(&held, "partials/one", &[name]));
        let spelled = as_text(&answered);
        assert!(spelled.contains(&format!("\"{name}\":")), "{spelled}");
        assert!(
            spelled.contains("\"source\":") && spelled.contains("\"profile\":"),
            "every answer says which reading ran: {spelled}"
        );
    }
    for name in [
        "spectrum",
        "derivative",
        "pitch",
        "formants",
        "stereo",
        "bands",
        "crest",
        "alias",
        "bindings",
    ] {
        let asked = held.rendered(
            "@partials/one([0, 1s])",
            Some(vec![name.to_string()]),
            options(&[]),
        );
        if let Err(refused) = asked.and_then(|one| one.representations()) {
            let spelled = as_text(&field(&refused, "refusal"));
            assert!(
                !spelled.contains("unknown representation"),
                "`{name}` is a name the CLI answers: {spelled}"
            );
        }
    }
    for gone in [
        "exact-envelope",
        "exact-derivative",
        "automation",
        "nonsense",
    ] {
        assert!(
            held.rendered(
                "@partials/one([0, 1s])",
                Some(vec![gone.to_string()]),
                options(&[]),
            )
            .is_err(),
            "`{gone}` is not a reading this surface answers"
        );
    }
}

/// The store is a `Mutex` over a map, and a lock is the third thing `wasm32-unknown-unknown`
/// compiles and might not run. Held across renders, it must also still answer the same samples.
#[wasm_bindgen_test]
fn a_cache_held_across_renders_locks_and_answers_what_the_first_render_did() {
    let mut held = page();
    let cold = plane(&render(&held, "partials/one"));
    assert_eq!(
        plane(&render(&held, "partials/one")),
        cold,
        "a hit is the work"
    );

    held.insert("partials/one", "sin(2*pi*200*t)\n");
    let after = plane(&render(&held, "partials/one"));
    assert_ne!(after, cold, "the replaced node is not answered from before");

    held.clear_cache();
    assert_eq!(held.cache_bytes(), 0.0);
    assert_eq!(plane(&render(&held, "partials/one")), after);
}

/// A render's cache stats, with the second of two identical renders answered wholly from the composition's own store.
#[wasm_bindgen_test]
fn a_repeated_render_is_all_hits() {
    let held = page();
    let cold = render(&held, "master")
        .stats()
        .unwrap_or_else(|_| unreachable!("stats answer"));
    let warm = render(&held, "master")
        .stats()
        .unwrap_or_else(|_| unreachable!("stats answer"));

    let lookups = items(&warm, "lookups");
    assert!(lookups.length() > 0, "{}", as_text(&warm));
    assert_eq!(
        field(&warm, "hits").as_f64(),
        Some(f64::from(lookups.length())),
        "every lookup a hit: {}",
        as_text(&warm)
    );
    assert_eq!(field(&warm, "computed").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "stored").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "replaced").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "evictions").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "bytes").as_f64(), Some(held.cache_bytes()));
    assert_eq!(
        field(&warm, "max_bytes").as_f64(),
        Some(held.cache_max_bytes())
    );
    assert_eq!(
        field(&warm, "entries").as_f64(),
        Some(held.cache_entries() as f64)
    );
    assert!(
        field(&warm, "nodes").as_f64() <= field(&cold, "nodes").as_f64(),
        "the target answers for what it holds"
    );
    let first = lookups.get(0);
    for key in ["node", "key", "kind"] {
        assert!(field(&first, key).is_string(), "{key}: {}", as_text(&first));
    }
    assert_eq!(field(&first, "outcome").as_string().as_deref(), Some("hit"));

    let first_cold = items(&cold, "lookups").get(0);
    assert_eq!(
        field(&first_cold, "outcome").as_string().as_deref(),
        Some("computed_stored")
    );
}

/// A note released after a held render reads the held run up to its release, resuming from
/// the last state it marked, and its lookup crosses as `prefix`; its samples are the cold
/// render's.
#[wasm_bindgen_test]
fn a_release_after_a_held_render_reads_it_as_a_prefix() {
    let mut held = Composition::new(None);
    held.insert(
        "string",
        "release = inf\nchaigne_askenfelt(440, damper_r=0.1*crop(1, release, inf))\n",
    );
    held.insert("released", "@string(t, release=2.5)\n");
    let over = |node: &str| {
        held.rendered(&format!("@{node}([0, 3s])"), None, options(&[]))
            .unwrap_or_else(|_| unreachable!("`{node}` renders"))
    };
    over("string");
    let warm = over("released");
    let outcomes: Vec<String> = items(&warm.stats().unwrap_or_else(|_| unreachable!()), "lookups")
        .iter()
        .filter_map(|l| field(&l, "outcome").as_string())
        .collect();
    assert!(outcomes.iter().any(|o| o == "prefix"), "{outcomes:?}");
    held.clear_cache();
    assert_eq!(plane(&warm), plane(&over("released")));
}

fn knobbed() -> Composition {
    let mut held = Composition::new(None);
    held.insert("note", "sample(sin(2*pi*220*t))*0.5\n");
    held.insert("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n");
    held
}

fn knob(cutoff: u32) -> String {
    format!("@tone([0, 1s], x=@note, cutoff={cutoff})")
}

/// `{ rate, until, volatile: [..] }` as a page writes it.
fn config(volatile: &[&str]) -> JsValue {
    let names: js_sys::Array = volatile.iter().map(|n| JsValue::from_str(n)).collect();
    options(&[
        ("until", JsValue::from_str("t >= 0.05s")),
        ("volatile", names.into()),
    ])
}

fn played(held: &Composition, cutoff: u32, volatile: &[&str]) -> Rendering {
    held.rendered(&knob(cutoff), None, config(volatile))
        .unwrap_or_else(|_| unreachable!("the knob at {cutoff} renders"))
}

fn stats_of(of: &Rendering) -> JsValue {
    of.stats().unwrap_or_else(|_| unreachable!("stats answer"))
}

/// `config.volatile` names the parameters a player is moving: what reads one keeps one value
/// in the store, its last, however far the knob moves, and the audio is the same.
#[wasm_bindgen_test]
fn a_volatile_knob_crosses_in_the_config_and_keeps_one_value_per_node() {
    let held = knobbed();
    let cutoff = || &["cutoff"][..];
    played(&held, 900, cutoff());
    let entries = held.cache_entries();

    let again = stats_of(&played(&held, 900, cutoff()));
    assert_eq!(
        field(&again, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&again)
    );

    let next = played(&held, 1300, cutoff());
    let replaced = stats_of(&next);
    assert!(
        field(&replaced, "replaced").as_f64() > Some(0.0),
        "{}",
        as_text(&replaced)
    );
    assert!(
        as_text(&replaced).contains("\"computed_replaced\""),
        "{}",
        as_text(&replaced)
    );
    assert_eq!(held.cache_entries(), entries, "one value per node");
    assert_eq!(
        plane(&next),
        plane(&played(&knobbed(), 1300, &[])),
        "the same audio as a plain render"
    );

    let refused = held
        .rendered(&knob(500), None, config(&["cutof"]))
        .err()
        .unwrap_or_else(|| unreachable!("a name nothing binds refuses"));
    assert!(
        as_text(&field(&refused, "refusal")).contains("render.volatile_unbound"),
        "{}",
        as_text(&field(&refused, "refusal"))
    );
}

#[wasm_bindgen_test]
fn the_cache_budget_is_the_pages_own_and_survives_a_clear() {
    let held = page();
    render(&held, "master");
    held.set_cache_max_bytes(1_024.0);
    assert_eq!(held.cache_max_bytes(), 1_024.0);
    assert!(held.cache_bytes() <= 1_024.0, "a lower cap prunes at once");
    held.clear_cache();
    assert_eq!(held.cache_max_bytes(), 1_024.0, "the ceiling is kept");
    assert_eq!((held.cache_bytes(), held.cache_entries()), (0.0, 0));
}

/// A cache policy crosses by name: the composition's default, and one render's own in its
/// config.
#[wasm_bindgen_test]
fn a_cache_policy_crosses_by_name_and_per_render() {
    let held = page();
    assert_eq!(held.cache_policy(), "all");
    held.set_cache_policy("none")
        .unwrap_or_else(|_| unreachable!("none is a policy"));
    render(&held, "master");
    assert_eq!(held.cache_entries(), 0, "nothing was stored");
    assert!(held.set_cache_policy("some").is_err());

    let at = |policy: &str| {
        held.rendered(
            "@master([0, 1s])",
            None,
            options(&[("cache", JsValue::from_str(policy))]),
        )
    };
    let target = at("target").unwrap_or_else(|_| unreachable!("target is a policy"));
    let stored = field(&stats_of(&target), "stored").as_f64();
    assert!(stored > Some(0.0), "the target was stored");
    assert_eq!(
        stored,
        Some(held.cache_entries() as f64),
        "and nothing else"
    );
    assert!(at("most").is_err());
}

/// A prune policy crosses by name: the default one a render over the cap prunes by, and the
/// one a page passes to prune by now.
#[wasm_bindgen_test]
fn a_prune_policy_crosses_by_name() {
    let held = page();
    assert_eq!(held.prune_policy(), "oldest");
    held.set_prune_policy("forks")
        .unwrap_or_else(|_| unreachable!("forks is a policy"));
    assert_eq!(held.prune_policy(), "forks");
    assert!(held.set_prune_policy("newest").is_err());

    render(&held, "wide");
    render(&held, "master");
    let before = held.cache_evictions();
    held.prune("oldest")
        .unwrap_or_else(|_| unreachable!("oldest is a policy"));
    assert!(
        held.cache_evictions() > before,
        "the first render's values went"
    );
    assert!(held.prune("everything").is_err());
}

/// A located refusal is the contract everywhere else in this engine, so it has to cross as one:
/// data on a thrown `Error`, never a trap that takes the module down with it.
#[wasm_bindgen_test]
fn a_refusal_crosses_as_data_and_the_module_keeps_working() {
    let mut held = page();
    held.insert("master", "@nowhere*2\n");

    let refused = held
        .rendered("@master([0, 1s])", None, options(&[]))
        .err()
        .unwrap_or_else(|| unreachable!("a dangling ref refuses"));

    assert_eq!(
        field(&refused, "name").as_string().as_deref(),
        Some("validation_error"),
        "the CLI's own error code"
    );
    let envelope = field(&refused, "refusal");
    let spelled = as_text(&envelope);
    assert!(
        spelled.contains("\"status\":\"error\"") && spelled.contains("\"diagnostics\""),
        "the envelope the CLI would have printed: {spelled}"
    );
    let found = field(&field(&envelope, "data"), "diagnostics");
    assert_eq!(
        field(&field(&found, "pagination"), "count").as_f64(),
        Some(1.0),
        "a collection carries its own count: {spelled}"
    );
    let first = js_sys::Array::from(&field(&found, "items")).get(0);
    assert!(
        field(&first, "location").is_object(),
        "and it is located: {spelled}"
    );
    let details = field(&field(&envelope, "error"), "details");
    assert_eq!(
        field(&details, "count").as_f64(),
        Some(1.0),
        "`error.details`, the third key the standard mandates: {spelled}"
    );
    assert_eq!(
        items(&details, "codes").get(0).as_string(),
        field(&first, "code").as_string(),
        "and it names the code a page branches on: {spelled}"
    );

    held.insert("master", "@partials/one*0.5\n");
    assert_eq!(
        render(&held, "master").channels(),
        1,
        "one node replaced, and the session renders again"
    );
}

/// Both refusal paths cross the boundary the same way: `name` is the code, `refusal` the
/// envelope. A page branching on one has to find the other beside it.
#[wasm_bindgen_test]
fn a_refusal_the_page_raised_crosses_exactly_as_a_pipeline_one_does() {
    let Err(raised) = page().rendered(
        "@partials/one([0, 1s])",
        Some(vec!["nonsense".to_string()]),
        options(&[]),
    ) else {
        unreachable!("`nonsense` is no reading")
    };
    assert_eq!(
        field(&raised, "name").as_string().as_deref(),
        Some("validation_error"),
        "the same code a pipeline refusal carries"
    );
    let envelope = field(&raised, "refusal");
    assert!(
        field(&envelope, "error").is_object(),
        "and the same envelope"
    );
    assert_eq!(
        items(&field(&envelope, "data"), "diagnostics").length(),
        1,
        "with a located finding of its own: {}",
        as_text(&envelope)
    );
}

/// An interval's ledger is summed over that interval alone, as `sva-cli` sums it.
#[wasm_bindgen_test]
fn a_ledger_is_summed_over_the_interval_asked_for() {
    let held = page()
        .rendered(
            "@master([0.25s, 0.5s])",
            Some(vec!["ledger".to_string()]),
            options(&[]),
        )
        .unwrap_or_else(|_| unreachable!("a ledger answers"));
    let asked = readings(&held);
    let interval = field(&asked, "interval");
    assert_eq!(field(&interval, "start_secs").as_f64(), Some(0.25));
    assert_eq!(field(&interval, "end_secs").as_f64(), Some(0.5));
    let rows = items(&field(&field(&asked, "representations"), "ledger"), "value");
    let spelled = as_text(&asked);
    assert!(rows.length() >= 2, "the target and its ref: {spelled}");
    let rms = field(&rows.get(0), "rms").as_f64().unwrap_or(0.0);
    assert!(
        (rms - 0.5 / 2f64.sqrt()).abs() < 1e-3,
        "a sine at 0.5 over whole periods: {spelled}"
    );
}

#[wasm_bindgen_test]
fn an_outline_crosses_as_the_object_the_cli_puts_under_data() {
    let text = "0.5*lowpass(@note, cutoff=700)";
    let answered = outline(text).unwrap_or_else(|_| unreachable!("it parses"));
    let tree = field(&answered, "outline");
    assert_eq!(field(&tree, "op").as_string().as_deref(), Some("*"));
    let call = field(&tree, "right");
    assert_eq!(field(&call, "name").as_string().as_deref(), Some("lowpass"));
    let named = js_sys::Array::from(&field(&field(&call, "args"), "items")).get(1);
    assert_eq!(field(&named, "name").as_string().as_deref(), Some("cutoff"));
    let at = field(&field(&named, "value"), "span");
    let (start, end) = (
        field(&at, "start").as_f64().unwrap_or(0.0) as usize,
        field(&at, "end").as_f64().unwrap_or(0.0) as usize,
    );
    assert_eq!(
        &text[start..end],
        "700",
        "a span indexes the text it came from"
    );

    let Err(refused) = outline("sin(") else {
        unreachable!("an unclosed call refuses")
    };
    assert_eq!(
        field(&refused, "name").as_string().as_deref(),
        Some("validation_error")
    );
}

#[wasm_bindgen_test]
fn builtins_cross_with_what_each_named_argument_means() {
    let answered = builtins().unwrap_or_else(|_| unreachable!("the vocabulary assembles"));
    let callables = items(&answered, "callables");
    let solver = callables
        .iter()
        .find(|c| field(c, "name").as_string().as_deref() == Some("chaigne_askenfelt"))
        .unwrap_or_else(|| unreachable!("the solver is a builtin"));
    let b = items(&solver, "arguments").get(0);
    assert_eq!(field(&b, "name").as_string().as_deref(), Some("b"));
    assert_eq!(
        field(&b, "meaning").as_string().as_deref(),
        Some("string stiffness (inharmonicity)")
    );
    assert_eq!(field(&b, "unit").as_string().as_deref(), Some("none"));
    assert_eq!(field(&b, "part").as_string().as_deref(), Some("string"));
}

/// An open render ends where its root's support does. A held sine's never does, so its render
/// refuses and its stream plays on for as long as it is pulled.
#[wasm_bindgen_test]
fn an_open_render_ends_where_its_support_does() {
    let mut held = Composition::new(None);
    held.insert("master", "crop(sin(2*pi*440*t), 0s, 0.3s)\n");
    let ended = held
        .rendered("@master", None, options(&[]))
        .unwrap_or_else(|_| unreachable!("the crop ends it"));
    assert_eq!(ended.duration_secs(), 0.3);

    let never = held.rendered("sin(2*pi*100*t)", None, options(&[]));
    let refused = never
        .err()
        .unwrap_or_else(|| unreachable!("a held sine never ends"));
    assert!(
        as_text(&field(&refused, "refusal")).contains("render.no_end"),
        "{}",
        as_text(&refused)
    );
    let sine = now(held.stream("sin(2*pi*100*t)", BLOCK, options(&[])))
        .unwrap_or_else(|e| unreachable!("{}", as_text(&field(&e, "refusal"))));
    let heard = blocks(&sine, 80);
    assert_eq!((heard.len(), sine.end()), (80 * BLOCK, None));
}

const BLOCK: usize = 256;

/// `@<node>` over four seconds, which no test reaches the end of.
fn opened(held: &Composition, node: &str) -> Stream {
    now(held.stream(&format!("@{node}([0, 4s])"), BLOCK, options(&[])))
        .unwrap_or_else(|e| unreachable!("`{node}` streams: {}", as_text(&field(&e, "refusal"))))
}

fn blocks(stream: &Stream, count: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK];
    let mut heard = Vec::with_capacity(count * BLOCK);
    for _ in 0..count {
        let took = stream
            .next(&mut out)
            .unwrap_or_else(|_| unreachable!("a block"));
        heard.extend_from_slice(&out[..took]);
    }
    heard
}

fn refused_as(refused: Option<JsValue>, code: &str) {
    let refused = refused.unwrap_or_else(|| unreachable!("`{code}` refuses"));
    assert!(
        as_text(&field(&refused, "refusal")).contains(code),
        "{}",
        as_text(&refused)
    );
}

/// A sampled filter over a closed form: the stream's blocks are the render's samples. A term
/// added to `@notes` crosses as a handle, and replacing it with one binding `release` at the
/// stream's position closes the envelope there.
#[wasm_bindgen_test]
fn a_stream_crosses_block_by_block_and_a_replaced_term_releases_it() {
    let mut held = page();
    held.insert(
        "filtered",
        "lowpass(sample(@partials/one), cutoff=300, q=0.7)\n",
    );
    held.insert("gated", "release = inf\ncrop(@filtered, 0s, release)\n");
    let stream = opened(&held, "filtered");
    assert_eq!((stream.channels(), stream.sample_rate()), (1, 8000));
    let heard = blocks(&stream, 8000 / BLOCK);
    let whole = plane(&render(&held, "filtered"));
    assert_eq!(heard[..], whole[..heard.len()]);

    let refusal = |e: JsValue| unreachable!("{}", as_text(&field(&e, "refusal")));
    let gated = opened(&held, "notes");
    let key = now(gated.add("@gated")).unwrap_or_else(refusal);
    assert!(blocks(&gated, 3).iter().any(|v| *v != 0.0));
    let at = gated.position() / 8000.0;
    let released = now(gated.replace(key, &format!("@gated(t, release={at})")));
    assert_eq!(released.ok(), Some(true));
    assert!(blocks(&gated, 2).iter().all(|v| *v == 0.0));
    assert_eq!(now(gated.remove(key)).ok(), Some(true));
    assert_eq!(
        now(gated.remove(key)).ok(),
        Some(false),
        "a handle removed is held no more"
    );
    refused_as(now(gated.add("@gated([0, 1s])")).err(), "validation_error");
    refused_as(now(gated.edit("@notes([0, 1s])")).err(), "validation_error");
}

/// Component `c` of a block starts at `c * block` in the array a page hands over.
#[wasm_bindgen_test]
fn a_wide_stream_lays_each_component_a_block_apart() {
    let held = page();
    let stream = opened(&held, "wide");
    assert_eq!(stream.channels(), 2);
    let mut out = vec![0.0f32; 2 * BLOCK];
    let took = stream
        .next(&mut out)
        .unwrap_or_else(|_| unreachable!("a block"));
    let wide = render(&held, "wide");
    for c in 0..2 {
        let whole = wide
            .samples(c)
            .unwrap_or_else(|_| unreachable!("two components"));
        assert_eq!(
            out[c * BLOCK..c * BLOCK + took],
            whole[..took],
            "component {c}"
        );
    }
}

/// An open stream ends where a render of the same target does, here where `exp` underflows.
#[wasm_bindgen_test]
fn an_open_stream_ends_where_the_render_does() {
    let mut held = Composition::new(None);
    held.insert("master", "sin(2*pi*100*t)*exp(0 - 300*t)\n");
    let stream = now(held.stream("@master", BLOCK, options(&[])))
        .unwrap_or_else(|_| unreachable!("a decay ends"));
    assert_eq!(stream.end(), None, "no block has reached the end yet");
    let mut out = vec![0.0f32; BLOCK];
    let mut heard = Vec::new();
    loop {
        let took = stream
            .next(&mut out)
            .unwrap_or_else(|_| unreachable!("a block"));
        if took == 0 {
            break;
        }
        heard.extend_from_slice(&out[..took]);
    }
    let end = stream.end().unwrap_or_else(|| unreachable!("an end"));
    assert_eq!(heard.len() as f64, end);
    assert_eq!(stream.next(&mut out).ok(), Some(0), "nothing after the end");
    let whole = held
        .rendered("@master", None, options(&[]))
        .unwrap_or_else(|_| unreachable!("the same decay ends"));
    assert_eq!(heard[..], plane(&whole)[..]);
}

#[wasm_bindgen_test]
fn a_stream_refuses_what_it_cannot_take_at_the_boundary() {
    let held = page();
    let stream = opened(&held, "partials/one");
    let mut short = vec![0.0f32; BLOCK - 1];
    refused_as(stream.next(&mut short).err(), "wasm.bad_argument");
    let open = |options: JsValue| now(held.stream("@master([0, 1s])", BLOCK, options)).err();
    refused_as(open(JsValue::from(3)), "wasm.bad_argument");
    refused_as(
        open(self::options(&[("until", JsValue::from_str("2"))])),
        "validation_error",
    );
    refused_as(
        open(self::options(&[("release", JsValue::from_str("soon"))])),
        "wasm.bad_argument",
    );
    let empty = now(held.stream("@master([0, 1s])", 0, options(&[])));
    refused_as(empty.err(), "engine.no_stream");
}

/// A sine is one line and its mirror, turned at each sample of its one period, 80 samples of
/// 100 Hz at 8 kHz; a render prices its schedule.
#[wasm_bindgen_test]
fn work_crosses_as_whole_counts_from_a_stream_and_a_render() {
    let held = page();
    let stream = opened(&held, "partials/one");
    blocks(&stream, 4);
    let work = stream
        .work()
        .unwrap_or_else(|_| unreachable!("a stream's work"));
    let count = |of: &JsValue, name: &str| field(of, name).as_f64();
    let samples = (4 * BLOCK) as f64;
    assert_eq!(count(&work, "samples"), Some(samples));
    assert_eq!(count(&work, "waves"), Some(2.0 * 80.0));
    assert!(count(&work, "priced_flops").is_some_and(|f| f > 0.0));

    let whole = render(&held, "partials/one")
        .work()
        .unwrap_or_else(|_| unreachable!("a render's work"));
    assert_eq!(count(&whole, "samples"), Some(8000.0));
    assert!(count(&whole, "priced_flops").is_some_and(|f| f > 0.0));
    assert!(field(&whole, "waves").is_null());
}

/// An in-memory directory handle. A missing name rejects as `NotFoundError`; a write lands on
/// close; a file moves whole. A file is open from `createWritable` to `close` and while it moves,
/// and removing or moving it then rejects, as OPFS does. Each call installs fresh exclusive,
/// queued `navigator.locks`.
fn fake_directory() -> JsValue {
    js_sys::Function::new_no_args(
        r#"
        const held = new Map();
        const locks = {
            async request(name, ...rest) {
                const callback = rest[rest.length - 1];
                const options = rest.length > 1 ? rest[0] : {};
                if (options.ifAvailable && held.has(name)) return callback(null);
                while (held.has(name)) await held.get(name);
                const lock = { name, mode: "exclusive" };
                let release;
                held.set(name, new Promise((r) => { release = r; }));
                try { return await callback(lock); }
                finally { held.delete(name); release(); }
            },
            async query() {
                return { held: [...held.keys()].map((name) => ({ name, mode: "exclusive" })), pending: [] };
            },
        };
        if (typeof globalThis.navigator !== "object" || globalThis.navigator === null) {
            Object.defineProperty(globalThis, "navigator", { value: {}, configurable: true, writable: true });
        }
        Object.defineProperty(globalThis.navigator, "locks", { value: locks, configurable: true });
        const missing = () => Object.assign(new Error("missing"), { name: "NotFoundError" });
        const busy = () => Object.assign(new Error("open elsewhere"), { name: "NoModificationAllowedError" });
        const directory = (name) => {
            const files = new Map();
            const dirs = new Map();
            const open = new Map();
            const opened = (name, by) => {
                const count = (open.get(name) || 0) + by;
                if (count === 0) open.delete(name); else open.set(name, count);
            };
            const held = {
                name,
                files,
                dirs,
                open,
                async getFileHandle(name, options) {
                    if (!files.has(name)) {
                        if (!(options && options.create)) throw missing();
                        files.set(name, new Uint8Array(0));
                    }
                    return {
                        async getFile() {
                            const bytes = files.get(name);
                            if (!bytes) throw missing();
                            const blob = (held) => ({
                                size: held.length,
                                async arrayBuffer() { return held.slice().buffer; },
                                slice(start, end) { return blob(held.slice(start, end)); },
                            });
                            return blob(bytes);
                        },
                        async createWritable() {
                            let pending = new Uint8Array(0);
                            opened(name, 1);
                            return {
                                async write(data) { pending = new Uint8Array(data); },
                                async close() { files.set(name, pending); opened(name, -1); },
                            };
                        },
                        async move(into, as) {
                            if (!files.has(name)) throw missing();
                            if (open.has(name) || into.open.has(as)) throw busy();
                            opened(name, 1);
                            into.open.set(as, (into.open.get(as) || 0) + 1);
                            await null;
                            const bytes = files.get(name);
                            files.delete(name);
                            into.files.set(as, bytes);
                            opened(name, -1);
                            const left = into.open.get(as) - 1;
                            if (left === 0) into.open.delete(as); else into.open.set(as, left);
                        },
                    };
                },
                async getDirectoryHandle(name, options) {
                    if (!dirs.has(name)) {
                        if (!(options && options.create)) throw missing();
                        dirs.set(name, directory(name));
                    }
                    return dirs.get(name);
                },
                async removeEntry(name) {
                    if (open.has(name)) throw busy();
                    if (!files.delete(name) && !dirs.delete(name)) throw missing();
                },
                keys() {
                    const names = [...files.keys(), ...dirs.keys()];
                    let at = 0;
                    return { async next() { return at < names.length ? { done: false, value: names[at++] } : { done: true }; } };
                },
            };
            return held;
        };
        return directory("sva");
        "#,
    )
    .call0(&JsValue::NULL)
    .unwrap_or_else(|_| unreachable!("the fake builds"))
}

/// Every value's file, beside the store's own index.
fn values_in(dir: &JsValue) -> usize {
    js_sys::Array::from(&field(dir, "files"))
        .iter()
        .filter_map(|pair| js_sys::Array::from(&pair).get(0).as_string())
        .filter(|name| name != "index")
        .count()
}

async fn over_store(dir: &JsValue) -> Composition {
    let mut held = Composition::open(None, Some(dir.clone().into())).await;
    held.insert("master", "sample(sin(2*pi*100*t))*0.5\n");
    held
}

async fn stored_render(held: &Composition) -> JsValue {
    held.render("@master([0, 0.1s])", None, options(&[]))
        .await
        .unwrap_or_else(|e| unreachable!("it renders: {}", as_text(&e)))
        .stats()
        .unwrap_or_else(|_| unreachable!("stats answer"))
}

#[wasm_bindgen_test]
async fn a_render_writes_the_directory_nothing_until_the_page_persists() {
    let dir = fake_directory();
    let held = over_store(&dir).await;
    stored_render(&held).await;
    assert_eq!(values_in(&dir), 0, "a render writes nothing");
    let written = held
        .persist()
        .await
        .unwrap_or_else(|_| unreachable!("persisted"));
    assert!(written > 0);
    assert_eq!(values_in(&dir), written);

    let warm = stored_render(&over_store(&dir).await).await;
    assert_eq!(
        field(&warm, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&warm)
    );
    assert_eq!(Composition::new(None).persist().await.ok(), Some(0));
}

/// A staging area whose lock no page holds, as a closed tab leaves one, goes when a store opens.
#[wasm_bindgen_test]
async fn opening_a_store_sweeps_a_staging_area_no_page_holds() {
    let dir = fake_directory();
    let make: js_sys::Function = field(&dir, "getDirectoryHandle").into();
    let left = make
        .call2(
            &dir,
            &"staging-left".into(),
            &options(&[("create", true.into())]),
        )
        .unwrap_or_else(|_| unreachable!("the fake makes a directory"));
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(left))
        .await
        .unwrap_or_else(|_| unreachable!("it settles"));
    over_store(&dir).await;
    let names: Vec<String> = js_sys::Array::from(&field(&dir, "dirs"))
        .iter()
        .filter_map(|pair| js_sys::Array::from(&pair).get(0).as_string())
        .collect();
    assert!(!names.contains(&"staging-left".to_string()), "{names:?}");
    assert_eq!(names.len(), 1, "the page's own staging area: {names:?}");
}

/// A note one page's worker rendered and persisted, a stream opened in another over the same
/// directory reads from the store: its blocks are a memory-only stream's.
#[wasm_bindgen_test]
async fn a_stream_reads_a_note_another_worker_persisted() {
    let dir = fake_directory();
    let nodes = |held: &mut Composition| {
        held.insert(
            "blip",
            "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.1s)\n",
        );
        held.insert("echo", "x + 0.5*self[idx(t - 0.05s)]\n");
    };
    let (mut renderer, mut player) = (over_store(&dir).await, over_store(&dir).await);
    let mut memory = Composition::new(None);
    for held in [&mut renderer, &mut player, &mut memory] {
        nodes(held);
    }
    renderer
        .render("@blip([0, 0.1s], f0=200)", None, options(&[]))
        .await
        .unwrap_or_else(|e| unreachable!("it renders: {}", as_text(&e)));
    renderer
        .persist()
        .await
        .unwrap_or_else(|_| unreachable!("persisted"));

    let mut heard = Vec::new();
    for held in [&player, &memory] {
        let stream = held
            .stream("@echo([0, 1s], x=@notes)", BLOCK, options(&[]))
            .await
            .unwrap_or_else(|e| unreachable!("it streams: {}", as_text(&e)));
        stream
            .add("@blip(t - 512sp, f0=200)")
            .await
            .unwrap_or_else(|e| unreachable!("added: {}", as_text(&e)));
        let mut heard_here = blocks(&stream, 8);
        stream.fetch().await;
        heard_here.extend(blocks(&stream, 8));
        heard.push((heard_here, stream.stats()));
    }
    let hits = |stats: &Result<JsValue, JsValue>| {
        field(stats.as_ref().ok().unwrap_or(&JsValue::NULL), "hits").as_f64()
    };
    assert_eq!(heard[0].0, heard[1].0);
    let (store, memory) = (hits(&heard[0].1), hits(&heard[1].1));
    assert!(
        store > memory,
        "the store answers the note: {store:?} hits against {memory:?}"
    );
}

/// An edit awaits the store without holding the stream: the stream plays and answers
/// meanwhile, an edit issued meanwhile is taken, and both land.
#[wasm_bindgen_test]
async fn a_stream_plays_on_while_its_edits_await_the_store() {
    let dir = fake_directory();
    let held = over_store(&dir).await;
    let live = options(&[("live", JsValue::TRUE)]);
    let stream = held
        .stream("@notes([0, 1s])", BLOCK, live)
        .await
        .unwrap_or_else(|e| unreachable!("it streams: {}", as_text(&e)));
    let mut first = std::pin::pin!(stream.add("@master"));
    let polled = first.as_mut().poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        polled.is_pending(),
        "the lookup of a node the stream has not met awaits the directory"
    );
    let mut out = vec![0.0f32; BLOCK];
    assert_eq!(stream.next(&mut out).ok(), Some(BLOCK));
    assert_eq!(stream.position(), BLOCK as f64);
    let mut second = std::pin::pin!(stream.add("@master(t - 0.05s)"));
    let _ = second
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert_eq!(stream.next(&mut out).ok(), Some(BLOCK));
    let first = first
        .await
        .unwrap_or_else(|e| unreachable!("added: {}", as_text(&e)));
    let second = second
        .await
        .unwrap_or_else(|e| unreachable!("added: {}", as_text(&e)));
    assert_ne!(first, second);
    assert!(
        blocks(&stream, 4).iter().any(|v| *v != 0.0),
        "the terms landed"
    );
}

async fn warmed(held: &Composition, readings: &[&str]) -> JsValue {
    let readings: js_sys::Array = readings.iter().map(|r| JsValue::from_str(r)).collect();
    held.warm(
        "@master([0, 0.1s])",
        options(&[("readings", readings.into())]),
    )
    .await
    .unwrap_or_else(|e| unreachable!("it warms: {}", as_text(&e)))
}

/// A warm answers stats and, only where asked, readings: never a sample. Persisted, the target
/// renders from the store alone; warmed again, nothing is computed.
#[wasm_bindgen_test]
async fn a_warm_answers_no_samples_and_readings_only_when_asked() {
    let dir = fake_directory();
    let held = over_store(&dir).await;
    let bare = warmed(&held, &[]).await;
    let keys: Vec<String> = js_sys::Object::keys(bare.unchecked_ref::<js_sys::Object>())
        .iter()
        .filter_map(|key| key.as_string())
        .collect();
    assert_eq!(keys, ["stats", "representations"], "{}", as_text(&bare));
    assert!(field(&bare, "representations").is_null());
    assert!(field(&field(&bare, "stats"), "computed").as_f64() > Some(0.0));
    assert_eq!(values_in(&dir), 0, "a warm writes nothing");
    held.persist()
        .await
        .unwrap_or_else(|_| unreachable!("persisted"));

    let reader = over_store(&dir).await;
    let hit = stored_render(&reader).await;
    assert_eq!(
        field(&hit, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&hit)
    );
    let again = warmed(&reader, &[]).await;
    let stats = field(&again, "stats");
    assert_eq!(
        field(&stats, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&stats)
    );

    let read = warmed(&reader, &["envelope"]).await;
    let asked = field(&field(&read, "representations"), "representations");
    assert!(
        !field(&asked, "envelope").is_undefined(),
        "{}",
        as_text(&read)
    );
    assert!(field(&asked, "samples").is_undefined());
    let refused = reader
        .warm(
            "@master([0, 0.1s])",
            options(&[("readings", js_sys::Array::of1(&"samples".into()).into())]),
        )
        .await;
    refused_as(refused.err(), "wasm.bad_argument");
}

/// `index` set to what an older format wrote, over the values a newer one left under it.
fn aged(dir: &JsValue) {
    let files: js_sys::Map = field(dir, "files").into();
    let text = js_sys::Uint8Array::from(&b"sva store format 1\n"[..]);
    files.set(&"index".into(), &text);
}

/// What `work` resolves to, run as its own task beside the others.
fn task(work: impl Future<Output = Result<JsValue, JsValue>> + 'static) -> js_sys::Promise {
    wasm_bindgen_futures::future_to_promise(work)
}

/// A directory holding the values of `over_store`'s render, under an older format's index.
async fn aged_directory() -> JsValue {
    let dir = fake_directory();
    let first = over_store(&dir).await;
    stored_render(&first).await;
    first
        .persist()
        .await
        .unwrap_or_else(|e| unreachable!("persisted: {}", as_text(&e)));
    aged(&dir);
    dir
}

/// Four workers warm and one renders the same sound, each over its own store on `dir`, all at
/// once, and each persists.
async fn workers(dir: &JsValue) {
    let workers: js_sys::Array = (0..5)
        .map(|n| {
            let dir = dir.clone();
            task(async move {
                let mut held = Composition::open(None, Some(dir.into())).await;
                held.insert("master", "sample(sin(2*pi*100*t))*0.5\n");
                match n {
                    0 => drop(
                        held.render("@master([0, 0.1s])", None, options(&[]))
                            .await?,
                    ),
                    _ => drop(held.warm("@master([0, 0.1s])", options(&[])).await?),
                }
                Ok(JsValue::from(held.persist().await?))
            })
        })
        .collect();
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::all(&workers))
        .await
        .unwrap_or_else(|e| unreachable!("every worker succeeds: {}", as_text(&e)));
}

/// On one directory an older format left, every open wipes or waits, every persist commits
/// whole, and no worker fails.
#[wasm_bindgen_test]
async fn workers_opening_one_aged_directory_at_once_each_warm_render_and_persist() {
    let dir = aged_directory().await;
    let values = values_in(&dir);
    workers(&dir).await;
    assert_eq!(values_in(&dir), values, "each value stored once, whole");
    let warm = stored_render(&over_store(&dir).await).await;
    assert_eq!(
        field(&warm, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&warm)
    );
}

/// An entry another holder keeps open is left by the wipe and by every commit over it, and no
/// worker fails; once it closes, the next persist writes it.
#[wasm_bindgen_test]
async fn an_entry_open_elsewhere_fails_no_worker_and_the_next_persist_writes_it() {
    let dir = aged_directory().await;
    let files: js_sys::Map = field(&dir, "files").into();
    let name = files
        .keys()
        .into_iter()
        .filter_map(|name| name.ok()?.as_string())
        .find(|name| name != "index")
        .unwrap_or_else(|| unreachable!("a value's file"));
    let handle = js_call(&dir, "getFileHandle", &[name.clone().into()]).await;
    let writable = js_call(&handle, "createWritable", &[]).await;
    workers(&dir).await;
    assert!(files.has(&name.clone().into()), "open, so left");
    js_call(&writable, "close", &[]).await;

    let held = over_store(&dir).await;
    let cold = stored_render(&held).await;
    assert!(field(&cold, "computed").as_f64() > Some(0.0));
    held.persist()
        .await
        .unwrap_or_else(|e| unreachable!("persisted: {}", as_text(&e)));
    let warm = stored_render(&over_store(&dir).await).await;
    assert_eq!(
        field(&warm, "computed").as_f64(),
        Some(0.0),
        "{}",
        as_text(&warm)
    );
}

/// Every call on `dir` and the directories in it rejects from now on, as on a revoked handle.
fn revoked(dir: &JsValue) {
    js_sys::Function::new_with_args(
        "dir",
        r#"
        const refuse = async () => { throw Object.assign(new Error("revoked"), { name: "SecurityError" }); };
        const revoke = (dir) => {
            for (const name of ["getFileHandle", "getDirectoryHandle", "removeEntry"]) dir[name] = refuse;
            dir.keys = () => ({ next: refuse });
            for (const inner of dir.dirs.values()) revoke(inner);
        };
        revoke(dir);
        "#,
    )
    .call1(&JsValue::NULL, dir)
    .unwrap_or_else(|_| unreachable!("the fake revokes"));
}

async fn channel(held: &Composition) -> Vec<f32> {
    held.render("@master([0, 0.1s])", None, options(&[]))
        .await
        .unwrap_or_else(|e| unreachable!("it renders: {}", as_text(&e)))
        .samples(0)
        .unwrap_or_else(|e| unreachable!("its samples: {}", as_text(&e)))
}

/// A directory that rejects every call fails no render and no warm, whether it failed after the
/// store opened or before: each renders a storeless composition's samples.
#[wasm_bindgen_test]
async fn a_directory_that_rejects_every_call_fails_no_render_and_no_warm() {
    let dir = fake_directory();
    let before = over_store(&dir).await;
    revoked(&dir);
    let after = over_store(&dir).await;
    let mut memory = Composition::new(None);
    memory.insert("master", "sample(sin(2*pi*100*t))*0.5\n");
    let bare = channel(&memory).await;
    for held in [&before, &after] {
        assert_eq!(channel(held).await, bare);
        let read = warmed(held, &["envelope"]).await;
        assert!(!field(&read, "representations").is_null());
    }
    assert_eq!(after.persist().await.ok(), Some(0));
}

/// `of.method(...args)`, awaited.
async fn js_call(of: &JsValue, method: &str, args: &[JsValue]) -> JsValue {
    let method: js_sys::Function = field(of, method).into();
    let args: js_sys::Array = args.iter().collect();
    let promise = method
        .apply(of, &args)
        .unwrap_or_else(|_| unreachable!("`{method:?}` answers"));
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
        .await
        .unwrap_or_else(|e| unreachable!("it settles: {}", as_text(&e)))
}

/// FNV-1a over every component's f64 bits, little-endian: a native `sva-cli render '@master([0,
/// 0.1s])' --representation samples --rate 8000` over `mixed()`'s nodes hashes to this. The
/// native build is scalar, so simd128 may vectorise but never fuse or reassociate.
const NATIVE_BITS: u64 = 0xb439_d354_438c_dfd3;

/// No `sin` in the source: wasm's libm and the native one differ in the last bit, scalar or not.
fn mixed() -> Composition {
    let mut held = Composition::new(None);
    held.insert("osc", "sample(40*t*(0.1 - t)) + 0.3*sample(rand(t))\n");
    held.insert("tone", "lowpass(@osc, cutoff=800, q=0.7)\n");
    held.insert("echo", "0.7*@tone + 0.5*self[idx(t) - 37]\n");
    held.insert(
        "mix",
        "0.5*@echo - 0.25*highpass(@tone, cutoff=200, q=0.7)/(2 + @osc)\n",
    );
    held.insert("master", "join(@mix, @tone)*(1 + @osc) - @echo\n");
    held
}

#[wasm_bindgen_test]
fn filters_sums_gains_and_a_loop_render_the_native_bits() {
    let asked = Some(vec!["samples".to_string()]);
    let one = mixed()
        .rendered("@master([0, 0.1s])", asked, options(&[]))
        .unwrap_or_else(|_| unreachable!("`master` renders"));
    let read = field(&readings(&one), "representations");
    let components = items(&field(&field(&read, "samples"), "value"), "components");
    assert_eq!(components.length(), 2);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for component in components.iter() {
        let values = js_sys::Array::from(&field(&field(&component, "values"), "items"));
        assert_eq!(values.length(), 800, "the whole reading, under the cap");
        for v in values.iter() {
            let bits = v.as_f64().unwrap_or_else(|| unreachable!("a number"));
            for b in bits.to_le_bytes() {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    assert_eq!(
        hash, NATIVE_BITS,
        "the wasm render's bits are the native ones"
    );
}
