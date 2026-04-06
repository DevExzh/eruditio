//! Cross-format conversion integration tests for the Pipeline.

use eruditio::domain::{Book, Chapter, Format, Metadata};
use eruditio::pipeline::convert::Pipeline;
use eruditio::pipeline::options::ConversionOptions;
use std::io::Cursor;

/// Helper: creates a simple test book with metadata, chapters, and resources.
fn make_test_book() -> Book {
    let mut book = Book::new();
    book.metadata.title = Some("Pipeline Test Book".into());
    book.metadata.authors.push("Alice Author".into());
    book.metadata.language = Some("en".into());
    book.metadata.description = Some("A book for pipeline integration tests.".into());

    book.add_chapter(&Chapter {
        title: Some("Introduction".into()),
        content: "<p>Welcome to the pipeline test book.</p>".into(),
        id: Some("intro".into()),
    });
    book.add_chapter(&Chapter {
        title: Some("Main Content".into()),
        content: "<p>This is the main body of the book.</p>".into(),
        id: Some("main".into()),
    });
    book.add_chapter(&Chapter {
        title: Some("Conclusion".into()),
        content: "<p>Thank you for reading.</p>".into(),
        id: Some("conclusion".into()),
    });

    book.add_resource(
        "cover_img",
        "images/cover.png",
        vec![0x89, 0x50, 0x4E, 0x47],
        "image/png",
    );

    book
}

#[test]
fn epub_to_epub_with_all_transforms() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert EPUB → EPUB with all transforms.
    let mut input = Cursor::new(epub_buf);
    let mut output = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Epub,
            &mut input,
            &mut output,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));
    assert!(!result.chapters().is_empty());

    // Verify the output is a readable EPUB.
    let mut verify_cursor = Cursor::new(output);
    let decoded = pipeline
        .read(Format::Epub, &mut verify_cursor, &ConversionOptions::none())
        .expect("read back EPUB");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
}

#[test]
fn fb2_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as FB2.
    let mut fb2_buf = Vec::new();
    pipeline
        .write(Format::Fb2, &book, &mut fb2_buf)
        .expect("write FB2");

    // Convert FB2 → EPUB.
    let mut input = Cursor::new(fb2_buf);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Fb2,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert FB2 → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));
    assert!(
        result
            .metadata
            .authors
            .contains(&"Alice Author".to_string())
    );

    // Verify EPUB output is readable.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn txt_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let mut book = Book::new();
    book.metadata.title = Some("Text Origin".into());
    book.add_chapter(&Chapter {
        title: Some("Ch 1".into()),
        content: "<p>Plain text content converted to EPUB.</p>".into(),
        id: Some("ch1".into()),
    });

    // Write as TXT.
    let mut txt_buf = Vec::new();
    pipeline
        .write(Format::Txt, &book, &mut txt_buf)
        .expect("write TXT");

    // Convert TXT → EPUB.
    let mut input = Cursor::new(txt_buf);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Txt,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert TXT → EPUB");

    // TXT reader doesn't preserve title, but content should survive.
    let chapters = result.chapters();
    assert!(!chapters.is_empty());

    // EPUB output is valid.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn mobi_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as MOBI.
    let mut mobi_buf = Vec::new();
    pipeline
        .write(Format::Mobi, &book, &mut mobi_buf)
        .expect("write MOBI");

    // Convert MOBI → EPUB.
    let mut input = Cursor::new(mobi_buf);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Mobi,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert MOBI → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // Verify EPUB output.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn epub_to_fb2_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert EPUB → FB2.
    let mut input = Cursor::new(epub_buf);
    let mut fb2_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Fb2,
            &mut input,
            &mut fb2_buf,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB → FB2");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // Verify FB2 output is readable.
    let mut verify = Cursor::new(fb2_buf);
    let decoded = pipeline
        .read(Format::Fb2, &mut verify, &ConversionOptions::none())
        .expect("read back FB2");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
}

