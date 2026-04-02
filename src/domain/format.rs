use std::fmt;
use std::io::Read;

use crate::error::{EruditioError, Result};

/// All ebook formats supported (or planned) by eruditio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Epub,
    Mobi,
    Azw,
    Azw3,
    Azw4,
    Pdf,
    Fb2,
    Fbz,
    Cbz,
    Cbr,
    Cb7,
    Cbc,
    Txt,
    Txtz,
    Tcr,
    Rtf,
    Html,
    Htmlz,
    Docx,
    Odt,
    Pdb,
    Pml,
    Pmlz,
    Prc,
    Lit,
    Lrf,
    Chm,
    Djvu,
    Snb,
    Rb,
    Kepub,
    Md,
    Oeb,
    Zip,
}

impl Format {
    /// Attempts to determine the format from a file extension (case-insensitive).
    ///
    /// Avoids heap allocation by lowering ASCII in a stack buffer.
    pub fn from_extension(ext: &str) -> Option<Self> {
        // File extensions are always short ASCII; use a stack buffer to avoid
        // the `to_lowercase()` heap allocation.
        let ext_bytes = ext.as_bytes();
        if ext_bytes.len() > 16 {
            return None;
        }
        let mut buf = [0u8; 16];
        for (i, &b) in ext_bytes.iter().enumerate() {
            buf[i] = b.to_ascii_lowercase();
        }
        let lower = std::str::from_utf8(&buf[..ext_bytes.len()]).ok()?;
        match lower {
            "epub" => Some(Self::Epub),
            "mobi" => Some(Self::Mobi),
            "kepub" | "kepub.epub" => Some(Self::Kepub),
            "azw" => Some(Self::Azw),
            "azw3" | "kf8" | "kfx" => Some(Self::Azw3),
            "azw4" => Some(Self::Azw4),
            "pdf" => Some(Self::Pdf),
            "fb2" => Some(Self::Fb2),
            "fbz" | "fb2.zip" => Some(Self::Fbz),
            "cbz" => Some(Self::Cbz),
            "cbr" => Some(Self::Cbr),
            "cb7" => Some(Self::Cb7),
            "cbc" => Some(Self::Cbc),
            "txt" | "text" => Some(Self::Txt),
            "txtz" => Some(Self::Txtz),
            "tcr" => Some(Self::Tcr),
            "rtf" => Some(Self::Rtf),
            "htm" | "html" | "xhtml" | "xhtm" => Some(Self::Html),
            "htmlz" => Some(Self::Htmlz),
            "docx" => Some(Self::Docx),
            "odt" => Some(Self::Odt),
            "pdb" | "updb" => Some(Self::Pdb),
            "pml" => Some(Self::Pml),
            "pmlz" => Some(Self::Pmlz),
            "prc" => Some(Self::Prc),
            "lit" => Some(Self::Lit),
            "lrf" | "lrs" => Some(Self::Lrf),
            "chm" => Some(Self::Chm),
            "djvu" | "djv" => Some(Self::Djvu),
            "snb" => Some(Self::Snb),
            "rb" => Some(Self::Rb),
            "md" | "markdown" => Some(Self::Md),
            "oeb" | "opf" | "oebzip" => Some(Self::Oeb),
            "zip" => Some(Self::Zip),
            _ => None,
        }
    }

    /// Returns the canonical file extension for this format.
    pub fn extension(&self) -> &str {
        match self {
            Self::Epub => "epub",
            Self::Mobi => "mobi",
            Self::Azw => "azw",
            Self::Azw3 => "azw3",
            Self::Azw4 => "azw4",
            Self::Pdf => "pdf",
            Self::Fb2 => "fb2",
            Self::Fbz => "fbz",
            Self::Cbz => "cbz",
            Self::Cbr => "cbr",
            Self::Cb7 => "cb7",
            Self::Cbc => "cbc",
            Self::Txt => "txt",
            Self::Txtz => "txtz",
            Self::Tcr => "tcr",
            Self::Rtf => "rtf",
            Self::Html => "html",
            Self::Htmlz => "htmlz",
            Self::Docx => "docx",
            Self::Odt => "odt",
            Self::Pdb => "pdb",
            Self::Pml => "pml",
            Self::Pmlz => "pmlz",
            Self::Prc => "prc",
            Self::Lit => "lit",
            Self::Lrf => "lrf",
            Self::Chm => "chm",
            Self::Djvu => "djvu",
            Self::Snb => "snb",
            Self::Rb => "rb",
            Self::Kepub => "kepub",
            Self::Md => "md",
            Self::Oeb => "oeb",
            Self::Zip => "zip",
        }
    }

