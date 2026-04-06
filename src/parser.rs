use crate::domain::{Book, FormatReader};
use crate::error::{EruditioError, Result};
#[cfg(feature = "cb7")]
use crate::formats::cb7::Cb7Reader;
#[cfg(feature = "cbr")]
use crate::formats::cbr::CbrReader;
use crate::formats::{
    azw4::Azw4Reader, cbc::CbcReader, cbz::CbzReader, chm::ChmReader, djvu::DjvuReader,
    epub::EpubReader, fb2::Fb2Reader, fbz::FbzReader, html::HtmlReader, htmlz::HtmlzReader,
    kepub::KepubReader, lit::LitReader, lrf::LrfReader, md::MdReader, mobi::MobiReader,
    oeb::OebReader, pdb::PdbReader, pdf::PdfReader, pml::PmlReader, pmlz::PmlzReader, rb::RbReader,
    rtf::RtfReader, snb::SnbReader, tcr::TcrReader, txt::TxtReader, txtz::TxtzReader,
};
use std::io::Read;

/// High-level parser that can automatically detect and parse various ebook formats.
///
/// # Relationship to `FormatRegistry`
///
/// [`crate::pipeline::registry::FormatRegistry`] maintains a parallel set of
/// format-to-reader (and writer) mappings used by the conversion [`crate::Pipeline`].
/// The two exist independently because they serve different ergonomic purposes:
///
/// - **`EruditioParser`** is a zero-setup convenience API: give it a `Read` +
///   extension string and get a `Book` back. It matches on string extensions
///   directly, avoiding `Format` enum conversion and `HashMap` lookups.
///
/// - **`FormatRegistry`** is the pipeline's pluggable registry, mapping `Format`
///   enum values to trait-object readers *and* writers. It supports dynamic
///   registration, format enumeration, and the read-transform-write pipeline.
///
/// Delegating `EruditioParser` to `FormatRegistry` would add unnecessary overhead
/// (constructing all writers, HashMap allocation) for users who just want to read
/// a single file. If a new format reader is added, it must be registered in both
/// places — see `FormatRegistry::new()` for the pipeline side.
#[must_use]
pub struct EruditioParser;

impl EruditioParser {
    /// Parse an ebook from a reader, using a format hint (typically a file extension).
    pub fn parse<R: Read>(reader: &mut R, format_hint: Option<&str>) -> Result<Book> {
        match format_hint {
            Some(fmt) => match fmt.to_ascii_lowercase().as_str() {
                "cbz" => CbzReader::new().read_book(reader),
                #[cfg(feature = "cb7")]
                "cb7" => Cb7Reader::new().read_book(reader),
                #[cfg(feature = "cbr")]
                "cbr" => CbrReader::new().read_book(reader),
                "cbc" => CbcReader::new().read_book(reader),
                "djvu" | "djv" => DjvuReader::new().read_book(reader),
                "epub" => EpubReader::new().read_book(reader),
                "mobi" | "azw" | "azw3" | "prc" | "kf8" | "kfx" | "pobi" => {
                    MobiReader::new().read_book(reader)
                },
                "pdf" => PdfReader::new().read_book(reader),
                "fb2" => Fb2Reader::new().read_book(reader),
                "fbz" | "fb2.zip" => FbzReader::new().read_book(reader),
                "txt" | "text" => TxtReader::new().read_book(reader),
                "txtz" => TxtzReader::new().read_book(reader),
                "tcr" => TcrReader::new().read_book(reader),
                "htm" | "html" | "xhtml" | "xhtm" => HtmlReader::new().read_book(reader),
                "htmlz" => HtmlzReader::new().read_book(reader),
                "rtf" => RtfReader::new().read_book(reader),
                "kepub" | "kepub.epub" => KepubReader::new().read_book(reader),
                "pdb" | "updb" => PdbReader::new().read_book(reader),
                "pml" => PmlReader::new().read_book(reader),
                "pmlz" => PmlzReader::new().read_book(reader),
                "rb" => RbReader::new().read_book(reader),
                "lrf" | "lrs" => LrfReader::new().read_book(reader),
                "snb" => SnbReader::new().read_book(reader),
                "chm" => ChmReader::new().read_book(reader),
                "lit" => LitReader::new().read_book(reader),
                "md" | "markdown" => MdReader::new().read_book(reader),
                "azw4" => Azw4Reader::new().read_book(reader),
                "oeb" | "opf" => OebReader::new().read_book(reader),
                _ => Err(EruditioError::Unsupported(format!(
                    "Unsupported format: {}",
                    fmt
                ))),
            },
            None => {
                // Default to EPUB if no hint provided.
                EpubReader::new().read_book(reader)
            },
        }
    }

    /// Convenience method to parse an ebook from a file path.
    #[cfg(feature = "native-fs")]
    pub fn parse_file<P: AsRef<std::path::Path>>(path: P) -> Result<Book> {
        let path_ref = path.as_ref();
        let extension = path_ref.extension().and_then(|e| e.to_str()).unwrap_or("");

        let mut file = std::fs::File::open(path_ref)?;
        Self::parse(&mut file, Some(extension))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_unsupported_format() {
        let mut data = Cursor::new(b"dummy data");
        let result = EruditioParser::parse(&mut data, Some("unknown"));
        assert!(result.is_err());
        if let Err(EruditioError::Unsupported(msg)) = result {
            assert_eq!(msg, "Unsupported format: unknown");
        } else {
            panic!("Expected Unsupported error");
        }
    }

    #[test]
    fn test_parse_pdf_not_implemented() {
        let mut data = Cursor::new(b"dummy pdf data");
        let result = EruditioParser::parse(&mut data, Some("pdf"));
        assert!(result.is_err());
        if let Err(EruditioError::Unsupported(msg)) = result {
            assert_eq!(msg, "PDF reading not yet implemented");
        } else {
            panic!("Expected Unsupported error");
        }
    }
}
