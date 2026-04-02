use std::collections::HashMap;

use crate::domain::format::Format;
use crate::domain::traits::{FormatReader, FormatWriter};
use crate::formats::{
    cbz::{CbzReader, CbzWriter},
    cb7::Cb7Reader,
    cbr::CbrReader,
    cbc::CbcReader,
    chm::ChmReader,
    djvu::DjvuReader,
    epub::{EpubReader, EpubWriter},
    fb2::{Fb2Reader, Fb2Writer},
    fbz::{FbzReader, FbzWriter},
    html::{HtmlReader, HtmlWriter},
    htmlz::{HtmlzReader, HtmlzWriter},
    kepub::{KepubReader, KepubWriter},
    lit::LitReader,
    lrf::LrfReader,
    md::MdReader,
    mobi::{MobiReader, MobiWriter},
    pdb::{PdbReader, PdbWriter},
    pdf::{PdfReader, PdfWriter},
    pml::{PmlReader, PmlWriter},
    pmlz::{PmlzReader, PmlzWriter},
    rb::{RbReader, RbWriter},
    rtf::{RtfReader, RtfWriter},
    snb::{SnbReader, SnbWriter},
    tcr::{TcrReader, TcrWriter},
    txt::{TxtReader, TxtWriter},
    txtz::{TxtzReader, TxtzWriter},
};

/// Registry mapping `Format` values to their reader/writer implementations.
pub struct FormatRegistry {
    readers: HashMap<Format, Box<dyn FormatReader>>,
    writers: HashMap<Format, Box<dyn FormatWriter>>,
}

impl FormatRegistry {
    /// Creates a registry pre-populated with all built-in format handlers.
    pub fn new() -> Self {
        let mut registry = Self {
            readers: HashMap::new(),
            writers: HashMap::new(),
        };

        // EPUB
        registry.register_reader(Format::Epub, Box::new(EpubReader::new()));
        registry.register_writer(Format::Epub, Box::new(EpubWriter::new()));

        // FB2
        registry.register_reader(Format::Fb2, Box::new(Fb2Reader::new()));
        registry.register_writer(Format::Fb2, Box::new(Fb2Writer::new()));

        // CBZ
        registry.register_reader(Format::Cbz, Box::new(CbzReader::new()));
        registry.register_writer(Format::Cbz, Box::new(CbzWriter::new()));

        // CB7 (Comic Book 7z, read-only)
        registry.register_reader(Format::Cb7, Box::new(Cb7Reader::new()));

        // CBR (Comic Book RAR, read-only)
        registry.register_reader(Format::Cbr, Box::new(CbrReader::new()));

        // CBC (Comic Book Collection, read-only)
        registry.register_reader(Format::Cbc, Box::new(CbcReader::new()));

        // DJVU (DjVu text layer extraction, read-only)
        registry.register_reader(Format::Djvu, Box::new(DjvuReader::new()));

        // CHM (Compiled HTML Help, read-only)
        registry.register_reader(Format::Chm, Box::new(ChmReader::new()));

        // LIT (Microsoft Reader, read-only)
        registry.register_reader(Format::Lit, Box::new(LitReader::new()));

        // TXT
        registry.register_reader(Format::Txt, Box::new(TxtReader::new()));
        registry.register_writer(Format::Txt, Box::new(TxtWriter::new()));

        // TCR
        registry.register_reader(Format::Tcr, Box::new(TcrReader::new()));
        registry.register_writer(Format::Tcr, Box::new(TcrWriter::new()));

        // MOBI / AZW / AZW3 / PRC (all map to the same reader/writer)
        registry.register_reader(Format::Mobi, Box::new(MobiReader::new()));
        registry.register_writer(Format::Mobi, Box::new(MobiWriter::new()));
        registry.register_reader(Format::Azw, Box::new(MobiReader::new()));
        registry.register_reader(Format::Azw3, Box::new(MobiReader::new()));
        registry.register_reader(Format::Prc, Box::new(MobiReader::new()));

        // PDF
        registry.register_reader(Format::Pdf, Box::new(PdfReader::new()));
        registry.register_writer(Format::Pdf, Box::new(PdfWriter::new()));

        // FBZ (FB2-in-ZIP)
        registry.register_reader(Format::Fbz, Box::new(FbzReader::new()));
        registry.register_writer(Format::Fbz, Box::new(FbzWriter::new()));

        // TXTZ (TXT-in-ZIP)
        registry.register_reader(Format::Txtz, Box::new(TxtzReader::new()));
        registry.register_writer(Format::Txtz, Box::new(TxtzWriter::new()));

        // HTML
        registry.register_reader(Format::Html, Box::new(HtmlReader::new()));
        registry.register_writer(Format::Html, Box::new(HtmlWriter::new()));

        // HTMLZ (HTML-in-ZIP)
        registry.register_reader(Format::Htmlz, Box::new(HtmlzReader::new()));
        registry.register_writer(Format::Htmlz, Box::new(HtmlzWriter::new()));

        // RTF
        registry.register_reader(Format::Rtf, Box::new(RtfReader::new()));
        registry.register_writer(Format::Rtf, Box::new(RtfWriter::new()));

        // Kepub (Kobo EPUB)
        registry.register_reader(Format::Kepub, Box::new(KepubReader::new()));
        registry.register_writer(Format::Kepub, Box::new(KepubWriter::new()));

        // LRF (Sony BBeB, read-only)
        registry.register_reader(Format::Lrf, Box::new(LrfReader::new()));

        // PDB (Palm Database — PalmDOC subtype)
        registry.register_reader(Format::Pdb, Box::new(PdbReader::new()));
        registry.register_writer(Format::Pdb, Box::new(PdbWriter::new()));

        // PML
        registry.register_reader(Format::Pml, Box::new(PmlReader::new()));
        registry.register_writer(Format::Pml, Box::new(PmlWriter::new()));

        // PMLZ (PML-in-ZIP)
        registry.register_reader(Format::Pmlz, Box::new(PmlzReader::new()));
        registry.register_writer(Format::Pmlz, Box::new(PmlzWriter::new()));

        // RB (RocketBook)
        registry.register_reader(Format::Rb, Box::new(RbReader::new()));
        registry.register_writer(Format::Rb, Box::new(RbWriter::new()));

        // SNB (Shanda Bambook)
        registry.register_reader(Format::Snb, Box::new(SnbReader::new()));
        registry.register_writer(Format::Snb, Box::new(SnbWriter::new()));

        // Markdown
        registry.register_reader(Format::Md, Box::new(MdReader::new()));

        registry
    }