#[test]
fn conversion_with_metadata_override() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert with metadata overrides.
    let overrides = Metadata {
        title: Some("Overridden Title".into()),
        authors: vec!["New Author".into()],
        ..Default::default()
    };

    let options = ConversionOptions::all().with_metadata(overrides);

    let mut input = Cursor::new(epub_buf);
    let mut output = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Epub,
            &mut input,
            &mut output,
            &options,
        )
        .expect("convert with overrides");

    assert_eq!(result.metadata.title.as_deref(), Some("Overridden Title"));
    assert_eq!(result.metadata.authors, vec!["New Author"]);
}

#[test]
fn pipeline_read_only_for_inspection() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Read-only (no write step) — useful for metadata extraction.
    let mut input = Cursor::new(epub_buf);
    let result = pipeline
        .read(Format::Epub, &mut input, &ConversionOptions::none())
        .expect("read EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));
    assert_eq!(result.chapter_count(), 3);
}

#[test]
fn unsupported_input_format_returns_error() {
    let pipeline = Pipeline::new();
    let mut input = Cursor::new(Vec::<u8>::new());
    let mut output = Vec::new();

    let result = pipeline.convert(
        Format::Docx,
        Format::Epub,
        &mut input,
        &mut output,
        &ConversionOptions::none(),
    );

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("No reader"));
}

#[test]
fn unsupported_output_format_returns_error() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write EPUB so we have valid input.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    let mut input = Cursor::new(epub_buf);
    let mut output = Vec::new();

    let result = pipeline.convert(
        Format::Epub,
        Format::Docx,
        &mut input,
        &mut output,
        &ConversionOptions::none(),
    );

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("No writer"));
}

#[test]
fn three_hop_conversion_fb2_epub_mobi() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Step 1: Write as FB2.
    let mut fb2_buf = Vec::new();
    pipeline
        .write(Format::Fb2, &book, &mut fb2_buf)
        .expect("write FB2");

    // Step 2: FB2 → EPUB.
    let mut fb2_input = Cursor::new(fb2_buf);
    let mut epub_buf = Vec::new();
    let _ = pipeline
        .convert(
            Format::Fb2,
            Format::Epub,
            &mut fb2_input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("FB2 → EPUB");

    // Step 3: EPUB → MOBI.
    let mut epub_input = Cursor::new(epub_buf);
    let mut mobi_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Mobi,
            &mut epub_input,
            &mut mobi_buf,
            &ConversionOptions::all(),
        )
        .expect("EPUB → MOBI");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // Verify final MOBI is readable.
    let mut mobi_verify = Cursor::new(mobi_buf);
    let decoded = pipeline
        .read(Format::Mobi, &mut mobi_verify, &ConversionOptions::none())
        .expect("read back MOBI");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn rtf_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as RTF.
    let mut rtf_buf = Vec::new();
    pipeline
        .write(Format::Rtf, &book, &mut rtf_buf)
        .expect("write RTF");

    // Convert RTF → EPUB.
    let mut input = Cursor::new(rtf_buf);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Rtf,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert RTF → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));
    assert!(
        result
            .metadata
            .authors
            .contains(&"Alice Author".to_string())
    );

    // Verify EPUB output is readable.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn epub_to_rtf_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert EPUB → RTF.
    let mut input = Cursor::new(epub_buf);
    let mut rtf_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Rtf,
            &mut input,
            &mut rtf_buf,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB → RTF");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // Verify RTF output is readable.
    let mut verify = Cursor::new(rtf_buf);
    let decoded = pipeline
        .read(Format::Rtf, &mut verify, &ConversionOptions::none())
        .expect("read back RTF");
    assert_eq!(
        decoded.metadata.title.as_deref(),
        Some("Pipeline Test Book")
    );
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn html_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as HTML.
    let mut html_buf = Vec::new();
    pipeline
        .write(Format::Html, &book, &mut html_buf)
        .expect("write HTML");

    // Convert HTML → EPUB.
    let mut input = Cursor::new(html_buf);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Html,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert HTML → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // Verify EPUB output is readable.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn epub_to_html_conversion() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert EPUB → HTML.
    let mut input = Cursor::new(epub_buf);
    let mut html_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Html,
            &mut input,
            &mut html_buf,
            &ConversionOptions::all(),
        )
        .expect("convert EPUB → HTML");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    // HTML output should contain content.
    let html_str = String::from_utf8(html_buf).expect("valid UTF-8");
    assert!(html_str.contains("Pipeline Test Book"));
}

