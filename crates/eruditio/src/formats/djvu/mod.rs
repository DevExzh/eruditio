/// DjVu format reader — extracts text layers from DjVu files.
///
/// DjVu uses an IFF85-based chunk container. Text is stored in `TXTz`
/// (BZZ-compressed) and `TXTa` (uncompressed) chunks. Each text chunk
/// begins with a 3-byte big-endian length prefix.
pub mod bzz;

use crate::domain::{Book, Chapter, FormatReader};
use crate::error::{EruditioError, Result};
use bzz::bzz_decompress;
use std::io::Read;

/// A parsed IFF85 chunk from a DjVu file.
struct DjvuChunk {
    chunk_type: [u8; 4],
    #[allow(dead_code)]
    subtype: Option<[u8; 4]>,
    data_start: usize,
    data_end: usize,
    sub_chunks: Vec<DjvuChunk>,
}

impl DjvuChunk {
    fn parse(buf: &[u8], start: usize, end: usize) -> Result<Self> {
        if start + 8 > end {
            return Err(EruditioError::Format(
                "DJVU: chunk too small for header".into(),
            ));
        }

        let mut chunk_type = [0u8; 4];
        chunk_type.copy_from_slice(&buf[start..start + 4]);

        let size = u32::from_be_bytes([
            buf[start + 4],
            buf[start + 5],
            buf[start + 6],
            buf[start + 7],
        ]) as usize;

        let mut pos = start + 8;
        let data_end = start
            .checked_add(8)
            .and_then(|v| v.checked_add(size))
            .unwrap_or(end)
            .min(end);

        let mut subtype = None;
        if &chunk_type == b"FORM" {
            if pos + 4 > data_end {
                return Err(EruditioError::Format(
                    "DJVU: FORM chunk too small for subtype".into(),
                ));
            }
            let mut st = [0u8; 4];
            st.copy_from_slice(&buf[pos..pos + 4]);
            subtype = Some(st);
            pos += 4;
        }

        let mut sub_chunks = Vec::new();

        if &chunk_type == b"FORM" {
            while pos + 8 <= data_end {
                let child_start = pos;
                let child = DjvuChunk::parse(buf, pos, data_end)?;
                // The original chunk size = data_end - start - 8 (the header).
                // Align to 2-byte boundary based on the original size field.
                let child_size = child.data_end.saturating_sub(child_start).saturating_sub(8);
                let padding = if !child_size.is_multiple_of(2) { 1 } else { 0 };
                pos = child.data_end + padding;
                sub_chunks.push(child);
            }
        }

        Ok(DjvuChunk {
            chunk_type,
            subtype,
            data_start: if subtype.is_some() {
                start + 12
            } else {
                start + 8
            },
            data_end,
            sub_chunks,
        })
    }

    #[cfg(test)]
    fn type_str(&self) -> &str {
        std::str::from_utf8(&self.chunk_type).unwrap_or("????")
    }

    /// Recursively collect text from TXTz and TXTa chunks.
    fn collect_text(&self, buf: &[u8], texts: &mut Vec<String>) -> Result<()> {
        if &self.chunk_type == b"TXTz" {
            let compressed = &buf[self.data_start..self.data_end];
            let decompressed = bzz_decompress(compressed)?;
            if decompressed.len() >= 3 {
                let text_len = ((decompressed[0] as usize) << 16)
                    | ((decompressed[1] as usize) << 8)
                    | (decompressed[2] as usize);
                let text_end = (3 + text_len).min(decompressed.len());
                let text = String::from_utf8_lossy(&decompressed[3..text_end]);
                if !text.trim().is_empty() {
                    texts.push(text.into_owned());
                }
            }
        } else if &self.chunk_type == b"TXTa" {
            let data = &buf[self.data_start..self.data_end];
            if data.len() >= 3 {
                let text_len =
                    ((data[0] as usize) << 16) | ((data[1] as usize) << 8) | (data[2] as usize);
                let text_end = (3 + text_len).min(data.len());
                let text = String::from_utf8_lossy(&data[3..text_end]);
                if !text.trim().is_empty() {
                    texts.push(text.into_owned());
                }
            }
        }

        for child in &self.sub_chunks {
            child.collect_text(buf, texts)?;
        }

        Ok(())
    }
}

/// DjVu format reader.
#[derive(Default)]
pub struct DjvuReader;

