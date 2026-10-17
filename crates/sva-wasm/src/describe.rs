// Concern: what a page reads without rendering: a parse tree, the readings | Non-concern: rendering (lib.rs) | IO: (text) -> an outline; () -> readings

use sva_engine::{READINGS, Reads};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{parse, thrown};

/// `sva-cli outline` of a node's body; spans count from the body.
#[wasm_bindgen]
pub fn outline(text: &str) -> Result<JsValue, JsValue> {
    let body = &text[sva_ast::body_at(text)..];
    parse(&sva_core::outline_data(body).map_err(|e| thrown(&e))?)
}

/// Each reading `render` answers: `name`, `takes`, `reads`, `help`.
#[wasm_bindgen]
pub fn readings() -> Result<js_sys::Array, JsValue> {
    let out = js_sys::Array::new();
    for reading in READINGS {
        let row = js_sys::Object::new();
        let takes: js_sys::Array = reading
            .takes
            .iter()
            .map(|arg| JsValue::from_str(arg.key()))
            .collect();
        let reads = match reading.reads {
            Reads::ClosedForm => "closed_form",
            Reads::Either => "either",
            Reads::Samples => "samples",
            Reads::Graph => "graph",
            Reads::Structure => "structure",
        };
        for (key, value) in [
            ("name", JsValue::from_str(reading.name)),
            ("takes", takes.into()),
            ("reads", JsValue::from_str(reads)),
            ("help", JsValue::from_str(reading.help)),
        ] {
            js_sys::Reflect::set(&row, &key.into(), &value)?;
        }
        out.push(&row);
    }
    Ok(out)
}
