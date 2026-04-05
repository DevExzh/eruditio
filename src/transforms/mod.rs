//! Book transforms for the conversion pipeline.
//!
//! Each transform takes an immutable `&Book` and returns a new `Book`
//! with modifications applied. Transforms are composable and order-independent
//! where possible.

pub mod cover_handler;
pub mod data_uri_extractor;
pub mod html_normalizer;
pub mod manifest_trimmer;
pub mod metadata_merger;
pub mod structure_detector;
pub mod toc_generator;

pub use cover_handler::CoverHandler;
pub use data_uri_extractor::DataUriExtractor;
pub use html_normalizer::HtmlNormalizer;
pub use manifest_trimmer::ManifestTrimmer;
pub use metadata_merger::MetadataMerger;
pub use structure_detector::StructureDetector;
pub use toc_generator::TocGenerator;
