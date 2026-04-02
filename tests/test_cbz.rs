use eruditio::EruditioParser;
use std::io::{Cursor, Write};
use zip::write::FileOptions;
use zip::ZipWriter;

#[test]
fn test_cbz_parsing() {
    // Create an in-memory zip file representing a CBZ
    let mut buf = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut zip = ZipWriter::new(cursor);
        let options: FileOptions<'_, ()> =
            FileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add images in random order to test sorting
        zip.start_file("page_02.png", options).unwrap();
        zip.write_all(b"fake png data 2").unwrap();

        zip.start_file("page_01.jpg", options).unwrap();
        zip.write_all(b"fake jpg data 1").unwrap();

        zip.start_file("not_an_image.txt", options).unwrap();
        zip.write_all(b"some text").unwrap();

        zip.finish().unwrap();
    }

    // Parse the CBZ
    let mut cursor = Cursor::new(buf);
    let book = EruditioParser::parse(&mut cursor, Some("cbz")).expect("Failed to parse CBZ");

    assert_eq!(book.metadata.title, Some("Unknown Comic".to_string()));

    let chapters = book.chapters();
    assert_eq!(chapters.len(), 2);

    // Verify chapters (pages) are sorted correctly
    let ch1 = &chapters[0];
    assert_eq!(ch1.title, Some("Page 1".to_string()));
    assert_eq!(ch1.id, Some("chapter_0000".to_string()));
    assert!(ch1.content.contains("src=\"page_0000\""));

    let ch2 = &chapters[1];
    assert_eq!(ch2.title, Some("Page 2".to_string()));
    assert_eq!(ch2.id, Some("chapter_0001".to_string()));
    assert!(ch2.content.contains("src=\"page_0001\""));

    // Verify resources
    let resources = book.resources();
    assert_eq!(resources.len(), 2);

    let res1_data = book.resource_data("page_0000").unwrap();
    assert_eq!(res1_data, b"fake jpg data 1");
    let res1 = book.manifest.get("page_0000").unwrap();
    assert_eq!(res1.media_type, "image/jpeg");

    let res2_data = book.resource_data("page_0001").unwrap();
    assert_eq!(res2_data, b"fake png data 2");
    let res2 = book.manifest.get("page_0001").unwrap();
    assert_eq!(res2.media_type, "image/png");
}
