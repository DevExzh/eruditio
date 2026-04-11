use crate::domain::Manifest;
use crate::domain::manifest::ManifestData;
use crate::error::{EruditioError, Result};
use flate2::{Decompress, FlushDecompress};
use rayon::prelude::*;
use std::io::{Cursor, Read, Seek};
use std::sync::Arc;
use zip::CompressionMethod;
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
        Ok(ManifestData::Inline(Arc::from(data)))
    }
}

/// Reads a single file from the ZIP, reusing a `flate2::Decompress` instance
/// to avoid per-entry inflate state allocation (~11 KB per entry).
///
/// Falls back to `read_from_archive` for non-Deflate compression methods.
fn read_from_archive_reuse<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    path: &str,
    as_text: bool,
    decompressor: &mut Decompress,
    raw_buf: &mut Vec<u8>,
) -> Result<ManifestData> {
    let idx = match archive.index_for_name(path) {
        Some(i) => i,
        None => {
            return Err(EruditioError::Format(format!(
                "File not found in EPUB: {}",
                path
            )));
        },
    };

    // Get entry metadata (compression method, sizes).
    let (compression, compressed_size, uncompressed_size) = {
        let file = archive.by_index_raw(idx).map_err(|_| {
            EruditioError::Format(format!("Cannot read raw entry in EPUB: {}", path))
        })?;
        (
            file.compression(),
            file.compressed_size() as usize,
            file.size() as usize,
        )
    };

    match compression {
        CompressionMethod::Stored => {
            // Stored entries: read directly, no decompression.
            let mut file = archive.by_index(idx).map_err(|_| {
                EruditioError::Format(format!("Cannot read entry in EPUB: {}", path))
            })?;
            let size_hint = uncompressed_size.min(256 * 1024 * 1024);
            let mut bytes = Vec::with_capacity(size_hint);
            file.read_to_end(&mut bytes)?;
            bytes_to_manifest_data(bytes, as_text)
        },
        CompressionMethod::Deflated => {
            // Read raw compressed bytes.
            raw_buf.clear();
            raw_buf.reserve(compressed_size.min(256 * 1024 * 1024));
            {
                let mut raw_file = archive.by_index_raw(idx).map_err(|_| {
                    EruditioError::Format(format!("Cannot read raw entry in EPUB: {}", path))
                })?;
                raw_file.read_to_end(raw_buf)?;
            }

            // Decompress with reusable decompressor.
            decompressor.reset(false);
            let out_cap = uncompressed_size.min(256 * 1024 * 1024);
            let mut output = Vec::with_capacity(out_cap);
            decompressor
                .decompress_vec(raw_buf, &mut output, FlushDecompress::Finish)
                .map_err(|e| EruditioError::Parse(format!("Deflate error in {}: {}", path, e)))?;

            bytes_to_manifest_data(output, as_text)
        },
        // Fallback for other compression methods (bzip2, zstd, etc.).
        _ => read_from_archive(archive, path, as_text),
    }
}

/// Converts raw bytes into the appropriate ManifestData variant.
fn bytes_to_manifest_data(bytes: Vec<u8>, as_text: bool) -> Result<ManifestData> {
    if as_text {
        // Fast path: if all bytes are ASCII, they are guaranteed valid UTF-8.
        // Skip the full from_utf8 validation (~3% of EPUB parsing cost) for
        // ASCII-only content (common: CSS, many English-text XHTML files).
        let text = if crate::formats::common::intrinsics::is_ascii::is_all_ascii(&bytes) {
            // SAFETY: every byte is < 0x80, which is valid single-byte UTF-8.
            unsafe { String::from_utf8_unchecked(bytes) }
        } else {
            match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
            }
        };
        Ok(ManifestData::Text(text))
    } else {
        Ok(ManifestData::Inline(Arc::from(bytes)))
    }
}

/// Minimum manifest entries to trigger parallel decompression.
/// Below this, sequential is faster due to rayon thread pool + ZIP re-parse overhead.
const PARALLEL_THRESHOLD: usize = 20;

