use eruditio::EruditioParser;
use eruditio::formats::fb2::Fb2Writer;
use eruditio::{Book, Chapter, FormatWriter};
use std::io::Cursor;

const FB2_DATA: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
  <description>
    <title-info>
      <author>
        <first-name>Arthur</first-name>
        <last-name>Conan Doyle</last-name>
      </author>
      <book-title>Sherlock Holmes</book-title>
      <lang>en</lang>
    </title-info>
  </description>
  <body>
    <title>
      <p>Sherlock Holmes</p>
    </title>
    <section>
      <title>
        <p>Chapter 1</p>
      </title>
      <p>This is the first chapter.</p>
    </section>
  </body>
  <binary id="cover.jpg" content-type="image/jpeg">iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=</binary>
</FictionBook>"#;

#[test]
fn test_fb2_parsing() {
    let mut cursor = Cursor::new(FB2_DATA);
    let book = EruditioParser::parse(&mut cursor, Some("fb2")).expect("Failed to parse FB2");

    assert_eq!(book.metadata.title, Some("Sherlock Holmes".to_string()));
    assert_eq!(book.metadata.language, Some("en".to_string()));
    assert_eq!(
        book.metadata.authors,
        vec!["Arthur Conan Doyle".to_string()]
    );

    let chapters = book.chapters();
    assert_eq!(chapters.len(), 1);
    let ch1 = &chapters[0];
    assert_eq!(ch1.title, Some("Chapter 1".to_string()));
    assert!(ch1.content.contains("<p>This is the first chapter.</p>"));

    // Verify binary resource
    let resources = book.resources();
    assert_eq!(resources.len(), 1);

    let cover = book.manifest.get("cover.jpg").unwrap();
    assert_eq!(cover.media_type, "image/jpeg");
    let cover_data = book.resource_data("cover.jpg").unwrap();
    assert!(!cover_data.is_empty());
}

#[test]
fn fb2_writer_closes_emphasis_at_paragraph_boundary() {
    let mut book = Book::new();
    book.metadata.title = Some("Emphasis Close Test".into());
    // Simulate emphasis that spans across a paragraph boundary:
    // the </em> comes in the second <p>, so the writer must auto-close and reopen.
    book.add_chapter(&Chapter {
        title: Some("Ch1".into()),
        content: r#"<p>Normal text <em>emphasized text</p><p>still emphasized</em> normal again</p>"#.into(),
        id: Some("ch1".into()),
    });

    let mut output = Vec::new();
    Fb2Writer::new().write_book(&book, &mut output).unwrap();
    let xml = String::from_utf8(output).unwrap();

    // The emphasis must be closed before the paragraph closes, and reopened in the next
    assert!(
        xml.contains("<p>Normal text <emphasis>emphasized text</emphasis></p>"),
        "emphasis should be closed before </p>, got:\n{}",
        xml
    );
    assert!(
        xml.contains("<p><emphasis>still emphasized</emphasis> normal again</p>"),
        "emphasis should be reopened in next paragraph and closed when </em> is hit, got:\n{}",
        xml
    );
    assert!(!xml.contains("<emphasis></emphasis>"), "spurious empty emphasis tags found:\n{xml}");
}

#[test]
fn fb2_writer_closes_strong_at_paragraph_boundary() {
    let mut book = Book::new();
    book.metadata.title = Some("Strong Close Test".into());
    // Simulate bold/strong that spans across a paragraph boundary
    book.add_chapter(&Chapter {
        title: Some("Ch1".into()),
        content: r#"<p>Normal text <b>bold text</p><p>still bold</b> normal again</p>"#.into(),
        id: Some("ch1".into()),
    });

    let mut output = Vec::new();
    Fb2Writer::new().write_book(&book, &mut output).unwrap();
    let xml = String::from_utf8(output).unwrap();

    // The strong tag must be closed before the paragraph closes, and reopened in the next
    assert!(
        xml.contains("<p>Normal text <strong>bold text</strong></p>"),
        "strong should be closed before </p>, got:\n{}",
        xml
    );
    assert!(
        xml.contains("<p><strong>still bold</strong> normal again</p>"),
        "strong should be reopened in next paragraph and closed when </b> is hit, got:\n{}",
        xml
    );
    assert!(!xml.contains("<strong></strong>"), "spurious empty strong tags found:\n{xml}");
}

#[test]
fn fb2_writer_handles_nested_emphasis_strong_across_paragraphs() {
    let mut book = Book::new();
    book.metadata.title = Some("Nested Formatting Test".into());
    // Both emphasis AND strong span across a paragraph boundary simultaneously
    book.add_chapter(&Chapter {
        title: Some("Ch1".into()),
        content: r#"<p>Normal <em><b>bold italic text</p><p>still bold italic</b></em> normal</p>"#.into(),
        id: Some("ch1".into()),
    });

    let mut output = Vec::new();
    Fb2Writer::new().write_book(&book, &mut output).unwrap();
    let xml = String::from_utf8(output).unwrap();

    // Both emphasis and strong must be closed before paragraph boundary, then reopened
    assert!(
        xml.contains("<p>Normal <emphasis><strong>bold italic text</strong></emphasis></p>"),
        "both emphasis and strong should be closed before </p>, got:\n{}",
        xml
    );
    assert!(
        xml.contains("<p><emphasis><strong>still bold italic</strong></emphasis> normal</p>"),
        "both should be reopened in next paragraph, got:\n{}",
        xml
    );
    assert!(!xml.contains("<emphasis></emphasis>"), "spurious empty emphasis tags found:\n{xml}");
    assert!(!xml.contains("<strong></strong>"), "spurious empty strong tags found:\n{xml}");
}

#[test]
fn fb2_writer_emphasis_spanning_multiple_paragraphs() {
    let mut book = Book::new();
    book.metadata.title = Some("Multi-Para Emphasis Test".into());
    book.add_chapter(&Chapter {
        title: Some("Ch1".into()),
        content: "<p>A <em>B</p><p>C</p><p>D</em> E</p>".into(),
        id: Some("ch1".into()),
    });

    let mut output = Vec::new();
    Fb2Writer::new().write_book(&book, &mut output).unwrap();
    let xml = String::from_utf8(output).unwrap();

    // Should have emphasis properly closed/reopened across all paragraphs
    assert!(xml.contains("<emphasis>B</emphasis>"), "first para should have emphasis, got:\n{xml}");
    assert!(xml.contains("<emphasis>C</emphasis>"), "middle para should have emphasis, got:\n{xml}");
    assert!(xml.contains("<emphasis>D</emphasis>"), "last para should have emphasis before close, got:\n{xml}");
    // No spurious empty paragraphs
    assert!(!xml.contains("<emphasis></emphasis>"), "spurious empty emphasis found:\n{xml}");
}
