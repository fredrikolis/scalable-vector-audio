// Concern: drives a persistent composition against a real origin-private file system | Non-concern: the records inside a pack (sva-engine's pack tests) | IO: (a dedicated worker) -> assertions
#![cfg(target_arch = "wasm32")]

//! Only a dedicated worker can hold a sync access handle, so this suite runs in one, in a
//! browser: `wasm-pack test --headless --chrome crates/sva-wasm --test opfs`. Node skips it.

use sva_wasm::Composition;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::{
    FileSystemDirectoryHandle, FileSystemGetDirectoryOptions, FileSystemGetFileOptions,
    StorageManager, WorkerGlobalScope,
};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

fn field(value: &JsValue, name: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(name))
        .unwrap_or_else(|_| unreachable!("{name} is readable"))
}

/// The origin's file system outlives one test, so every test caches under a name of its own.
fn fresh(what: &str) -> String {
    format!("{what}-{}", (js_sys::Math::random() * 1e15) as u64)
}

async fn opened(name: &str) -> Composition {
    let mut held = Composition::open_persistent(None, name.to_string(), None)
        .await
        .unwrap_or_else(|_| unreachable!("a name of one component opens"));
    held.insert("osc", "saw(110*t) + 0.5*square(165*t)\n");
    held.insert("master", "lowpass(sample(@osc), cutoff=900, q=0.8)*0.5\n");
    held
}

fn hits(held: &Composition, tier: &str) -> f64 {
    let stats = held
        .render(None, Some(8000), JsValue::from(0.25), None, None, None)
        .unwrap_or_else(|_| unreachable!("`master` renders"))
        .stats()
        .unwrap_or_else(|_| unreachable!("stats answer"));
    field(&field(&stats, "hits"), tier)
        .as_f64()
        .unwrap_or_else(|| unreachable!("a count"))
}

async fn cache_dir(name: &str) -> FileSystemDirectoryHandle {
    let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
    let storage: StorageManager = field(&scope.navigator(), "storage").unchecked_into();
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    let mut dir: FileSystemDirectoryHandle = JsFuture::from(storage.get_directory())
        .await
        .unwrap_or_else(|_| unreachable!("a worker reaches its file system"))
        .unchecked_into();
    for part in ["sva-cache", name] {
        dir = JsFuture::from(dir.get_directory_handle_with_options(part, &options))
            .await
            .unwrap_or_else(|_| unreachable!("{part} opens"))
            .unchecked_into();
    }
    dir
}

async fn entries(dir: &FileSystemDirectoryHandle) -> Vec<String> {
    let names = dir.keys();
    let mut found = Vec::new();
    loop {
        let step = JsFuture::from(names.next().unwrap_or_else(|_| unreachable!("iterable")))
            .await
            .unwrap_or_else(|_| unreachable!("a listing"));
        if field(&step, "done").is_truthy() {
            return found;
        }
        found.extend(field(&step, "value").as_string());
    }
}

#[wasm_bindgen_test]
async fn a_reopened_name_answers_a_render_from_the_pack() {
    let name = fresh("reopen");
    let first = opened(&name).await;
    assert!(first.is_persistent(), "a dedicated worker holds a pack");
    assert_eq!(
        hits(&first, "persistent"),
        0.0,
        "a fresh pack answers nothing"
    );
    assert!(first.persistent_bytes() > 0.0, "the render was written");
    drop(first);

    let again = opened(&name).await;
    assert!(
        again.is_persistent(),
        "the dropped composition let the file go"
    );
    assert!(
        hits(&again, "persistent") > 0.0,
        "the same render reads the pack"
    );
}

#[wasm_bindgen_test]
async fn opening_a_name_retires_every_other_pack_beside_its_own() {
    let name = fresh("retire");
    let dir = cache_dir(&name).await;
    let options = FileSystemGetFileOptions::new();
    options.set_create(true);
    JsFuture::from(dir.get_file_handle_with_options("pack-v0-stale.bin", &options))
        .await
        .unwrap_or_else(|_| unreachable!("a stale pack is written"));

    let held = opened(&name).await;
    assert!(held.is_persistent());
    let left = entries(&dir).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(
        left[0].starts_with("pack-") && left[0] != "pack-v0-stale.bin",
        "only the open pack is left: {left:?}"
    );
}

#[wasm_bindgen_test]
async fn a_name_already_held_opens_memory_only() {
    let name = fresh("held");
    let first = opened(&name).await;
    assert!(first.is_persistent());
    let second = opened(&name).await;
    assert!(!second.is_persistent(), "one pack, one holder");
    assert_eq!(second.persistent_bytes(), 0.0);
    assert!(first.is_persistent(), "the holder keeps it");
}
