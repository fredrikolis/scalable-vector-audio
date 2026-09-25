// Concern: drives every JS export through real wasm | Non-concern: what a sample is worth (sva-engine), the pipeline (sva-cli) | IO: (a built composition) -> assertions
#![cfg(target_arch = "wasm32")]

//! The gap a native test cannot see: `wasm32-unknown-unknown` has no clock and no threads, and
//! reaching for either compiles clean and traps only at RUNTIME. Hence one session for the
//! whole surface. Run it with `wasm-pack test --node crates/sva-wasm`.

use sva_wasm::{Composition, Rendering, builtins, outline};
use wasm_bindgen::JsValue;
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

fn page() -> Composition {
    let mut held = Composition::new(Some("a-registry".to_string()));
    held.insert("master", "@partials/one*0.5\n");
    held.insert("partials/one", "sin(2*pi*100*t)\n");
    held.insert("wide", "join(sin(2*pi*100*t), sin(2*pi*200*t))\n");
    held.insert("unreached", "this is not an expression (\n");
    held
}

fn render(of: &Composition, target: &str) -> Rendering {
    of.render(
        Some(target.to_string()),
        Some(8000),
        JsValue::UNDEFINED,
        None,
        None,
        None,
    )
    .unwrap_or_else(|_| unreachable!("`{target}` renders"))
}

fn of_default(held: &Composition) -> Rendering {
    held.render(None, Some(8000), JsValue::UNDEFINED, None, None, None)
        .unwrap_or_else(|_| unreachable!("`master` renders"))
}

fn plane(of: &Rendering) -> Vec<f32> {
    of.samples(0, None, None)
        .unwrap_or_else(|_| unreachable!("one component"))
}

#[wasm_bindgen_test]
fn every_export_survives_the_boundary() {
    let held = page();

    assert_eq!(
        of_default(&held).target(),
        "master",
        "an unset target renders `master`"
    );

    let one = render(&held, "partials/one");
    assert_eq!(one.sample_rate(), 8000);
    assert_eq!(one.channels(), 1);
    assert_eq!(one.duration_secs(), 1.0);

    let drawn = plane(&one);
    assert_eq!(drawn.len(), 8000, "a typed array of every sample");
    let want = (2.0 * std::f64::consts::PI * 100.0 * (2.0 / 8000.0)).sin() as f32;
    assert!((drawn[2] - want).abs() < 1e-4, "{} vs {want}", drawn[2]);

    let part = one
        .samples(0, Some(0.25), Some(0.5))
        .unwrap_or_else(|_| unreachable!("one component"));
    assert_eq!(part.len(), 2000, "a window narrows the array");

    for (from, to) in [
        (Some(-1.0), None),
        (Some(0.9), Some(0.1)),
        (Some(5.0), Some(10.0)),
        (None, Some(9.0)),
    ] {
        assert!(
            one.samples(0, from, to).is_err(),
            "a window {from:?}..{to:?} this rendering does not cover refuses, as on argv"
        );
    }

    let probed = render(&held, "@partials/one*0.5");
    assert_eq!(probed.target(), "probe", "math renders as the CLI's probe");

    let wide = render(&held, "wide");
    assert_eq!(wide.channels(), 2);
    assert!(
        wide.samples(2, None, None).is_err(),
        "a component that is not there refuses, and does not trap"
    );

    assert!(held.cache_max_bytes() > 0.0, "a default budget is held");
}

#[wasm_bindgen_test]
fn a_representation_crosses_as_the_object_the_cli_puts_under_data() {
    let one = render(&page(), "partials/one");

    let asked = one
        .query("envelope", None, None)
        .unwrap_or_else(|_| unreachable!("envelope answers"));
    let spelled = as_text(&asked);
    assert!(
        spelled.contains("\"target\":\"partials/one\"") && spelled.contains("\"envelope\":"),
        "the CLI's own keys: {spelled}"
    );
    assert_eq!(field(&asked, "sample_rate").as_f64(), Some(8000.0));

    let capped = one
        .query("samples", None, None)
        .unwrap_or_else(|_| unreachable!("samples answers"));
    let component = items(&field(&field(&capped, "samples"), "value"), "components").get(0);
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
    let one = render(&page(), "partials/one");
    for name in [
        "lines",
        "atoms",
        "samples",
        "ledger",
        "loudness",
        "envelope",
        "arguments",
    ] {
        let answered = one
            .query(name, None, None)
            .unwrap_or_else(|_| unreachable!("`{name}` answers"));
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
        if let Err(refused) = one.query(name, None, None) {
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
            one.query(gone, None, None).is_err(),
            "`{gone}` is not a reading this surface answers"
        );
    }
}

