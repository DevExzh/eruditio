use eruditio::EruditioParser;
use std::io::Cursor;

const TXT_DATA: &str = r#"
First line.

Second paragraph after blank line.

Third paragraph after blank line.
Line inside third paragraph.

"#;

#[test]
fn test_txt_parsing() {
    let mut cursor = Cursor::new(TXT_DATA);
    let book = EruditioParser::parse(&mut cursor, Some("txt")).expect("Failed to parse TXT");

    assert_eq!(book.metadata.title, Some("Unknown TXT Document".to_string()));

    let chapters = book.chapters();
    assert_eq!(chapters.len(), 1);

    let ch = &chapters[0];
    assert_eq!(ch.title, Some("Main Content".to_string()));
    assert!(ch.content.contains("<p>First line.</p>"));
    assert!(ch.content.contains("<p>Second paragraph after blank line.</p>"));
    assert!(ch.content.contains("<p>Line inside third paragraph.</p>"));
}
