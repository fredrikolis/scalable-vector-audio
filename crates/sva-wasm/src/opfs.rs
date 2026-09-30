// Concern: named bytes in an origin-private file system directory a page hands over | Non-concern: what the bytes mean, the budget (sva-engine) | IO: (name[, bytes]) -> bytes, names, or why not

use js_sys::{Promise, Uint8Array};
use sva_engine::Backend;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    /// What `navigator.storage.getDirectory()` answers, or a directory inside it.
    #[wasm_bindgen(typescript_type = "FileSystemDirectoryHandle")]
    pub type DirectoryHandle;

    #[wasm_bindgen(method, js_name = getFileHandle)]
    fn get_file_handle(this: &DirectoryHandle, name: &str, options: &JsValue) -> Promise;

    #[wasm_bindgen(method, js_name = getDirectoryHandle)]
    fn get_directory_handle(this: &DirectoryHandle, name: &str, options: &JsValue) -> Promise;

    #[wasm_bindgen(method, js_name = removeEntry)]
    fn remove_entry(this: &DirectoryHandle, name: &str) -> Promise;

    #[wasm_bindgen(method, js_name = removeEntry)]
    fn remove_tree(this: &DirectoryHandle, name: &str, options: &JsValue) -> Promise;

    #[wasm_bindgen(method)]
    fn keys(this: &DirectoryHandle) -> Names;

    #[wasm_bindgen(method, getter)]
    fn name(this: &DirectoryHandle) -> String;

    type Names;

    #[wasm_bindgen(method)]
    fn next(this: &Names) -> Promise;

    type FileHandle;

    #[wasm_bindgen(method, js_name = getFile)]
    fn get_file(this: &FileHandle) -> Promise;

    #[wasm_bindgen(method, js_name = createWritable)]
    fn create_writable(this: &FileHandle) -> Promise;

    #[wasm_bindgen(method, js_name = move)]
    fn move_into(this: &FileHandle, dir: &DirectoryHandle, name: &str) -> Promise;

    type File;

    #[wasm_bindgen(method, js_name = arrayBuffer)]
    fn array_buffer(this: &File) -> Promise;

    #[wasm_bindgen(method, getter)]
    fn size(this: &File) -> f64;

    #[wasm_bindgen(method)]
    fn slice(this: &File, start: f64, end: f64) -> File;

    /// `navigator.locks`: a page holds one lock per staging area for as long as it lives, and
    /// the directory's own lock while it changes the directory's names.
    type LockManager;

    #[wasm_bindgen(method)]
    fn request(this: &LockManager, name: &str, granted: &JsValue) -> Promise;

    #[wasm_bindgen(method)]
    fn query(this: &LockManager) -> Promise;

    type Writable;

    #[wasm_bindgen(method)]
    fn write(this: &Writable, data: &Uint8Array) -> Promise;

    #[wasm_bindgen(method)]
    fn close(this: &Writable) -> Promise;
}

pub struct Opfs {
    pub dir: DirectoryHandle,
}

