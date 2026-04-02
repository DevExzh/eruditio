use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use crate::formats::cb7::Cb7Reader;
use crate::formats::cbr::CbrReader;
use crate::formats::cbz::CbzReader;
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// CBC (Comic Book Collection) format reader.
///
/// A CBC file is a ZIP archive containing a `comics.txt` manifest and one or more
/// inner comic archives (CBZ, CBR, or CB7). Each line in `comics.txt` has the format
/// `filename:title`.
#[derive(Default)]
pub struct CbcReader;

impl CbcReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for CbcReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(EruditioError::Io)?;
        let cursor = Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| EruditioError::Format(format!("Failed to open CBC as ZIP: {}", e)))?;

        let manifest = read_manifest(&mut archive)?;

        if manifest.is_empty() {
            return Err(EruditioError::Format(
                "CBC comics.txt is empty or missing entries".into(),
            ));
        }

        let mut book = Book::new();
        let mut page_offset: usize = 0;

        for (filename, title) in &manifest {
            let mut entry_data = Vec::new();
            {
                let mut entry = archive.by_name(filename).map_err(|e| {
                    EruditioError::Format(format!(
                        "CBC missing inner archive '{}': {}",
                        filename, e
                    ))
                })?;
                entry
                    .read_to_end(&mut entry_data)
                    .map_err(EruditioError::Io)?;
            }

            let inner_book = read_inner_comic(&entry_data, filename)?;

            // Merge inner book's resources and chapters with offset
            for resource in &inner_book.resources() {
                let prefixed_id = format!("{}_{}", filename, resource.id);
                book.add_resource(
                    &prefixed_id,
                    resource.href,
                    resource.data.to_vec(),
                    resource.media_type,
                );
            }

            for (i, chapter) in inner_book.chapters().iter().enumerate() {
                let chapter_id = format!("{}_{:04}", filename, i);
                let chapter_title = if i == 0 {
                    Some(title.clone())
                } else {
                    chapter.title.clone()
                };

                // Rewrite img src references to use prefixed resource IDs
                let content = rewrite_img_refs(&chapter.content, filename);

                book.add_chapter(&Chapter {
                    title: chapter_title,
                    content,
                    id: Some(chapter_id),
                });
            }

            page_offset += inner_book.chapters().len();
        }

        book.metadata.title = Some("Comic Book Collection".into());

        if page_offset == 0 {
            return Err(EruditioError::Format(
                "CBC collection contains no pages".into(),
            ));
        }

        Ok(book)
    }
}

fn read_manifest(archive: &mut ZipArchive<Cursor<Vec<u8>>>) -> Result<Vec<(String, String)>> {
    let mut manifest_data = Vec::new();
    {
        let mut manifest_file = archive.by_name("comics.txt").map_err(|_| {
            EruditioError::Format("CBC archive missing comics.txt manifest".into())
        })?;
        manifest_file
            .read_to_end(&mut manifest_data)
            .map_err(EruditioError::Io)?;
    }

    // Handle UTF-16 BOM or UTF-8
    let text = decode_manifest_text(&manifest_data);

    let entries: Vec<(String, String)> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            if let Some((filename, title)) = line.split_once(':') {
                (filename.trim().to_string(), title.trim().to_string())
            } else {
                let filename = line.trim().to_string();
                let title = filename.clone();
                (filename, title)
            }
        })
        .collect();

    Ok(entries)
}

fn decode_manifest_text(data: &[u8]) -> String {
    // Check for UTF-16 LE BOM
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        let u16_iter = data[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]));
        char::decode_utf16(u16_iter)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    }
    // Check for UTF-16 BE BOM
    else if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        let u16_iter = data[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]));
        char::decode_utf16(u16_iter)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}

fn read_inner_comic(data: &[u8], filename: &str) -> Result<Book> {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    let mut cursor = Cursor::new(data.to_vec());

    match ext.as_str() {
        "cbz" => CbzReader::new().read_book(&mut cursor),
        "cbr" => CbrReader::new().read_book(&mut cursor),
        "cb7" => Cb7Reader::new().read_book(&mut cursor),
        _ => {
            // Try to detect by magic bytes
            if data.len() >= 6 {
                if data.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
                    return CbzReader::new().read_book(&mut cursor);
                }
                if data.starts_with(b"Rar!\x1a\x07") {
                    return CbrReader::new().read_book(&mut cursor);
                }
                if data.starts_with(b"7z\xbc\xaf\x27\x1c") {
                    return Cb7Reader::new().read_book(&mut cursor);
                }
            }
            Err(EruditioError::Format(format!(
                "Unknown inner comic format: {}",
                filename
            )))
        }
    }
}

fn rewrite_img_refs(content: &str, prefix: &str) -> String {
    // Rewrite src="page_XXXX" to src="filename_page_XXXX"
    content.replace("src=\"page_", &format!("src=\"{}_page_", prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_utf8_manifest() {
        let data = b"comic1.cbz:First Comic\ncomic2.cbr:Second Comic\n";
        let result = decode_manifest_text(data);
        assert_eq!(result, "comic1.cbz:First Comic\ncomic2.cbr:Second Comic\n");
    }

    #[test]
    fn decode_utf16le_manifest() {
        let mut data = vec![0xFF, 0xFE]; // BOM
        for c in "test.cbz:Title\n".encode_utf16() {
            data.extend_from_slice(&c.to_le_bytes());
        }
        let result = decode_manifest_text(&data);
        assert_eq!(result, "test.cbz:Title\n");
    }

    #[test]
    fn parse_manifest_entries() {
        let data = b"comic1.cbz:First Comic\ncomic2.cbr:Second Comic\n";
        // Test decode + parse logic directly (read_manifest requires a real ZIP)
        let text = decode_manifest_text(data);
        let entries: Vec<(String, String)> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                if let Some((filename, title)) = line.split_once(':') {
                    (filename.trim().to_string(), title.trim().to_string())
                } else {
                    let filename = line.trim().to_string();
                    let title = filename.clone();
                    (filename, title)
                }
            })
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("comic1.cbz".into(), "First Comic".into()));
        assert_eq!(entries[1], ("comic2.cbr".into(), "Second Comic".into()));
    }

    #[test]
    fn rejects_non_zip_data() {
        let data = b"not a zip archive";
        let mut cursor = Cursor::new(data.as_slice());
        let reader = CbcReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }
}
