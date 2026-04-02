//! `eruditio` is a Rust crate providing full support for various ebook file types.
//!
//! The goal of this crate is to offer safe, fast, idiomatic, memory-efficient,
//! and production-ready parsing and generation of ebook formats such as EPUB, MOBI,
//! PDF, FB2, and others.

pub mod domain;
pub mod error;
pub mod formats;
pub mod parser;
pub mod pipeline;
pub mod transforms;

pub use domain::{
    Book, Chapter, Format, FormatReader, FormatWriter, Guide, GuideReference, GuideType, Manifest,
    ManifestData, ManifestItem, Metadata, PageProgression, ResourceView, Spine, SpineItem, TocItem,
    Transform,
};
pub use error::{EruditioError, Result};
pub use parser::EruditioParser;
