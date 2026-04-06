//! Conversion pipeline: reads a book in one format, applies transforms, writes in another.

use crate::domain::Book;
use crate::domain::format::Format;
use crate::error::{EruditioError, Result};
use crate::transforms::{
    CoverHandler, DataUriExtractor, HtmlNormalizer, ManifestTrimmer, MetadataMerger,
    StructureDetector, TocGenerator,
};
use std::io::{Read, Write};

use super::options::ConversionOptions;
use super::registry::FormatRegistry;
use crate::domain::traits::Transform;

/// The conversion pipeline: reads, transforms, and writes ebooks.
#[must_use]
pub struct Pipeline {
    registry: FormatRegistry,
}

impl Pipeline {
    /// Creates a new pipeline with the default format registry.
    pub fn new() -> Self {
        Self {
            registry: FormatRegistry::new(),
        }
    }

    /// Creates a pipeline with a custom registry.
    pub fn with_registry(registry: FormatRegistry) -> Self {
        Self { registry }
    }

    /// Converts an ebook from one format to another.
    ///
    /// Reads from `input`, applies configured transforms, and writes to `output`.
    pub fn convert(
        &self,
        input_format: Format,
        output_format: Format,
        input: &mut dyn Read,
        output: &mut dyn Write,
        options: &ConversionOptions,
    ) -> Result<Book> {
        // Read.
        let reader = self
            .registry
            .reader(&input_format)
            .ok_or_else(|| EruditioError::Unsupported(format!("No reader for {}", input_format)))?;

        let book = reader.read_book(input)?;

        // Transform (takes ownership, avoids cloning).
        let book = self.apply_transforms(book, options)?;

        // Write.
        let writer = self.registry.writer(&output_format).ok_or_else(|| {
            EruditioError::Unsupported(format!("No writer for {}", output_format))
        })?;

        writer.write_book(&book, output)?;

        Ok(book)
    }

    /// Reads a book without writing (useful for inspection/metadata extraction).
    pub fn read(
        &self,
        format: Format,
        input: &mut dyn Read,
        options: &ConversionOptions,
    ) -> Result<Book> {
        let reader = self
            .registry
            .reader(&format)
            .ok_or_else(|| EruditioError::Unsupported(format!("No reader for {}", format)))?;

        let book = reader.read_book(input)?;
        self.apply_transforms(book, options)
    }

    /// Writes a book without reading (useful when you already have a Book).
    pub fn write(&self, format: Format, book: &Book, output: &mut dyn Write) -> Result<()> {
        let writer = self
            .registry
            .writer(&format)
            .ok_or_else(|| EruditioError::Unsupported(format!("No writer for {}", format)))?;

        writer.write_book(book, output)
    }

    /// Returns a reference to the format registry.
    pub fn registry(&self) -> &FormatRegistry {
        &self.registry
    }

    /// Applies the configured transforms to a book (takes ownership to avoid cloning).
    fn apply_transforms(&self, book: Book, options: &ConversionOptions) -> Result<Book> {
        let transforms = self.build_transform_chain(options);

        let mut current = book;
        for transform in &transforms {
            current = transform.apply(current)?;
        }

        Ok(current)
    }

    /// Builds the ordered transform chain based on options.
    fn build_transform_chain(&self, options: &ConversionOptions) -> Vec<Box<dyn Transform>> {
        let mut chain: Vec<Box<dyn Transform>> = Vec::new();

        // Order matters: extract data URIs first (simplifies HTML, makes images
        // available as manifest resources), then normalize, detect structure,
        // and generate TOC.

        if options.extract_data_uris {
            chain.push(Box::new(DataUriExtractor));
        }

        if options.normalize_html {
            chain.push(Box::new(HtmlNormalizer));
        }

        if let Some(ref overrides) = options.metadata_overrides {
            chain.push(Box::new(MetadataMerger::new(overrides.clone())));
        }

        if options.detect_structure {
            chain.push(Box::new(StructureDetector));
        }

        if options.generate_toc {
            chain.push(Box::new(TocGenerator));
        }

        if options.detect_cover {
            chain.push(Box::new(CoverHandler));
        }

        if options.trim_manifest {
            chain.push(Box::new(ManifestTrimmer));
        }

        chain
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn pipeline_pass_through_conversion() {
        let pipeline = Pipeline::new();

        let mut book = Book::new();
        book.metadata.title = Some("Pipeline Test".into());
        book.add_chapter(&Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Hello from the pipeline</p>".into(),
            id: Some("ch1".into()),
        });

        // Write as EPUB.
        let mut epub_buf = Vec::new();
        pipeline.write(Format::Epub, &book, &mut epub_buf).unwrap();

        // Read back as EPUB with no transforms.
        let mut cursor = std::io::Cursor::new(epub_buf);
        let decoded = pipeline
            .read(Format::Epub, &mut cursor, &ConversionOptions::none())
            .unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Pipeline Test"));
    }

    #[test]
    fn pipeline_cross_format_fb2_to_epub() {
        let pipeline = Pipeline::new();

        let mut book = Book::new();
        book.metadata.title = Some("FB2 to EPUB".into());
        book.metadata.authors.push("Author".into());
        book.add_chapter(&Chapter {
            title: Some("Section".into()),
            content: "<p>Cross-format content</p>".into(),
            id: Some("s1".into()),
        });

        // Write as FB2.
        let mut fb2_buf = Vec::new();
        pipeline.write(Format::Fb2, &book, &mut fb2_buf).unwrap();

        // Convert FB2 -> EPUB.
        let mut fb2_cursor = std::io::Cursor::new(fb2_buf);
        let mut epub_buf = Vec::new();
        let result = pipeline
            .convert(
                Format::Fb2,
                Format::Epub,
                &mut fb2_cursor,
                &mut epub_buf,
                &ConversionOptions::all(),
            )
            .unwrap();

        assert_eq!(result.metadata.title.as_deref(), Some("FB2 to EPUB"));

        // Verify the EPUB is readable.
        let mut epub_cursor = std::io::Cursor::new(epub_buf);
        let decoded = pipeline
            .read(Format::Epub, &mut epub_cursor, &ConversionOptions::none())
            .unwrap();
        assert_eq!(decoded.metadata.title.as_deref(), Some("FB2 to EPUB"));
    }

    #[test]
    fn pipeline_with_metadata_override() {
        let pipeline = Pipeline::new();

        let mut book = Book::new();
        book.metadata.title = Some("Original".into());
        book.add_chapter(&Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let overrides = crate::domain::Metadata {
            title: Some("Overridden Title".into()),
            ..Default::default()
        };

        let options = ConversionOptions::none().with_metadata(overrides);

        // Write as TXT.
        let mut txt_buf = Vec::new();
        pipeline.write(Format::Txt, &book, &mut txt_buf).unwrap();

        // Read and transform.
        let mut cursor = std::io::Cursor::new(txt_buf);
        let result = pipeline.read(Format::Txt, &mut cursor, &options).unwrap();

        assert_eq!(result.metadata.title.as_deref(), Some("Overridden Title"));
    }

    #[test]
    fn pipeline_unsupported_format_returns_error() {
        let pipeline = Pipeline::new();
        let mut data = std::io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();

        let result = pipeline.convert(
            Format::Djvu,
            Format::Epub,
            &mut data,
            &mut output,
            &ConversionOptions::none(),
        );

        assert!(result.is_err());
    }
}