/// A window narrows a reading taken off the buffer, and the envelope names the window it ran
/// over rather than the render's own.
#[wasm_bindgen_test]
fn a_window_narrows_a_query_and_the_envelope_says_which_one_ran() {
    let one = render(&page(), "partials/one");
    let whole = one
        .query("samples", None, None)
        .unwrap_or_else(|_| unreachable!("samples answers"));
    let part = one
        .query("samples", Some(0.25), Some(0.5))
        .unwrap_or_else(|_| unreachable!("samples answers"));

    assert_eq!(
        field(&field(&whole, "window"), "end_secs").as_f64(),
        Some(1.0)
    );
    let window = field(&part, "window");
    assert_eq!(field(&window, "start_secs").as_f64(), Some(0.25));
    assert_eq!(field(&window, "end_secs").as_f64(), Some(0.5));
    let count = |held: &JsValue| {
        let component = items(&field(&field(held, "samples"), "value"), "components").get(0);
        field(&field(&field(&component, "values"), "pagination"), "count").as_f64()
    };
    assert_eq!(count(&whole), Some(8000.0));
    assert_eq!(count(&part), Some(2000.0), "the window really narrowed it");
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

/// The object `sva-cli render` puts under `data.cache.stats`, with the second of two
/// identical renders answered wholly from the composition's own store.
#[wasm_bindgen_test]
fn stats_cross_as_the_cli_object_and_a_repeated_render_is_all_hits() {
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
        field(&field(&warm, "hits"), "memory").as_f64(),
        Some(f64::from(lookups.length())),
        "every lookup a hit: {}",
        as_text(&warm)
    );
    assert_eq!(
        field(&field(&warm, "hits"), "persistent").as_f64(),
        Some(0.0)
    );
    assert_eq!(field(&field(&warm, "hits"), "volatile").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "computed").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "stored").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "slotted").as_f64(), Some(0.0));
    assert_eq!(field(&warm, "replaced").as_f64(), Some(0.0));
    assert_eq!(
        field(&warm, "nodes").as_f64(),
        field(&cold, "nodes").as_f64()
    );
    let first = lookups.get(0);
    for key in ["node", "key", "kind"] {
        assert!(field(&first, key).is_string(), "{key}: {}", as_text(&first));
    }
    assert_eq!(field(&first, "outcome").as_string().as_deref(), Some("hit"));
    assert_eq!(field(&first, "tier").as_string().as_deref(), Some("memory"));

    let first_cold = items(&cold, "lookups").get(0);
    assert_eq!(
        field(&first_cold, "outcome").as_string().as_deref(),
        Some("computed_stored")
    );
    assert!(
        field(&first_cold, "tier").is_undefined(),
        "a miss has no tier"
    );
}

fn knob(cutoff: u32) -> String {
    format!("@tone(t, x=@note, cutoff={cutoff})")
}

fn played(held: &Composition, cutoff: u32, volatile: Option<Vec<String>>) -> Rendering {
    held.render(
        Some(knob(cutoff)),
        Some(8000),
        JsValue::from(0.05),
        volatile,
        None,
        None,
    )
    .unwrap_or_else(|_| unreachable!("the knob at {cutoff} renders"))
}

fn stats_of(of: &Rendering) -> JsValue {
    of.stats().unwrap_or_else(|_| unreachable!("stats answer"))
}

/// The fourth argument names the parameters a player is moving: what reads one lands in a slot
/// the composition owns, the memory store is left as it was, and the audio is the same.
#[wasm_bindgen_test]
fn a_volatile_knob_crosses_as_a_fourth_argument_and_keeps_to_its_slots() {
    let mut held = Composition::new(None);
    held.insert("note", "sample(sin(2*pi*220*t))*0.5\n");
    held.insert("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n");
    let cutoff = || Some(vec!["cutoff".to_string()]);
    let plain = plane(&played(&held, 500, None));
    let bytes = held.cache_bytes();
    assert_eq!(held.volatile_bytes(), 0.0);

    let moved = played(&held, 900, cutoff());
    let cold = stats_of(&moved);
    assert!(
        field(&cold, "slotted").as_f64() > Some(0.0),
        "{}",
        as_text(&cold)
    );
    assert_eq!(
        field(&cold, "stored").as_f64(),
        Some(0.0),
        "{}",
        as_text(&cold)
    );
    assert!(held.volatile_bytes() > 0.0);
    assert_eq!(held.cache_bytes(), bytes, "the memory store is as it was");

    let again = stats_of(&played(&held, 900, cutoff()));
    assert!(field(&field(&again, "hits"), "volatile").as_f64() > Some(0.0));
    let tiers: Vec<String> = items(&again, "lookups")
        .iter()
        .filter_map(|l| field(&l, "tier").as_string())
        .collect();
    assert!(tiers.iter().any(|t| t == "volatile"), "{}", as_text(&again));

    let back = played(&held, 500, cutoff());
    assert_eq!(
        plane(&back),
        plain,
        "a value the store holds is read from it"
    );
    assert_eq!(field(&stats_of(&back), "computed").as_f64(), Some(0.0));

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
    assert_eq!(
        plane(&next),
        plane(&played(&held, 1300, None)),
        "the same audio as a plain render"
    );

    let default = held.volatile_max_bytes();
    assert!(default > 0.0);
    held.set_volatile_max_bytes(0.0);
    assert_eq!(
        (held.volatile_max_bytes(), held.volatile_bytes()),
        (0.0, 0.0)
    );
    held.set_volatile_max_bytes(default);
    played(&held, 700, cutoff());
    assert!(held.volatile_bytes() > 0.0);
    held.clear_volatile();
    assert_eq!(held.volatile_bytes(), 0.0);
    held.clear_cache();

    let refused = held
        .render(
            Some(knob(500)),
            Some(8000),
            JsValue::from(0.05),
            Some(vec!["cutof".to_string()]),
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| unreachable!("a name nothing binds refuses"));
    assert!(
        as_text(&field(&refused, "refusal")).contains("render.volatile_unbound"),
        "{}",
        as_text(&field(&refused, "refusal"))
    );
}

