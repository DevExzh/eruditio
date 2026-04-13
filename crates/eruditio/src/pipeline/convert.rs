//! Conversion pipeline: reads a book in one format, applies transforms, writes in another.

use crate::domain::Book;
use crate::domain::format::Format;
use crate::domain::load_filter::LoadFilter;
use crate::error::{EruditioError, Result};
use crate::transforms::{
    CoverHandler, DataUriExtractor, HtmlNormalizer, ManifestTrimmer, MetadataMerger,
    StructureDetector, TocGenerator,
};
use std::io::{Read, Write};

use super::options::ConversionOptions;
use super::registry::FormatRegistry;
use crate::domain::traits::Transform;

/// Enum dispatch for transforms — avoids `Box<dyn Transform>` vtable overhead
/// and heap allocation per conversion.  All variants except `MetadataMerger`
/// are zero-sized, so the enum is small and cache-friendly.
enum TransformKind {
    DataUriExtractor(DataUriExtractor),
    HtmlNormalizer(HtmlNormalizer),
    MetadataMerger(MetadataMerger),
    StructureDetector(StructureDetector),
    TocGenerator(TocGenerator),
    CoverHandler(CoverHandler),
    ManifestTrimmer(ManifestTrimmer),
}

impl TransformKind {
    fn apply(&self, book: Book) -> Result<Book> {
        match self {
            Self::DataUriExtractor(t) => Transform::apply(t, book),
            Self::HtmlNormalizer(t) => Transform::apply(t, book),
            Self::MetadataMerger(t) => Transform::apply(t, book),
            Self::StructureDetector(t) => Transform::apply(t, book),
            Self::TocGenerator(t) => Transform::apply(t, book),
            Self::CoverHandler(t) => Transform::apply(t, book),
            Self::ManifestTrimmer(t) => Transform::apply(t, book),
        }
    }
}

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

        // Read with output-format-aware filtering.
        let filter = LoadFilter::for_output_format(output_format);
        let book = reader.read_book_filtered(input, filter)?;

        // Transform (takes ownership, avoids cloning).
        let book = self.apply_transforms(book, options, Some(input_format), Some(output_format))?;

        // Write.
        let writer = self.registry.writer(&output_format).ok_or_else(|| {
            EruditioError::Unsupported(format!("No writer for {}", output_format))
        })?;

        writer.write_book(&book, output)?;

        Ok(book)
    }

    /// Reads a book without writing (useful for inspection/metadata extraction).
    ///
    /// All manifest resources are loaded unconditionally because there is no
    /// output format from which to derive a [`LoadFilter`].
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
        self.apply_transforms(book, options, Some(format), None)
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

    /// Applies transforms to a book without format context.
    ///
    /// Use this when you have already read the book and want to apply
    /// transforms before writing. Since no input/output format is known,
    /// all enabled transforms run unconditionally (text-only and
    /// container-source optimizations are skipped).
    pub fn apply_transforms_standalone(
        &self,
        book: Book,
        options: &ConversionOptions,
    ) -> Result<Book> {
        self.apply_transforms(book, options, None, None)
    }

    /// Applies the configured transforms to a book (takes ownership to avoid cloning).
    fn apply_transforms(
        &self,
        book: Book,
        options: &ConversionOptions,
        input_format: Option<Format>,
        output_format: Option<Format>,
    ) -> Result<Book> {
        let transforms = self.build_transform_chain(options, input_format, output_format);

        let mut current = book;
        for transform in &transforms {
            current = transform.apply(current)?;
        }

        Ok(current)
    }

    /// Builds the ordered transform chain based on options.
    ///
    /// When `output_format` is a plain-text format (TXT, MD, PML), transforms
    /// that only benefit structured/rich output are skipped — they scan HTML
    /// without affecting the final text, wasting cycles.
    ///
    /// When `input_format` is a non-container source (HTML, FB2, TXT, etc.),
    /// ManifestTrimmer is skipped because these readers only produce referenced
    /// resources — there are no orphan manifest items to trim.
    fn build_transform_chain(
        &self,
        options: &ConversionOptions,
        input_format: Option<Format>,
        output_format: Option<Format>,
    ) -> Vec<TransformKind> {
        let text_only = matches!(
            output_format,
            Some(
                Format::Txt | Format::Txtz | Format::Tcr | Format::Md | Format::Pml | Format::Pmlz
            )
        );

        // Source formats that bundle resources in a container (ZIP, etc.) may
        // have orphan manifest items that need trimming.  Non-container formats
        // (HTML, FB2, TXT, …) only produce referenced resources.
        let source_has_container = matches!(
            input_format,
            Some(
                Format::Epub
                    | Format::Kepub
                    | Format::Oeb
                    | Format::Htmlz
                    | Format::Cbz
                    | Format::Cb7
                    | Format::Cbr
                    | Format::Cbc
                    | Format::Mobi
                    | Format::Azw
                    | Format::Azw3
                    | Format::Prc
                    | Format::Fbz
                    | Format::Pmlz
                    | Format::Txtz
                    | Format::Lit
            )
        );
        let mut chain = Vec::new();

        // Order matters: extract data URIs first (simplifies HTML, makes images
        // available as manifest resources), then normalize, detect structure,
        // and generate TOC.

        if options.extract_data_uris && !text_only {
            chain.push(TransformKind::DataUriExtractor(DataUriExtractor));
        }

        if options.normalize_html && !text_only {
            chain.push(TransformKind::HtmlNormalizer(HtmlNormalizer));
        }

        if let Some(ref overrides) = options.metadata_overrides {
            chain.push(TransformKind::MetadataMerger(MetadataMerger::new(
                overrides.clone(),
            )));
        }

        if options.detect_structure && !text_only {
            chain.push(TransformKind::StructureDetector(StructureDetector));
        }

        if options.generate_toc && !text_only {
            chain.push(TransformKind::TocGenerator(TocGenerator));
        }

        if options.detect_cover && !text_only {
            chain.push(TransformKind::CoverHandler(CoverHandler));
        }

        if options.trim_manifest && !text_only && source_has_container {
            chain.push(TransformKind::ManifestTrimmer(ManifestTrimmer));
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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
        book.add_chapter(Chapter {
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

    #[test]
    fn apply_transforms_public_api() {
        let pipeline = Pipeline::new();
        let mut book = Book::new();
        book.metadata.title = Some("Original".into());
        book.add_chapter(Chapter {
            title: None,
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let overrides = crate::domain::Metadata {
            title: Some("Transformed".into()),
            ..Default::default()
        };
        let opts = ConversionOptions::none().with_metadata(overrides);
        let result = pipeline.apply_transforms_standalone(book, &opts).unwrap();
        assert_eq!(result.metadata.title.as_deref(), Some("Transformed"));
    }
}