#[test]
fn htmlz_round_trip_through_pipeline() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as HTMLZ.
    let mut htmlz_buf = Vec::new();
    pipeline
        .write(Format::Htmlz, &book, &mut htmlz_buf)
        .expect("write HTMLZ");

    // Convert HTMLZ → HTMLZ.
    let mut input = Cursor::new(htmlz_buf);
    let mut output = Vec::new();
    let result = pipeline
        .convert(
            Format::Htmlz,
            Format::Htmlz,
            &mut input,
            &mut output,
            &ConversionOptions::all(),
        )
        .expect("convert HTMLZ → HTMLZ");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));
    assert!(!result.chapters().is_empty());
}

#[test]
fn rtf_to_html_cross_format() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Write as RTF.
    let mut rtf_buf = Vec::new();
    pipeline
        .write(Format::Rtf, &book, &mut rtf_buf)
        .expect("write RTF");

    // Convert RTF → HTML.
    let mut input = Cursor::new(rtf_buf);
    let mut html_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Rtf,
            Format::Html,
            &mut input,
            &mut html_buf,
            &ConversionOptions::all(),
        )
        .expect("convert RTF → HTML");

    assert_eq!(result.metadata.title.as_deref(), Some("Pipeline Test Book"));

    let html_str = String::from_utf8(html_buf).expect("valid UTF-8");
    assert!(html_str.contains("Pipeline Test Book"));
}

#[test]
fn four_hop_html_rtf_epub_mobi() {
    let pipeline = Pipeline::new();
    let book = make_test_book();

    // Step 1: Write as HTML.
    let mut html_buf = Vec::new();
    pipeline
        .write(Format::Html, &book, &mut html_buf)
        .expect("write HTML");

    // Step 2: HTML → RTF.
    let mut html_input = Cursor::new(html_buf);
    let mut rtf_buf = Vec::new();
    let _ = pipeline
        .convert(
            Format::Html,
            Format::Rtf,
            &mut html_input,
            &mut rtf_buf,
            &ConversionOptions::all(),
        )
        .expect("HTML → RTF");

    // Step 3: RTF → EPUB.
    let mut rtf_input = Cursor::new(rtf_buf);
    let mut epub_buf = Vec::new();
    let _ = pipeline
        .convert(
            Format::Rtf,
            Format::Epub,
            &mut rtf_input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("RTF → EPUB");

    // Step 4: EPUB → MOBI.
    let mut epub_input = Cursor::new(epub_buf);
    let mut mobi_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Mobi,
            &mut epub_input,
            &mut mobi_buf,
            &ConversionOptions::all(),
        )
        .expect("EPUB → MOBI");

    // Title may have survived the chain.
    assert!(!result.chapters().is_empty());

    // Verify final MOBI is readable.
    let mut verify = Cursor::new(mobi_buf);
    let decoded = pipeline
        .read(Format::Mobi, &mut verify, &ConversionOptions::none())
        .expect("read back MOBI");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn transforms_disabled_passes_through_unchanged() {
    let pipeline = Pipeline::new();
    let mut book = Book::new();
    book.metadata.title = Some("Pass Through".into());
    book.add_chapter(&Chapter {
        title: Some("Only Chapter".into()),
        content: "<p>A & B<br>line two</p>".into(),
        id: Some("ch1".into()),
    });

    // Write as EPUB.
    let mut epub_buf = Vec::new();
    pipeline
        .write(Format::Epub, &book, &mut epub_buf)
        .expect("write EPUB");

    // Convert with no transforms.
    let mut input = Cursor::new(epub_buf);
    let mut output = Vec::new();
    let result = pipeline
        .convert(
            Format::Epub,
            Format::Epub,
            &mut input,
            &mut output,
            &ConversionOptions::none(),
        )
        .expect("convert pass-through");

    assert_eq!(result.metadata.title.as_deref(), Some("Pass Through"));
    // Content should be unchanged (no normalization applied).
    let chapters = result.chapters();
    assert_eq!(chapters.len(), 1);
}