/// Node has no origin-private file system, so the factory resolves the memory-only
/// composition it falls back to everywhere one cannot be opened, and says so.
#[wasm_bindgen_test]
async fn a_persistent_composition_without_a_file_system_is_memory_only() {
    let mut held = Composition::open_persistent(None, "a-registry".to_string(), None)
        .await
        .unwrap_or_else(|_| unreachable!("a fallback, not a refusal"));
    assert!(!held.is_persistent());
    assert_eq!(held.persistent_bytes(), 0.0, "no pack holds anything");
    assert_eq!(held.persistent_max_bytes(), 0.0);
    assert!(
        held.cache_max_bytes() > 0.0,
        "the memory tier still has its budget"
    );
    held.insert("master", "sin(2*pi*100*t)\n");
    assert_eq!(of_default(&held).channels(), 1, "and it renders");

    for name in ["", "..", "a/b"] {
        assert!(
            Composition::open_persistent(None, name.to_string(), None)
                .await
                .is_err(),
            "`{name}` names no one directory"
        );
    }
}

#[wasm_bindgen_test]
fn the_cache_budget_is_the_pages_own_and_survives_a_clear() {
    let mut held = page();
    held.bound_cache(1_024.0);
    assert_eq!(held.cache_max_bytes(), 1_024.0);
    held.clear_cache();
    assert_eq!(held.cache_max_bytes(), 1_024.0, "the ceiling is kept");
    assert_eq!(held.cache_bytes(), 0.0);
}

/// A located refusal is the contract everywhere else in this engine, so it has to cross as one:
/// data on a thrown `Error`, never a trap that takes the module down with it.
#[wasm_bindgen_test]
fn a_refusal_crosses_as_data_and_the_module_keeps_working() {
    let mut held = page();
    held.insert("master", "@nowhere*2\n");

    let refused = held
        .render(None, Some(8000), JsValue::UNDEFINED, None, None, None)
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
        of_default(&held).target(),
        "master",
        "one node replaced, and the session renders again"
    );
}

/// Both refusal paths cross the boundary the same way: `name` is the code, `refusal` the
/// envelope. A page branching on one has to find the other beside it.
#[wasm_bindgen_test]
fn a_refusal_the_page_raised_crosses_exactly_as_a_pipeline_one_does() {
    let one = render(&page(), "partials/one");
    let Err(raised) = one.query("nonsense", None, None) else {
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

/// `sva-cli render --as ledger --from --to` answers a windowed ledger, so a page asking the
/// same window gets one too, summed over that window alone.
#[wasm_bindgen_test]
fn a_ledger_narrows_to_the_window_asked_for() {
    let held = of_default(&page());
    let asked = held
        .query("ledger", Some(0.25), Some(0.5))
        .unwrap_or_else(|_| unreachable!("a windowed ledger answers"));
    let window = field(&asked, "window");
    assert_eq!(field(&window, "start_secs").as_f64(), Some(0.25));
    assert_eq!(field(&window, "end_secs").as_f64(), Some(0.5));
    let rows = items(&field(&asked, "ledger"), "value");
    let spelled = as_text(&asked);
    assert!(rows.length() >= 2, "the target and its ref: {spelled}");
    let master = rows.get(0);
    assert_eq!(
        field(&master, "node").as_string().as_deref(),
        Some("master")
    );
    let rms = field(&master, "rms").as_f64().unwrap_or(0.0);
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

/// Echo `k` of a 50 ms burst is `0.35^k` loud; the fifteenth is the last at or over
/// `2^-24`, and it ends at `15*0.25 + 0.05 = 3.8` seconds.
#[wasm_bindgen_test]
fn a_silent_render_ends_where_its_last_echo_is_heard() {
    let mut held = Composition::new(None);
    held.insert(
        "master",
        "crop(sin(2*pi*440*t), 0s, 0.05s) + 0.35*self(t - 0.25s)\n",
    );
    let silent = held
        .render(
            None,
            Some(8000),
            JsValue::from("silent"),
            None,
            None,
            Some(30.0),
        )
        .unwrap_or_else(|_| unreachable!("the echo falls silent"));
    let secs = silent.duration_secs();
    assert!((3.75..=3.8).contains(&secs), "{secs}");

    let never = held.render(
        Some("sin(2*pi*100*t)".to_string()),
        Some(8000),
        JsValue::from("silent"),
        None,
        Some(16),
        None,
    );
    let refused = never
        .err()
        .unwrap_or_else(|| unreachable!("a held sine never falls silent"));
    assert!(
        as_text(&field(&refused, "refusal")).contains("engine.never_silent"),
        "{}",
        as_text(&refused)
    );
}
