use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use mime_guess::from_path;
use std::io::Read;
use unrar::Archive;

/// CBR (Comic Book RAR) format reader.
#[derive(Default)]
pub struct CbrReader;

impl CbrReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for CbrReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;

        // unrar requires a file path — write to a temp file
        let temp_path =
            std::env::temp_dir().join(format!("eruditio_cbr_{}.rar", std::process::id()));

        std::fs::write(&temp_path, &buffer)
            .map_err(|e| EruditioError::Format(format!("Failed to write temp RAR file: {}", e)))?;

        let result = extract_images_from_rar(&temp_path);

        // Always clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        let image_entries = result?;

        if image_entries.is_empty() {
            return Err(EruditioError::Format(
                "CBR archive contains no image files".into(),
            ));
        }

        let mut book = Book::new();

        for (index, (name, data)) in image_entries.iter().enumerate() {
            let media_type = from_path(name)
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            let resource_id = format!("page_{:04}", index);
            let chapter_id = format!("chapter_{:04}", index);

            book.add_resource(&resource_id, name, data.clone(), &media_type);

            book.add_chapter(Chapter {
                title: Some(format!("Page {}", index + 1)),
                content: format!("<img src=\"{}\" alt=\"Page {}\" />", resource_id, index + 1),
                id: Some(chapter_id),
            });
        }

        book.metadata.title = Some("Unknown Comic".into());

        Ok(book)
    }
}

fn extract_images_from_rar(path: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut image_entries: Vec<(String, Vec<u8>)> = Vec::new();

    let archive = Archive::new(path)
        .open_for_processing()
        .map_err(|e| EruditioError::Format(format!("Failed to open CBR as RAR: {}", e)))?;

    let mut cursor = archive;
    loop {
        match cursor.read_header() {
            Ok(Some(header)) => {
                let entry = header.entry();
                let filename = entry.filename.to_string_lossy().to_string();
                let is_image = entry.is_file() && {
                    let mime = from_path(&filename).first_or_octet_stream();
                    mime.type_() == "image"
                };

                if is_image {
                    let (data, next) = header.read().map_err(|e| {
                        EruditioError::Format(format!("Failed to read RAR entry: {}", e))
                    })?;
                    image_entries.push((filename, data));
                    cursor = next;
                } else {
                    cursor = header.skip().map_err(|e| {
                        EruditioError::Format(format!("Failed to skip RAR entry: {}", e))
                    })?;
                }
            },
            Ok(None) => break,
            Err(e) => {
                return Err(EruditioError::Format(format!(
                    "Failed to read RAR header: {}",
                    e
                )));
            },
        }
    }

    image_entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(image_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_non_rar_data() {
        let data = b"not a rar archive";
        let mut cursor = Cursor::new(data.as_slice());
        let reader = CbrReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_empty_input() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let reader = CbrReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }
}
