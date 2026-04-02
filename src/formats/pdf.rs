use crate::domain::{Book, FormatReader, FormatWriter};
use crate::error::Result;
use std::io::{Read, Write};

/// PDF format reader.
#[derive(Default)]
pub struct PdfReader;

impl PdfReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for PdfReader {
    fn read_book(&self, _reader: &mut dyn Read) -> Result<Book> {
        Err(crate::error::EruditioError::Unsupported("PDF reading not yet implemented".into()))
    }
}

/// PDF format writer.
#[derive(Default)]
pub struct PdfWriter;

impl PdfWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for PdfWriter {
    fn write_book(&self, _book: &Book, _writer: &mut dyn Write) -> Result<()> {
        Err(crate::error::EruditioError::Unsupported("PDF writing not yet implemented".into()))
    }
}
