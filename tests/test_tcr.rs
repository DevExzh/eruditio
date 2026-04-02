use eruditio::EruditioParser;
use std::io::Cursor;

#[test]
fn test_tcr_parsing() {
    let mut data = b"!!8-Bit!!".to_vec();

    // Add dictionary entries
    // 0: "H"
    data.push(1);
    data.push(b'H');
    // 1: "e"
    data.push(1);
    data.push(b'e');
    // 2: "l"
    data.push(1);
    data.push(b'l');
    // 3: "o"
    data.push(1);
    data.push(b'o');
    // 4: " "
    data.push(1);
    data.push(b' ');
    // 5: "W"
    data.push(1);
    data.push(b'W');
    // 6: "r"
    data.push(1);
    data.push(b'r');
    // 7: "d"
    data.push(1);
    data.push(b'd');

    // Fill remaining 248 entries with empty strings
    for _ in 8..256 {
        data.push(0);
    }

    // Encoded text: "Hello World"
    data.extend_from_slice(&[0, 1, 2, 2, 3, 4, 5, 3, 6, 2, 7]);

    let mut cursor = Cursor::new(data);
    let book = EruditioParser::parse(&mut cursor, Some("tcr")).expect("Failed to parse TCR");

    assert_eq!(
        book.metadata.title,
        Some("Unknown TCR Document".to_string())
    );

    let chapters = book.chapters();
    assert_eq!(chapters.len(), 1);

    let ch = &chapters[0];
    assert!(ch.content.contains("<p>Hello World</p>"));
}
