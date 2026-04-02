use crate::domain::Manifest;
use crate::error::{EruditioError, Result};
use crate::domain::manifest::ManifestData;
use std::io::{Read, Seek};
use zip::ZipArchive;

/// Text-based media types whose content should be stored as `ManifestData::Text`.
const TEXT_MEDIA_TYPES: &[&str] = &[
    "application/xhtml+xml",
    "text/html",
    "text/css",
    "application/x-dtbncx+xml",
    "application/xml",
    "text/xml",
    "application/smil+xml",
    "application/javascript",
    "text/javascript",
    "text/plain",
    "application/json",
];

/// Loads data for all manifest items from the EPUB ZIP archive.
///
/// For each item in the manifest, resolves the full path within the ZIP
/// (relative to the OPF directory) and reads the file content. Text-based
/// media types are stored as `ManifestData::Text`, binary types as
/// `ManifestData::Inline`. Items whose files are missing in the ZIP are
/// left as `ManifestData::Empty`.
pub fn load_manifest_data<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &mut Manifest,
    opf_dir: &str,
) -> Result<()> {
    let ids: Vec<String> = manifest.ids().map(String::from).collect();

    for id in &ids {
        let item = match manifest.get(id) {
            Some(i) => i,
            None => continue,
        };

        // Skip items that already have data loaded.
        if !item.data.is_empty() {
            continue;
        }

        let zip_path = resolve_href(opf_dir, &item.href);
        let is_text = is_text_media_type(&item.media_type);

        // Try to read from the ZIP archive.
        let data = match read_from_archive(archive, &zip_path, is_text) {
            Ok(d) => d,
            Err(_) => {
                // File missing in archive — try the href as-is (some EPUBs use absolute paths).
                match read_from_archive(archive, &item.href, is_text) {
                    Ok(d) => d,
                    Err(_) => {
                        log::warn!("EPUB: missing file in archive: {}", zip_path);
                        continue;
                    }
                }
            }
        };

        if let Some(item_mut) = manifest.get_mut(id) {
            item_mut.data = data;
        }
    }

    Ok(())
}

/// Resolves a manifest href relative to the OPF directory.
///
/// If the OPF is at `OEBPS/content.opf`, then `opf_dir` is `OEBPS/`
/// and a manifest href `chapter1.xhtml` resolves to `OEBPS/chapter1.xhtml`.
pub fn resolve_href(opf_dir: &str, href: &str) -> String {
    if opf_dir.is_empty() || href.starts_with('/') {
        return href.to_string();
    }
    format!("{}{}", opf_dir, href)
}

/// Extracts the directory portion of the OPF path (including trailing slash).
pub fn opf_directory(opf_path: &str) -> String {
    match opf_path.rfind('/') {
        Some(pos) => opf_path[..=pos].to_string(),
        None => String::new(),
    }
}

/// Determines if a media type should be treated as text content.
fn is_text_media_type(media_type: &str) -> bool {
    TEXT_MEDIA_TYPES.contains(&media_type) || media_type.starts_with("text/")
}

/// Reads a single file from the ZIP, returning it as the appropriate `ManifestData`.
fn read_from_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
    as_text: bool,
) -> Result<ManifestData> {
    let mut file = archive
        .by_name(path)
        .map_err(|_| EruditioError::Format(format!("File not found in EPUB: {}", path)))?;

    if as_text {
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(EruditioError::Io)?;
        Ok(ManifestData::Text(text))
    } else {
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(EruditioError::Io)?;
        Ok(ManifestData::Inline(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opf_directory_extracts_dir() {
        assert_eq!(opf_directory("OEBPS/content.opf"), "OEBPS/");
        assert_eq!(opf_directory("content.opf"), "");
        assert_eq!(opf_directory("a/b/c/package.opf"), "a/b/c/");
    }

    #[test]
    fn resolve_href_joins_paths() {
        assert_eq!(resolve_href("OEBPS/", "chapter1.xhtml"), "OEBPS/chapter1.xhtml");
        assert_eq!(resolve_href("OEBPS/", "images/cover.jpg"), "OEBPS/images/cover.jpg");
        assert_eq!(resolve_href("", "chapter1.xhtml"), "chapter1.xhtml");
    }

    #[test]
    fn resolve_href_handles_absolute() {
        assert_eq!(resolve_href("OEBPS/", "/chapter1.xhtml"), "/chapter1.xhtml");
    }

    #[test]
    fn text_media_types_detected() {
        assert!(is_text_media_type("application/xhtml+xml"));
        assert!(is_text_media_type("text/css"));
        assert!(is_text_media_type("text/plain"));
        assert!(!is_text_media_type("image/jpeg"));
        assert!(!is_text_media_type("application/font-woff"));
    }
}
