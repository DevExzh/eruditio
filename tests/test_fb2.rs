use eruditio::EruditioParser;
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
    assert_eq!(book.metadata.authors, vec!["Arthur Conan Doyle".to_string()]);

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