    /// Returns the primary MIME type for this format.
    pub fn mime_type(&self) -> &str {
        match self {
            Self::Epub | Self::Kepub => "application/epub+zip",
            Self::Mobi | Self::Azw | Self::Prc => "application/x-mobipocket-ebook",
            Self::Azw3 => "application/x-mobi8-ebook",
            Self::Azw4 => "application/pdf",
            Self::Pdf => "application/pdf",
            Self::Fb2 => "application/x-fictionbook+xml",
            Self::Fbz => "application/x-zip-compressed-fb2",
            Self::Cbz => "application/vnd.comicbook+zip",
            Self::Cbr => "application/vnd.comicbook-rar",
            Self::Cb7 => "application/x-cb7",
            Self::Cbc => "application/x-cbc",
            Self::Txt | Self::Tcr => "text/plain",
            Self::Txtz => "application/x-txtz",
            Self::Rtf => "application/rtf",
            Self::Html => "text/html",
            Self::Htmlz => "application/x-htmlz",
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Odt => "application/vnd.oasis.opendocument.text",
            Self::Pdb => "application/x-palm-database",
            Self::Pml | Self::Pmlz => "application/x-pml",
            Self::Lit => "application/x-ms-reader",
            Self::Lrf => "application/x-sony-bbeb",
            Self::Chm => "application/vnd.ms-htmlhelp",
            Self::Djvu => "image/vnd.djvu",
            Self::Snb => "application/x-snb",
            Self::Md => "text/markdown",
            Self::Rb => "application/x-rocketbook",
            Self::Oeb => "application/oebps-package+xml",
            Self::Zip => "application/zip",
        }
    }

    /// Attempts to detect the format by reading magic bytes from the start of a stream.
    /// Reads up to 16 bytes; the stream position advances.
    pub fn detect<R: Read>(reader: &mut R) -> Result<Self> {
        let mut buf = [0u8; 16];
        let n = reader.read(&mut buf).map_err(EruditioError::Io)?;
        let bytes = &buf[..n];

        Self::from_magic_bytes(bytes)
            .ok_or_else(|| EruditioError::Format("Unable to detect format from magic bytes".into()))
    }

