use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use mime_guess::from_path;
use sevenz_rust2::{ArchiveReader, Password};
use std::io::{Cursor, Read};

/// CB7 (Comic Book 7z) format reader.
#[derive(Default)]
pub struct Cb7Reader;

impl Cb7Reader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for Cb7Reader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(EruditioError::Io)?;
        let cursor = Cursor::new(buffer);

        let mut archive_reader = ArchiveReader::new(cursor, Password::empty())
            .map_err(|e| EruditioError::Format(format!("Failed to open CB7 as 7z: {}", e)))?;

        let mut image_entries: Vec<(String, Vec<u8>)> = Vec::new();

        archive_reader
            .for_each_entries(|entry, reader| {
                if !entry.is_directory() && entry.has_stream() {
                    let name = entry.name().to_string();
                    let mime = from_path(&name).first_or_octet_stream();
                    if mime.type_() == "image" {
                        let mut data = Vec::new();
                        reader.read_to_end(&mut data)?;
                        image_entries.push((name, data));
                    }
                }
                Ok(true)
            })
            .map_err(|e| EruditioError::Format(format!("Failed to read CB7 entries: {}", e)))?;

        if image_entries.is_empty() {
            return Err(EruditioError::Format(
                "CB7 archive contains no image files".into(),
            ));
        }

        image_entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut book = Book::new();

        for (index, (name, data)) in image_entries.into_iter().enumerate() {
            let media_type = from_path(&name)
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            let resource_id = format!("page_{:04}", index);
            let chapter_id = format!("chapter_{:04}", index);

            book.add_resource(&resource_id, name, data, &media_type);

            book.add_chapter(&Chapter {
                title: Some(format!("Page {}", index + 1)),
                content: format!("<img src=\"{}\" alt=\"Page {}\" />", resource_id, index + 1),
                id: Some(chapter_id),
            });
        }

        book.metadata.title = Some("Unknown Comic".into());

        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_7z_data() {
        let data = b"not a 7z archive";
        let mut cursor = Cursor::new(data.as_slice());
        let reader = Cb7Reader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_empty_input() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let reader = Cb7Reader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }
}
