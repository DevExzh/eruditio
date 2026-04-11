//! Conversion pipeline options.

use crate::domain::Metadata;

/// Options controlling the conversion pipeline behavior.
#[derive(Debug, Clone, Default)]
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