    /// Identifies the format from a slice of leading bytes (at least 8 bytes recommended).
    pub fn from_magic_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }

        // PDF: %PDF
        if bytes.starts_with(b"%PDF") {
            return Some(Self::Pdf);
        }

        // ZIP-based formats: PK\x03\x04
        if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
            // Distinguish by looking at filenames within the ZIP is
            // complex without reading more. Return Zip as a generic marker;
            // callers should refine using extension or internal inspection.
            return Some(Self::Zip);
        }

        // MOBI/PRC/AZW: check for BOOKMOBI at offset 60 (within PDB header)
        // but we only have first 16 bytes here. The PDB header's type+creator
        // fields are at offset 60-67. We can't read that with 16 bytes alone.
        // Instead, check common PDB signatures: the first 32 bytes are the DB name.

        // LIT: ITOLITLS at offset 0
        if bytes.len() >= 8 && &bytes[0..8] == b"ITOLITLS" {
            return Some(Self::Lit);
        }

        // DjVu: AT&TFORM
        if bytes.len() >= 8 && &bytes[0..8] == b"AT&TFORM" {
            return Some(Self::Djvu);
        }

        // TCR: !!8-Bit!!
        if bytes.len() >= 9 && &bytes[0..9] == b"!!8-Bit!!" {
            return Some(Self::Tcr);
        }

        // CHM: ITSF signature
        if bytes.len() >= 4 && &bytes[0..4] == b"ITSF" {
            return Some(Self::Chm);
        }

        // RTF: {\rtf
        if bytes.len() >= 5 && &bytes[0..5] == b"{\\rtf" {
            return Some(Self::Rtf);
        }

        // LRF: L\x00R\x00F\x00\x00\x00
        if bytes.len() >= 8
            && bytes[0] == 0x4C
            && bytes[1] == 0x00
            && bytes[2] == 0x52
            && bytes[3] == 0x00
            && bytes[4] == 0x46
            && bytes[5] == 0x00
        {
            return Some(Self::Lrf);
        }

        // RAR: Rar!\x1a\x07
        if bytes.len() >= 6 && &bytes[0..6] == b"Rar!\x1a\x07" {
            return Some(Self::Cbr);
        }

        // 7z: 7z\xbc\xaf\x27\x1c
        if bytes.len() >= 6 && &bytes[0..6] == b"7z\xbc\xaf\x27\x1c" {
            return Some(Self::Cb7);
        }

        None
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Format {
    /// Returns the uppercase display name for this format.
    /// Uses a static string to avoid per-call allocation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Epub => "EPUB",
            Self::Mobi => "MOBI",
            Self::Azw => "AZW",
            Self::Azw3 => "AZW3",
            Self::Azw4 => "AZW4",
            Self::Pdf => "PDF",
            Self::Fb2 => "FB2",
            Self::Fbz => "FBZ",
            Self::Cbz => "CBZ",
            Self::Cbr => "CBR",
            Self::Cb7 => "CB7",
            Self::Cbc => "CBC",
            Self::Txt => "TXT",
            Self::Txtz => "TXTZ",
            Self::Tcr => "TCR",
            Self::Rtf => "RTF",
            Self::Html => "HTML",
            Self::Htmlz => "HTMLZ",
            Self::Docx => "DOCX",
            Self::Odt => "ODT",
            Self::Pdb => "PDB",
            Self::Pml => "PML",
            Self::Pmlz => "PMLZ",
            Self::Prc => "PRC",
            Self::Lit => "LIT",
            Self::Lrf => "LRF",
            Self::Chm => "CHM",
            Self::Djvu => "DJVU",
            Self::Snb => "SNB",
            Self::Rb => "RB",
            Self::Kepub => "KEPUB",
            Self::Md => "MD",
            Self::Oeb => "OEB",
            Self::Zip => "ZIP",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_round_trip() {
        let formats = [
            Format::Epub,
            Format::Mobi,
            Format::Pdf,
            Format::Fb2,
            Format::Cbz,
            Format::Txt,
            Format::Tcr,
            Format::Rtf,
            Format::Html,
            Format::Docx,
            Format::Odt,
        ];
        for fmt in formats {
            let ext = fmt.extension();
            assert_eq!(Format::from_extension(ext), Some(fmt));
        }
    }

    #[test]
    fn case_insensitive_extension() {
        assert_eq!(Format::from_extension("EPUB"), Some(Format::Epub));
        assert_eq!(Format::from_extension("Pdf"), Some(Format::Pdf));
        assert_eq!(Format::from_extension("TXT"), Some(Format::Txt));
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert_eq!(Format::from_extension("xyz"), None);
    }

    #[test]
    fn magic_bytes_pdf() {
        assert_eq!(Format::from_magic_bytes(b"%PDF-1.7"), Some(Format::Pdf));
    }

    #[test]
    fn magic_bytes_zip() {
        assert_eq!(
            Format::from_magic_bytes(&[0x50, 0x4B, 0x03, 0x04, 0x00]),
            Some(Format::Zip)
        );
    }

    #[test]
    fn magic_bytes_tcr() {
        assert_eq!(
            Format::from_magic_bytes(b"!!8-Bit!!rest"),
            Some(Format::Tcr)
        );
    }

    #[test]
    fn magic_bytes_rtf() {
        assert_eq!(Format::from_magic_bytes(b"{\\rtf1}"), Some(Format::Rtf));
    }

    #[test]
    fn magic_bytes_too_short() {
        assert_eq!(Format::from_magic_bytes(b"PK"), None);
    }

    #[test]
    fn display_shows_uppercase() {
        assert_eq!(format!("{}", Format::Epub), "EPUB");
        assert_eq!(format!("{}", Format::Pdf), "PDF");
    }
}
