// Concern: named bytes in an origin-private file system directory a page hands over | Non-concern: what the bytes mean, the budget (sva-engine) | IO: (name[, bytes]) -> bytes, names, or why not

use js_sys::{Promise, Uint8Array};
use sva_engine::Backend;
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

    #[wasm_bindgen(method, js_name = removeEntry)]
    fn remove_entry(this: &DirectoryHandle, name: &str) -> Promise;

    #[wasm_bindgen(method)]
    fn keys(this: &DirectoryHandle) -> Names;

    type Names;

    #[wasm_bindgen(method)]
    fn next(this: &Names) -> Promise;

    type FileHandle;

    #[wasm_bindgen(method, js_name = getFile)]
    fn get_file(this: &FileHandle) -> Promise;

    #[wasm_bindgen(method, js_name = createWritable)]
    fn create_writable(this: &FileHandle) -> Promise;

    type File;

    #[wasm_bindgen(method, js_name = arrayBuffer)]
    fn array_buffer(this: &File) -> Promise;

    #[wasm_bindgen(method, getter)]
    fn size(this: &File) -> f64;

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

fn missing(e: &JsValue) -> bool {
    field(e, "name").as_string().as_deref() == Some("NotFoundError")
}

async fn settled<T: JsCast>(promise: Promise) -> Result<T, JsValue> {
    Ok(JsFuture::from(promise).await?.unchecked_into())
}

impl Opfs {
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
    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(file) = self.file(name).await? else {
            return Ok(None);
        };
        let buffer: JsValue = settled(file.array_buffer()).await.map_err(why)?;
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

    async fn delete(&self, name: &str) -> Result<(), String> {
        match settled::<JsValue>(self.dir.remove_entry(name)).await {
            Err(e) if !missing(&e) => Err(why(e)),
            _ => Ok(()),
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
}
