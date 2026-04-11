//! Conversion pipeline options.

use crate::domain::Metadata;
use crate::pipeline::load_filter::LoadFilter;

/// Options controlling the conversion pipeline behavior.
#[derive(Debug, Clone)]
#[must_use]
pub struct ConversionOptions {
    /// Metadata overrides to apply (non-None fields replace book metadata).
    pub metadata_overrides: Option<Metadata>,

    /// Whether to detect chapter structure from heading tags.
    pub detect_structure: bool,

    /// Whether to generate/rebuild the table of contents.
    pub generate_toc: bool,

    /// Whether to normalize HTML content to well-formed XHTML.
    pub normalize_html: bool,

    /// Whether to remove unreferenced resources from the manifest.
    pub trim_manifest: bool,

    /// Whether to detect and set the cover image.
    pub detect_cover: bool,

    /// Whether to extract data URI images into manifest resources.
    pub extract_data_uris: bool,

    /// Controls which categories of manifest resources are loaded by the reader.
    pub load_filter: LoadFilter,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            metadata_overrides: None,
            detect_structure: false,
            generate_toc: false,
            normalize_html: false,
            trim_manifest: false,
            detect_cover: false,
            extract_data_uris: false,
            load_filter: LoadFilter::ALL,
        }
    }
}

impl ConversionOptions {
    /// Returns options with all transforms enabled (recommended for most conversions).
    pub fn all() -> Self {
        Self {
            metadata_overrides: None,
            detect_structure: true,
            generate_toc: true,
            normalize_html: true,
            trim_manifest: true,
            detect_cover: true,
            extract_data_uris: true,
            load_filter: LoadFilter::ALL,
        }
    }

    /// Returns options with no transforms (pass-through conversion).
    pub fn none() -> Self {
        Self::default()
    }

    /// Sets metadata overrides.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata_overrides = Some(metadata);
        self
    }

    /// Enables HTML normalization.
    pub fn with_normalize_html(mut self) -> Self {
        self.normalize_html = true;
        self
    }

    /// Enables structure detection.
    pub fn with_detect_structure(mut self) -> Self {
        self.detect_structure = true;
        self
    }

    /// Enables TOC generation.
    pub fn with_generate_toc(mut self) -> Self {
        self.generate_toc = true;
        self
    }

    /// Sets the load filter controlling which resource categories the reader loads.
    pub fn with_load_filter(mut self, filter: LoadFilter) -> Self {
        self.load_filter = filter;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_pass_through() {
        let opts = ConversionOptions::default();
        assert!(!opts.detect_structure);
        assert!(!opts.generate_toc);
        assert!(!opts.normalize_html);
        assert!(!opts.trim_manifest);
        assert!(!opts.detect_cover);
        assert!(!opts.extract_data_uris);
        assert!(opts.metadata_overrides.is_none());
    }

    #[test]
    fn all_options_enabled() {
        let opts = ConversionOptions::all();
        assert!(opts.detect_structure);
        assert!(opts.generate_toc);
        assert!(opts.normalize_html);
        assert!(opts.trim_manifest);
        assert!(opts.detect_cover);
        assert!(opts.extract_data_uris);
    }
}
