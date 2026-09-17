// Concern: drives every JS export through real wasm | Non-concern: what a sample is worth (sva-engine), the pipeline (sva-cli) | IO: (a built composition) -> assertions
#![cfg(target_arch = "wasm32")]

//! The gap a native test cannot see: `wasm32-unknown-unknown` has no clock and no threads, and
//! reaching for either compiles clean and traps only at RUNTIME. Hence one session for the
//! whole surface. Run it with `wasm-pack test --node crates/sva-wasm`.

use sva_wasm::{Composition, Rendering};
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
    of.render(Some(target.to_string()), Some(8000), None)
        .unwrap_or_else(|_| unreachable!("`{target}` renders"))
}

fn of_default(held: &Composition) -> Rendering {
    held.render(None, Some(8000), None)
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
        "lines", "atoms", "samples", "ledger", "loudness", "envelope",
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
        .render(None, Some(8000), None)
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