/// Loads data for all manifest items from the EPUB ZIP archive.
/// Uses parallel decompression for large EPUBs, sequential for small ones.
pub(crate) fn load_manifest_data_parallel(
    archive: ZipArchive<Cursor<Vec<u8>>>,
    manifest: &mut Manifest,
    opf_dir: &str,
) -> Result<()> {
    // Count entries that need loading.
    let entry_count = manifest
        .ids()
        .filter(|id| {
            manifest
                .get(id)
                .is_some_and(|item| item.data.is_empty())
        })
        .count();

    if entry_count == 0 {
        return Ok(());
    }

    // For small/medium EPUBs: use the archive directly (no buffer extraction overhead).
    // Parallel decompression only pays off for EPUBs with many entries where
    // concurrent Deflate across threads outweighs ZIP central directory re-parse.
    if entry_count < PARALLEL_THRESHOLD {
        return load_sequential(archive, manifest, opf_dir, entry_count);
    }

    // Large EPUB: parallel decompression with per-thread ZipArchive.
    load_parallel(archive, manifest, opf_dir, entry_count)
}

/// Sequential manifest loading — uses a single reusable decompressor.
fn load_sequential(
    mut archive: ZipArchive<Cursor<Vec<u8>>>,
    manifest: &mut Manifest,
    opf_dir: &str,
    entry_count: usize,
) -> Result<()> {
    let mut ids_to_load = Vec::with_capacity(entry_count);
    ids_to_load.extend(
        manifest
            .ids()
            .filter(|id| {
                manifest
                    .get(id)
                    .is_some_and(|item| item.data.is_empty())
            })
            .map(String::from),
    );

    // Reusable decompressor and compressed-data buffer across all entries.
    // Avoids ~11 KB heap allocation per Deflate entry.
    let mut decompressor = Decompress::new(false);
    let mut raw_buf = Vec::new();

    for id in &ids_to_load {
        let item = match manifest.get(id) {
            Some(i) => i,
            None => continue,
        };

        let zip_path = resolve_href(opf_dir, &item.href);
        let is_text = is_text_media_type(&item.media_type);

        let data = match read_from_archive_reuse(
            &mut archive,
            &zip_path,
            is_text,
            &mut decompressor,
            &mut raw_buf,
        ) {
            Ok(d) => d,
            Err(_) => {
                match read_from_archive_reuse(
                    &mut archive,
                    &item.href,
                    is_text,
                    &mut decompressor,
                    &mut raw_buf,
                ) {
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

/// Parallel manifest loading — extracts the buffer and uses rayon.
fn load_parallel(
    archive: ZipArchive<Cursor<Vec<u8>>>,
    manifest: &mut Manifest,
    opf_dir: &str,
    entry_count: usize,
) -> Result<()> {
    let mut entries: Vec<(String, String, String, bool)> = Vec::with_capacity(entry_count);
    entries.extend(manifest
        .ids()
        .filter(|id| {
            manifest
                .get(id)
                .is_some_and(|item| item.data.is_empty())
        })
        .filter_map(|id| {
            let item = manifest.get(&id)?;
            let zip_path = resolve_href(opf_dir, &item.href);
            let fallback = item.href.clone();
            let is_text = is_text_media_type(&item.media_type);
            Some((id.to_string(), zip_path, fallback, is_text))
        }));

    let zip_data = archive.into_inner().into_inner();
    let zip_ref = zip_data.as_slice();
    let results: Vec<(String, ManifestData)> = entries
        .into_par_iter()
        .map_init(
            || {
                (
                    ZipArchive::new(Cursor::new(zip_ref)),
                    Decompress::new(false),
                    Vec::new(),
                )
            },
            |(archive_result, decompressor, raw_buf),
             (id, zip_path, fallback, is_text)| {
                let archive = match archive_result {
                    Ok(a) => a,
                    Err(_) => return None,
                };
                let data = match read_from_archive_reuse(
                    archive, &zip_path, is_text, decompressor, raw_buf,
                ) {
                    Ok(d) => d,
                    Err(_) => {
                        match read_from_archive_reuse(
                            archive, &fallback, is_text, decompressor, raw_buf,
                        ) {
                            Ok(d) => d,
                            Err(_) => return None,
                        }
                    },
                };
                Some((id, data))
            },
        )
        .flatten()
        .collect();

    for (id, data) in results {
        if let Some(item_mut) = manifest.get_mut(&id) {
            item_mut.data = data;
        }
    }

    Ok(())
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
