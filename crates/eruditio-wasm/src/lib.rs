use eruditio::Pipeline;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[derive(Serialize)]
struct SupportedFormats {
    input: Vec<String>,
    output: Vec<String>,
}

#[wasm_bindgen]
pub fn supported_formats() -> JsValue {
    let pipeline = Pipeline::new();
    let registry = pipeline.registry();

    let mut input: Vec<String> = registry
        .readable_formats()
        .into_iter()
        .map(|f| f.extension().to_string())
        .collect();
    input.sort();

    let mut output: Vec<String> = registry
        .writable_formats()
        .into_iter()
        .map(|f| f.extension().to_string())
        .collect();
    output.sort();

    serde_wasm_bindgen::to_value(&SupportedFormats { input, output }).unwrap()
}