    pub fn register_reader(&mut self, format: Format, reader: Box<dyn FormatReader>) {
        self.readers.insert(format, reader);
    }

    pub fn register_writer(&mut self, format: Format, writer: Box<dyn FormatWriter>) {
        self.writers.insert(format, writer);
    }

    /// Returns the reader for a given format, if one is registered.
    pub fn reader(&self, format: &Format) -> Option<&dyn FormatReader> {
        self.readers.get(format).map(|r| r.as_ref())
    }

    /// Returns the writer for a given format, if one is registered.
    pub fn writer(&self, format: &Format) -> Option<&dyn FormatWriter> {
        self.writers.get(format).map(|w| w.as_ref())
    }

    /// Returns all formats that have a registered reader.
    pub fn readable_formats(&self) -> Vec<Format> {
        self.readers.keys().copied().collect()
    }

    /// Returns all formats that have a registered writer.
    pub fn writable_formats(&self) -> Vec<Format> {
        self.writers.keys().copied().collect()
    }

    /// Returns `true` if the given format can be read.
    pub fn can_read(&self, format: &Format) -> bool {
        self.readers.contains_key(format)
    }

    /// Returns `true` if the given format can be written.
    pub fn can_write(&self, format: &Format) -> bool {
        self.writers.contains_key(format)
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_epub_reader_and_writer() {
        let registry = FormatRegistry::new();
        assert!(registry.can_read(&Format::Epub));
        assert!(registry.can_write(&Format::Epub));
    }

    #[test]
    fn registry_has_mobi_family_readers() {
        let registry = FormatRegistry::new();
        assert!(registry.can_read(&Format::Mobi));
        assert!(registry.can_read(&Format::Azw));
        assert!(registry.can_read(&Format::Azw3));
        assert!(registry.can_read(&Format::Prc));
    }

    #[test]
    fn unregistered_format_returns_none() {
        let registry = FormatRegistry::new();
        assert!(!registry.can_read(&Format::Docx));
        assert!(registry.reader(&Format::Docx).is_none());
    }

    #[test]
    fn readable_formats_includes_all_registered() {
        let registry = FormatRegistry::new();
        let formats = registry.readable_formats();
        assert!(formats.contains(&Format::Epub));
        assert!(formats.contains(&Format::Fb2));
        assert!(formats.contains(&Format::Cbz));
        assert!(formats.contains(&Format::Txt));
        assert!(formats.contains(&Format::Tcr));
        assert!(formats.contains(&Format::Pdf));
    }
}
