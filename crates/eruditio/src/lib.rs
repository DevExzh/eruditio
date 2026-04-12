//! `eruditio` is a Rust crate providing full support for various ebook file types.
//!
//! The goal of this crate is to offer safe, fast, idiomatic, memory-efficient,
//! and production-ready parsing and generation of ebook formats such as EPUB, MOBI,
//! PDF, FB2, and others.

#[cfg(all(feature = "mimalloc", not(feature = "dhat-heap"), not(target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

pub mod domain;
pub mod error;
pub mod formats;
pub mod parser;
pub mod pipeline;
pub mod transforms;

pub use domain::{
    Book, Chapter, ChapterView, Format, FormatReader, FormatWriter, Guide, GuideReference,
    GuideType, LoadFilter, Manifest, ManifestData, ManifestItem, Metadata, PageProgression,
    ResourceView, Spine, SpineItem, TocItem, Transform,
};
pub use error::{EruditioError, Result};
pub use parser::EruditioParser;
pub use pipeline::convert::Pipeline;
pub use pipeline::options::ConversionOptions;
pub use pipeline::registry::FormatRegistry;
