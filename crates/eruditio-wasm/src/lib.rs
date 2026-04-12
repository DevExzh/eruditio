use eruditio::{ConversionOptions, Format, Metadata, Pipeline};
use serde::Serialize;
use std::io::Cursor;
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

#[wasm_bindgen]
pub fn convert(
    input: &[u8],
    input_format: &str,
    output_format: &str,
    options: JsValue,
    progress: &js_sys::Function,
) -> Result<Vec<u8>, JsError> {
    let in_fmt = Format::from_extension(input_format)
        .ok_or_else(|| JsError::new(&format!("Unsupported input format: {input_format}")))?;
    let out_fmt = Format::from_extension(output_format)
        .ok_or_else(|| JsError::new(&format!("Unsupported output format: {output_format}")))?;

    let opts = parse_options(options)?;
    let pipeline = Pipeline::new();

    report_progress(progress, "Reading...")?;
    let book = pipeline
        .read(in_fmt, &mut Cursor::new(input), &opts)
        .map_err(|e| JsError::new(&e.to_string()))?;

    report_progress(progress, "Transforming...")?;
    let book = pipeline
        .apply_transforms_standalone(book, &opts)
        .map_err(|e| JsError::new(&e.to_string()))?;

    report_progress(progress, "Writing...")?;
    let mut output = Vec::new();
    pipeline
        .write(out_fmt, &book, &mut output)
        .map_err(|e| JsError::new(&e.to_string()))?;

    report_progress(progress, "Done")?;
    Ok(output)
}

fn report_progress(progress: &js_sys::Function, stage: &str) -> Result<(), JsError> {
    progress
        .call1(&JsValue::NULL, &JsValue::from_str(stage))
        .map_err(|e| JsError::new(&format!("Progress callback failed: {e:?}")))?;
    Ok(())
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct JsConversionOptions {
    title: Option<String>,
    authors: Option<String>,
    publisher: Option<String>,
    language: Option<String>,
    isbn: Option<String>,
    description: Option<String>,
    series: Option<String>,
    series_index: Option<f64>,
    tags: Option<String>,
    rights: Option<String>,
}

fn parse_options(options: JsValue) -> Result<ConversionOptions, JsError> {
    let js_opts: JsConversionOptions = if options.is_undefined() || options.is_null() {
        JsConversionOptions::default()
    } else {
        serde_wasm_bindgen::from_value(options)
            .map_err(|e| JsError::new(&format!("Invalid options: {e}")))?
    };

    let mut opts = ConversionOptions::all();

    let has_overrides = js_opts.title.is_some()
        || js_opts.authors.is_some()
        || js_opts.publisher.is_some()
        || js_opts.language.is_some()
        || js_opts.isbn.is_some()
        || js_opts.description.is_some()
        || js_opts.series.is_some()
        || js_opts.series_index.is_some()
        || js_opts.rights.is_some();

    if has_overrides {
        let mut meta = Metadata::default();
        meta.title = js_opts.title;
        if let Some(authors) = js_opts.authors {
            meta.authors = authors.split(',').map(|s| s.trim().to_string()).collect();
        }
        meta.publisher = js_opts.publisher;
        meta.language = js_opts.language;
        meta.isbn = js_opts.isbn;
        meta.description = js_opts.description;
        meta.series = js_opts.series;
        meta.series_index = js_opts.series_index;
        meta.rights = js_opts.rights;
        if let Some(tags) = js_opts.tags {
            meta.subjects = tags.split(',').map(|s| s.trim().to_string()).collect();
        }
        opts = opts.with_metadata(meta);
    }

    Ok(opts)
}