fn field(of: &JsValue, name: &str) -> JsValue {
    js_sys::Reflect::get(of, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn why(e: JsValue) -> String {
    e.as_string()
        .or_else(|| field(&e, "message").as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

fn named(e: &JsValue) -> Option<String> {
    field(e, "name").as_string()
}

fn missing(e: &JsValue) -> bool {
    named(e).as_deref() == Some("NotFoundError")
}

/// What OPFS rejects a removal or a move with while another holder has the file open.
fn busy(e: &JsValue) -> bool {
    matches!(
        named(e).as_deref(),
        Some("NoModificationAllowedError" | "InvalidModificationError")
    )
}

async fn settled<T: JsCast>(promise: Promise) -> Result<T, JsValue> {
    Ok(JsFuture::from(promise).await?.unchecked_into())
}

const STAGING: &str = "staging-";

fn locks() -> Result<LockManager, String> {
    let navigator = js_sys::Reflect::get(&js_sys::global(), &"navigator".into()).map_err(why)?;
    let locks = field(&navigator, "locks");
    match locks.is_undefined() {
        true => Err("this page has no navigator.locks".to_string()),
        false => Ok(locks.unchecked_into()),
    }
}

fn lock_of(staging: &str) -> String {
    format!("sva-store-{staging}")
}

/// Web Locks keep a lock while its callback's promise is unsettled: the lock is `name`'s until
/// the function this answers is called.
async fn granted(name: &str) -> Result<js_sys::Function, String> {
    let locks = locks()?;
    let granted = Promise::new(&mut |resolve, _| {
        let held = Closure::once_into_js(move || {
            let mut release = JsValue::UNDEFINED;
            let kept = Promise::new(&mut |done, _| release = done.into());
            let _ = resolve.call1(&JsValue::NULL, &release);
            kept
        });
        let _ = locks.request(name, &held);
    });
    settled(granted).await.map_err(why)
}

/// `name` locked until the page goes.
async fn hold(name: &str) -> Result<(), String> {
    granted(name).await.map(drop)
}

/// A directory's lock, released when dropped.
pub struct Held(js_sys::Function);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.call0(&JsValue::NULL);
    }
}

async fn held() -> Result<Vec<String>, String> {
    let state: JsValue = settled(locks()?.query()).await.map_err(why)?;
    let held = js_sys::Array::from(&field(&state, "held"));
    Ok(held
        .iter()
        .filter_map(|lock| field(&lock, "name").as_string())
        .collect())
}

impl Opfs {
    /// Every staging area whose lock no page holds, removed.
    async fn swept(&self) -> Result<(), String> {
        let held = held().await?;
        let names = self.dir.keys();
        let mut stale = Vec::new();
        loop {
            let step: JsValue = settled(names.next()).await.map_err(why)?;
            if field(&step, "done").is_truthy() {
                break;
            }
            let Some(name) = field(&step, "value").as_string() else {
                continue;
            };
            if name.starts_with(STAGING) && !held.contains(&lock_of(&name)) {
                stale.push(name);
            }
        }
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"recursive".into(), &true.into()).map_err(why)?;
        for name in stale {
            match settled::<JsValue>(self.dir.remove_tree(&name, &options)).await {
                Err(e) if !missing(&e) && !busy(&e) => return Err(why(e)),
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle(&self, name: &str, create: bool) -> Result<Option<FileHandle>, String> {
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"create".into(), &create.into()).map_err(why)?;
        match settled(self.dir.get_file_handle(name, &options)).await {
            Ok(handle) => Ok(Some(handle)),
            Err(e) if missing(&e) => Ok(None),
            Err(e) => Err(why(e)),
        }
    }

    async fn file(&self, name: &str) -> Result<Option<File>, String> {
        let Some(handle) = self.handle(name, false).await? else {
            return Ok(None);
        };
        settled(handle.get_file()).await.map(Some).map_err(why)
    }
}

impl Backend for Opfs {
    type Lock = Held;

    /// Named after the directory, as a handle names nothing wider: two directories of one name
    /// share it, which only makes each wait on the other's changes.
    async fn lock(&self) -> Result<Held, String> {
        granted(&format!("sva-store-dir-{}", self.dir.name()))
            .await
            .map(Held)
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(file) = self.file(name).await? else {
            return Ok(None);
        };
        let buffer: JsValue = settled(file.array_buffer()).await.map_err(why)?;
        Ok(Some(Uint8Array::new(&buffer).to_vec()))
    }

    async fn get_range(&self, name: &str, from: u64, len: u64) -> Result<Option<Vec<u8>>, String> {
        let Some(file) = self.file(name).await? else {
            return Ok(None);
        };
        let part = file.slice(from as f64, from.saturating_add(len) as f64);
        let buffer: JsValue = settled(part.array_buffer()).await.map_err(why)?;
        Ok(Some(Uint8Array::new(&buffer).to_vec()))
    }

    /// A writable stream commits on `close` alone, so a reader never sees half of one.
    async fn put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let handle = self
            .handle(name, true)
            .await?
            .ok_or_else(|| format!("`{name}` could not be created"))?;
        let writable: Writable = settled(handle.create_writable()).await.map_err(why)?;
        settled::<JsValue>(writable.write(&Uint8Array::from(bytes)))
            .await
            .map_err(why)?;
        settled::<JsValue>(writable.close()).await.map_err(why)?;
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<bool, String> {
        match settled::<JsValue>(self.dir.remove_entry(name)).await {
            Err(e) if busy(&e) => Ok(false),
            Err(e) if !missing(&e) => Err(why(e)),
            _ => Ok(true),
        }
    }

    async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        let names = self.dir.keys();
        let mut out = Vec::new();
        loop {
            let step: JsValue = settled(names.next()).await.map_err(why)?;
            if field(&step, "done").is_truthy() {
                return Ok(out);
            }
            let Some(name) = field(&step, "value").as_string() else {
                continue;
            };
            if let Ok(Some(file)) = self.file(&name).await {
                out.push((name, file.size() as u64));
            }
        }
    }

    /// A directory of its own per page, locked before it exists, so a sweep never removes one
    /// a live page uses.
    async fn staging(&self) -> Result<Opfs, String> {
        let name = format!(
            "{STAGING}{:016x}",
            (js_sys::Math::random() * 2f64.powi(53)) as u64
        );
        hold(&lock_of(&name)).await?;
        self.swept().await?;
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"create".into(), &true.into()).map_err(why)?;
        let dir = settled(self.dir.get_directory_handle(&name, &options)).await;
        Ok(Opfs {
            dir: dir.map_err(why)?,
        })
    }

    async fn rename(&self, name: &str, to: &Opfs) -> Result<bool, String> {
        let handle = self
            .handle(name, false)
            .await?
            .ok_or_else(|| format!("`{name}` is not staged"))?;
        match settled::<JsValue>(handle.move_into(&to.dir, name)).await {
            Err(e) if busy(&e) => Ok(false),
            moved => moved.map(|_| true).map_err(why),
        }
    }
}
