// Concern: opens one named pack file in the origin-private file system and serves it as a Medium | Non-concern: the records inside it (sva-engine's pack) | IO: (name) -> an OpfsMedium or nothing

use std::cell::RefCell;
use std::collections::HashMap;

use sva_engine::{Medium, PACK_FORMAT, RENDER_FINGERPRINT};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemReadWriteOptions, FileSystemSyncAccessHandle,
    StorageManager, WorkerGlobalScope,
};

/// A reloaded page's old worker can still hold the file for a moment after the new one asks.
const ATTEMPTS: u32 = 5;
const RETRY_MS: i32 = 100;

thread_local! {
    /// A JS handle is not `Sync`, so the medium holds only the name it is filed under here.
    static HANDLES: RefCell<HashMap<String, FileSystemSyncAccessHandle>> =
        RefCell::new(HashMap::new());
}

pub struct OpfsMedium {
    path: String,
}

impl OpfsMedium {
    fn with<T>(&self, f: impl FnOnce(&FileSystemSyncAccessHandle) -> Option<T>) -> Option<T> {
        HANDLES.with(|held| held.borrow().get(&self.path).and_then(f))
    }
}

fn at(off: u64) -> FileSystemReadWriteOptions {
    let options = FileSystemReadWriteOptions::new();
    options.set_at(off as f64);
    options
}

impl Medium for OpfsMedium {
    fn size(&self) -> u64 {
        self.with(|h| h.get_size().ok())
            .map_or(0, |size| size as u64)
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool {
        let wanted = buf.len();
        self.with(|h| h.read_with_u8_array_and_options(buf, &at(off)).ok())
            .is_some_and(|read| read as usize == wanted)
    }

    fn write_at(&self, off: u64, bytes: &[u8]) -> bool {
        self.with(|h| h.write_with_u8_array_and_options(bytes, &at(off)).ok())
            .is_some_and(|wrote| wrote as usize == bytes.len())
    }

    fn truncate(&self, len: u64) -> bool {
        self.with(|h| h.truncate_with_f64(len as f64).ok())
            .is_some()
    }

    fn flush(&self) -> bool {
        self.with(|h| h.flush().ok()).is_some()
    }
}

impl Drop for OpfsMedium {
    fn drop(&mut self) {
        if let Some(handle) = HANDLES.with(|held| held.borrow_mut().remove(&self.path)) {
            handle.close();
        }
    }
}

/// `sva-cache/<name>/pack-v<format>-<render fingerprint>.bin`, every other pack beside it deleted. `None`
/// wherever this is not a dedicated worker with a file system, or the file stays locked.
pub async fn open(name: &str) -> Option<OpfsMedium> {
    let scope = js_sys::global().dyn_into::<WorkerGlobalScope>().ok()?;
    let storage = js_sys::Reflect::get(&scope.navigator(), &"storage".into()).ok()?;
    if storage.is_undefined() {
        return None;
    }
    let root = resolved(storage.unchecked_into::<StorageManager>().get_directory()).await?;
    let dir = subdirectory(&subdirectory(&root, "sva-cache").await?, name).await?;
    let file = format!("pack-v{PACK_FORMAT}-{RENDER_FINGERPRINT:016x}.bin");
    retire_other_packs(&dir, &file).await;

    let options = FileSystemGetFileOptions::new();
    options.set_create(true);
    let handle: FileSystemFileHandle =
        resolved(dir.get_file_handle_with_options(&file, &options)).await?;
    for attempt in 1..=ATTEMPTS {
        match JsFuture::from(handle.create_sync_access_handle()).await {
            Ok(access) => {
                let path = format!("{name}/{file}");
                HANDLES.with(|held| {
                    held.borrow_mut()
                        .insert(path.clone(), access.unchecked_into())
                });
                return Some(OpfsMedium { path });
            }
            Err(e) if attempt < ATTEMPTS && named(&e) == "NoModificationAllowedError" => {
                pause(&scope).await;
            }
            Err(_) => return None,
        }
    }
    None
}

async fn resolved<T: JsCast>(promise: js_sys::Promise) -> Option<T> {
    JsFuture::from(promise).await.ok()?.dyn_into().ok()
}

async fn subdirectory(
    parent: &FileSystemDirectoryHandle,
    name: &str,
) -> Option<FileSystemDirectoryHandle> {
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    resolved(parent.get_directory_handle_with_options(name, &options)).await
}

/// Only an older version's packs: one another tab still holds is left for its next open.
async fn retire_other_packs(dir: &FileSystemDirectoryHandle, keep: &str) {
    let names = dir.keys();
    let mut stale = Vec::new();
    while let Ok(next) = names.next() {
        let Ok(step) = JsFuture::from(next).await else {
            break;
        };
        if js_sys::Reflect::get(&step, &"done".into()).is_ok_and(|done| done.is_truthy()) {
            break;
        }
        if let Some(entry) = js_sys::Reflect::get(&step, &"value".into())
            .ok()
            .and_then(|v| v.as_string())
            .filter(|entry| entry.starts_with("pack-") && entry != keep)
        {
            stale.push(entry);
        }
    }
    for entry in stale {
        let _ = JsFuture::from(dir.remove_entry(&entry)).await;
    }
}

fn named(error: &JsValue) -> String {
    js_sys::Reflect::get(error, &"name".into())
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default()
}

async fn pause(scope: &WorkerGlobalScope) {
    let slept = js_sys::Promise::new(&mut |resolve, _| {
        let _ = scope.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, RETRY_MS);
    });
    let _ = JsFuture::from(slept).await;
}
