use crate::domain::Manifest;
use crate::domain::manifest::ManifestData;
use crate::error::{EruditioError, Result};
use std::io::{Read, Seek};
use std::sync::Arc;
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
pub(crate) fn load_manifest_data<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &mut Manifest,
    opf_dir: &str,
) -> Result<()> {
    let ids_to_load: Vec<String> = manifest
        .ids()
        .filter(|id| {
            manifest
                .get(id)
                .map_or(false, |item| item.data.is_empty())
        })
        .map(String::from)
        .collect();

    for id in &ids_to_load {
        let item = match manifest.get(id) {
            Some(i) => i,
            None => continue,
        };

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
                    },
                }
            },
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
pub(crate) fn resolve_href(opf_dir: &str, href: &str) -> String {
    if opf_dir.is_empty() || href.starts_with('/') {
        return href.to_string();
    }
    let mut result = String::with_capacity(opf_dir.len() + href.len());
    result.push_str(opf_dir);
    result.push_str(href);
    result
}

/// Extracts the directory portion of the OPF path (including trailing slash).
pub(crate) fn opf_directory(opf_path: &str) -> String {
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
        // Read as raw bytes first, then validate UTF-8 once. This is faster
        // than `read_to_string` which validates incrementally on each chunk.
        let size_hint = (file.size() as usize).min(256 * 1024 * 1024);
        let mut bytes = Vec::with_capacity(size_hint);
        file.read_to_end(&mut bytes)?;
        let text = match String::from_utf8(bytes) {
            Ok(s) => s, // Fast path: valid UTF-8, wraps the Vec with zero copy.
            Err(e) => {
                // Fallback for EPUBs with Windows-1252 or other non-UTF-8 content.
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            },
        };
        Ok(ManifestData::Text(text))
    } else {
        let size_hint = (file.size() as usize).min(256 * 1024 * 1024);
        let mut data = Vec::with_capacity(size_hint);
        file.read_to_end(&mut data)?;
        Ok(ManifestData::Inline(Arc::new(data)))
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
        assert_eq!(
            resolve_href("OEBPS/", "chapter1.xhtml"),
            "OEBPS/chapter1.xhtml"
        );
        assert_eq!(
            resolve_href("OEBPS/", "images/cover.jpg"),
            "OEBPS/images/cover.jpg"
        );
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
