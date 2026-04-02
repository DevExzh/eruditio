use crate::error::{EruditioError, Result};
use std::io::{Read, Seek};
use zip::ZipArchive;

/// Validates the mimetype file in an EPUB archive.
/// It must be exactly 'application/epub+zip'.
pub fn verify_mimetype<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<()> {
    let mut mimetype_file = archive
        .by_name("mimetype")
        .map_err(|_| EruditioError::Format("Missing mimetype file".to_string()))?;

    let mut contents = String::new();
    mimetype_file
        .read_to_string(&mut contents)
        .map_err(EruditioError::Io)?;

    if contents.trim() != "application/epub+zip" {
        return Err(EruditioError::Format("Invalid EPUB mimetype".to_string()));
    }

    Ok(())
}