impl DjvuReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for DjvuReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;

        // Validate AT&T magic
        if buffer.len() < 12 || &buffer[0..4] != b"AT&T" {
            return Err(EruditioError::Format(
                "Not a valid DJVU file (missing AT&T magic)".into(),
            ));
        }

        // Parse the root FORM chunk (starts after the 4-byte AT&T prefix)
        let root = DjvuChunk::parse(&buffer, 4, buffer.len())?;

        if &root.chunk_type != b"FORM" {
            return Err(EruditioError::Format(
                "DJVU: expected FORM chunk after AT&T magic".into(),
            ));
        }

        // Collect text from all TXTz and TXTa chunks
        let mut texts = Vec::new();
        root.collect_text(&buffer, &mut texts)?;

        if texts.is_empty() {
            return Err(EruditioError::Format(
                "DJVU file contains no text layer".into(),
            ));
        }

        let mut book = Book::new();

        // Each text block becomes a chapter
        for (i, text) in texts.iter().enumerate() {
            let paragraphs: Vec<String> = text
                .split('\n')
                .filter(|line| !line.trim().is_empty())
                .map(|line| format!("<p>{}</p>", line.trim()))
                .collect();
            let content = paragraphs.join("\n");

            book.add_chapter(Chapter {
                title: Some(format!("Page {}", i + 1)),
                content,
                id: Some(format!("page_{:04}", i)),
            });
        }

        book.metadata.title = Some("DjVu Document".into());

        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_non_djvu_data() {
        let data = b"not a djvu file at all";
        let mut cursor = Cursor::new(data.as_slice());
        let reader = DjvuReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("AT&T magic"), "got: {}", msg);
    }

    #[test]
    fn rejects_empty_input() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let reader = DjvuReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn parses_txta_chunk() {
        // Build a minimal DJVU with a TXTa chunk containing "Hello World"
        let text = b"Hello World";
        let text_len_bytes = [0u8, 0, text.len() as u8]; // 3-byte BE length
        let txta_data_len = 3 + text.len();

        let mut buf = Vec::new();
        // AT&T magic
        buf.extend_from_slice(b"AT&T");
        // FORM chunk
        buf.extend_from_slice(b"FORM");
        let form_size = 4 + 8 + txta_data_len; // subtype(4) + TXTa header(8) + TXTa data
        buf.extend_from_slice(&(form_size as u32).to_be_bytes());
        buf.extend_from_slice(b"DJVU"); // subtype
        // TXTa chunk
        buf.extend_from_slice(b"TXTa");
        buf.extend_from_slice(&(txta_data_len as u32).to_be_bytes());
        buf.extend_from_slice(&text_len_bytes);
        buf.extend_from_slice(text);

        let mut cursor = Cursor::new(buf);
        let reader = DjvuReader::new();
        let book = reader.read_book(&mut cursor).unwrap();

        assert_eq!(book.chapters().len(), 1);
        assert!(book.chapters()[0].content.contains("Hello World"));
    }

    #[test]
    fn rejects_djvu_with_no_text() {
        // Build a DJVU with only an INFO chunk (no text)
        let mut buf = Vec::new();
        buf.extend_from_slice(b"AT&T");
        buf.extend_from_slice(b"FORM");
        let form_size: u32 = 4 + 8 + 4; // subtype + INFO header + 4 bytes data
        buf.extend_from_slice(&form_size.to_be_bytes());
        buf.extend_from_slice(b"DJVU");
        buf.extend_from_slice(b"INFO");
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(&[0, 0, 0, 0]);

        let mut cursor = Cursor::new(buf);
        let reader = DjvuReader::new();
        let result = reader.read_book(&mut cursor);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("no text layer"), "got: {}", msg);
    }

    #[test]
    fn chunk_parsing_handles_alignment() {
        // FORM chunk with a 5-byte TXTa (odd size → 1 byte padding) + INFO chunk
        let text = b"Hi"; // 2 bytes of text
        let txta_data_len = 3 + 2; // 5 bytes (odd)

        let mut buf = Vec::new();
        buf.extend_from_slice(b"AT&T");
        buf.extend_from_slice(b"FORM");
        // form_size = subtype(4) + TXTa(8+5) + pad(1) + INFO(8+4) = 30
        let form_size: u32 = 4 + 8 + 5 + 1 + 8 + 4;
        buf.extend_from_slice(&form_size.to_be_bytes());
        buf.extend_from_slice(b"DJVU");
        // TXTa
        buf.extend_from_slice(b"TXTa");
        buf.extend_from_slice(&(txta_data_len as u32).to_be_bytes());
        buf.extend_from_slice(&[0u8, 0, 2]); // 3-byte BE length = 2
        buf.extend_from_slice(text);
        buf.push(0); // alignment padding
        // INFO
        buf.extend_from_slice(b"INFO");
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(&[0, 0, 0, 0]);

        let root = DjvuChunk::parse(&buf, 4, buf.len()).unwrap();
        assert_eq!(root.sub_chunks.len(), 2);
        assert_eq!(root.sub_chunks[0].type_str(), "TXTa");
        assert_eq!(root.sub_chunks[1].type_str(), "INFO");
    }
}
