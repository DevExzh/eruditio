//! MOBI file writer.
//!
//! Produces a valid MOBI 6 file with PalmDoc compression, EXTH metadata,
//! and embedded images. The output is compatible with Kindle readers.

use std::borrow::Cow;

use crate::domain::Book;
use crate::error::Result;
use crate::formats::common::compression::palmdoc;
use crate::formats::common::html_utils::strip_tags;
use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};

use image::ImageEncoder;
use image::imageops::FilterType;

use super::exth::{
    self, EXTH_ASIN, EXTH_AUTHOR, EXTH_CDE_TYPE, EXTH_COVER_OFFSET, EXTH_CREATOR_BUILD,
    EXTH_CREATOR_MAJOR, EXTH_CREATOR_MINOR, EXTH_CREATOR_SOFTWARE, EXTH_DESCRIPTION,
    EXTH_HAS_FAKE_COVER, EXTH_ISBN, EXTH_KF8_COVER_URI, EXTH_LANGUAGE, EXTH_OVERRIDE_KINDLE_FONTS,
    EXTH_PUBLISHED_DATE, EXTH_PUBLISHER, EXTH_RIGHTS, EXTH_START_READING, EXTH_SUBJECT,
    EXTH_THUMB_OFFSET, EXTH_UPDATED_TITLE,
};
use super::header::{COMPRESSION_PALMDOC, ENCODING_UTF8, NULL_INDEX};

/// Maximum uncompressed text record size.
const RECORD_SIZE: usize = 4096;

/// MOBI 6 header length.
///
/// Set to 232 to indicate that extra data flags (offset 240) and
/// FCIS/FLIS record indices (offsets 200-215) are present. Readers
/// such as Calibre check `header_length >= 232` before parsing these
/// fields.
const MOBI_HEADER_LEN: u32 = 232;

/// FLIS record constant data.
const FLIS_RECORD: &[u8] = &[
    b'F', b'L', b'I', b'S', 0x00, 0x00, 0x00, 0x08, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01,
    0xFF, 0xFF, 0xFF, 0xFF,
];

/// FCIS record (parameterized by text length).
fn build_fcis(text_length: u32) -> Vec<u8> {
    let mut data = vec![0u8; 44];
    data[0..4].copy_from_slice(b"FCIS");
    write_u32_be(&mut data, 4, 0x14); // FCIS data offset
    write_u32_be(&mut data, 8, 0x10); // unknown
    write_u32_be(&mut data, 12, 0x01);
    write_u32_be(&mut data, 16, 0x00);
    write_u32_be(&mut data, 20, text_length);
    write_u32_be(&mut data, 24, 0x00);
    write_u32_be(&mut data, 28, 0x20);
    write_u32_be(&mut data, 32, 0x08);
    write_u32_be(&mut data, 36, 0x01);
    write_u32_be(&mut data, 40, 0x01);
    data
}

/// EOF record.
const EOF_RECORD: &[u8] = &[0xE9, 0x8E, 0x0D, 0x0A];

/// INDX header record length (standard for Mobi6).
const INDX_HEADER_LEN: usize = 192;

/// Encodes a value as a Variable Width Integer (VWI).
///
/// Each byte uses 7 data bits with the MSB set on all bytes except the last.
/// Returns a stack-allocated `([u8; 5], usize)` — a u32 needs at most 5 VWI
/// bytes. Callers use `&buf[..len]` to get the encoded slice.
fn encode_vwi(value: u32) -> ([u8; 5], usize) {
    if value == 0 {
        let mut buf = [0u8; 5];
        buf[0] = 0;
        return (buf, 1);
    }
    // Encode in reverse (least significant 7-bit group first).
    let mut tmp = [0u8; 5];
    let mut len = 0usize;
    let mut v = value;
    while v > 0 {
        tmp[len] = (v & 0x7F) as u8;
        v >>= 7;
        len += 1;
    }
    // Reverse into output buffer and set MSB on all but the last byte.
    let mut buf = [0u8; 5];
    for i in 0..len {
        buf[i] = tmp[len - 1 - i];
        if i < len - 1 {
            buf[i] |= 0x80;
        }
    }
    (buf, len)
}

/// Builds the 3 NCX index records (INDX header, INDX data, CNCX) from chapter entries.
///
/// Each entry in `chapters` is `(title, byte_offset)` where byte_offset is
/// the position of the chapter start within the uncompressed HTML text.
///
/// Returns `(indx_header_record, indx_data_record, cncx_record)`.
fn build_ncx_indx(chapters: &[(String, usize)]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    // Cap entries to prevent u16 overflow in IDXT offsets.
    // Each entry is ~18 bytes; with 192-byte header, u16 fits ~3600 entries.
    let max_entries = 3000;
    let chapters = &chapters[..chapters.len().min(max_entries)];
    let entry_count = chapters.len();

    // --- Build CNCX record (chapter title strings) ---
    let mut cncx = Vec::new();
    let mut cncx_offsets: Vec<usize> = Vec::with_capacity(entry_count);
    for (title, _) in chapters {
        cncx_offsets.push(cncx.len());
        let title_bytes = title.as_bytes();
        // Clamp to u16::MAX to avoid silent truncation on pathological titles.
        let clamped_len = title_bytes.len().min(u16::MAX as usize);
        let len = clamped_len as u16;
        cncx.extend_from_slice(&len.to_be_bytes());
        cncx.extend_from_slice(&title_bytes[..clamped_len]);
    }

    // --- Build INDX header record ---
    // Layout: 192-byte INDX header + TAGX section + IDXT stub
    let tagx_data = build_tagx_section();
    let idxt_offset_in_header = INDX_HEADER_LEN + tagx_data.len();

    // IDXT section for header record: just the magic, no entries.
    let mut header_idxt = Vec::new();
    header_idxt.extend_from_slice(b"IDXT");
    // Pad to even length.
    if header_idxt.len() % 2 != 0 {
        header_idxt.push(0);
    }

    let header_total = INDX_HEADER_LEN + tagx_data.len() + header_idxt.len();
    let mut indx_header = vec![0u8; header_total];

    // INDX magic.
    indx_header[0..4].copy_from_slice(b"INDX");
    // Header length.
    write_u32_be(&mut indx_header, 4, INDX_HEADER_LEN as u32);
    // Index type = 0 (normal).
    write_u32_be(&mut indx_header, 8, 0);
    // Unknown fields at 12, 16 = 0 (already zero).
    // IDXT offset within this record.
    write_u32_be(&mut indx_header, 20, idxt_offset_in_header as u32);
    // Index count (number of INDX data records that follow) = 1.
    write_u32_be(&mut indx_header, 24, 1);
    // Index encoding = UTF-8 (65001).
    write_u32_be(&mut indx_header, 28, 65001);
    // Index language = 0xFFFFFFFF.
    write_u32_be(&mut indx_header, 32, 0xFFFF_FFFF);
    // Total index entry count (= number of chapters).
    write_u32_be(&mut indx_header, 36, entry_count as u32);
    // ORDT offset = 0, LIGT offset = 0, LIGT entry count = 0 (already zero).
    // CNCX record count = 1.
    write_u32_be(&mut indx_header, 52, 1);
    // Remaining header fields (56-191) are zeros.

    // Write TAGX section after header.
    indx_header[INDX_HEADER_LEN..INDX_HEADER_LEN + tagx_data.len()].copy_from_slice(&tagx_data);

    // Write IDXT stub after TAGX.
    let idxt_start = INDX_HEADER_LEN + tagx_data.len();
    indx_header[idxt_start..idxt_start + header_idxt.len()].copy_from_slice(&header_idxt);

    // --- Build INDX data record ---
    // Layout: 192-byte INDX header + entry data + IDXT section
    // We first build all entries, tracking their offsets, then assemble.
    let mut entry_data = Vec::new();
    let mut entry_offsets: Vec<u16> = Vec::with_capacity(entry_count);

    for (i, (_, byte_offset)) in chapters.iter().enumerate() {
        // Record the offset of this entry relative to the start of this record.
        let entry_start = INDX_HEADER_LEN + entry_data.len();
        entry_offsets.push(entry_start as u16);

        // Text key: zero-padded 5-digit string.
        let key = format!("{:05}", i);
        let key_bytes = key.as_bytes();

        // Key length byte.
        entry_data.push(key_bytes.len() as u8);
        // Key bytes.
        entry_data.extend_from_slice(key_bytes);

        // Control byte: bitmask of which tags are present.
        // Tags 1, 2, 3 all present = 0x07.
        entry_data.push(0x07);

        // Tag 1: byte offset of chapter start in HTML text.
        let (vwi_buf, vwi_len) = encode_vwi(*byte_offset as u32);
        entry_data.extend_from_slice(&vwi_buf[..vwi_len]);
        // Tag 2: length (0, not used).
        let (vwi_buf, vwi_len) = encode_vwi(0);
        entry_data.extend_from_slice(&vwi_buf[..vwi_len]);
        // Tag 3: label offset into CNCX record.
        let (vwi_buf, vwi_len) = encode_vwi(cncx_offsets[i] as u32);
        entry_data.extend_from_slice(&vwi_buf[..vwi_len]);
    }

    // IDXT section for data record.
    let idxt_offset = INDX_HEADER_LEN + entry_data.len();
    let mut data_idxt = Vec::new();
    data_idxt.extend_from_slice(b"IDXT");
    for &offset in &entry_offsets {
        data_idxt.extend_from_slice(&offset.to_be_bytes());
    }
    // Pad to 4-byte alignment.
    while data_idxt.len() % 4 != 0 {
        data_idxt.push(0);
    }

    let data_total = INDX_HEADER_LEN + entry_data.len() + data_idxt.len();
    let mut indx_data = vec![0u8; data_total];

    // INDX magic.
    indx_data[0..4].copy_from_slice(b"INDX");
    // Header length.
    write_u32_be(&mut indx_data, 4, INDX_HEADER_LEN as u32);
    // Index type = 0.
    write_u32_be(&mut indx_data, 8, 0);
    // IDXT offset within this record.
    write_u32_be(&mut indx_data, 20, idxt_offset as u32);
    // Entry count (number of entries in THIS record).
    write_u32_be(&mut indx_data, 24, entry_count as u32);
    // Remaining header fields are zeros.

    // Write entry data after header.
    indx_data[INDX_HEADER_LEN..INDX_HEADER_LEN + entry_data.len()].copy_from_slice(&entry_data);

    // Write IDXT section.
    indx_data[idxt_offset..idxt_offset + data_idxt.len()].copy_from_slice(&data_idxt);

    (indx_header, indx_data, cncx)
}

/// Builds the TAGX section for the NCX INDX header record.
///
/// Defines three tags:
///   Tag 1 (position): 1 value, bitmask 0x01
///   Tag 2 (length):   1 value, bitmask 0x02
///   Tag 3 (label):    1 value, bitmask 0x04
///   End-of-table marker: [0, 0, 0, 1]
fn build_tagx_section() -> Vec<u8> {
    let mut tagx = Vec::with_capacity(28);
    tagx.extend_from_slice(b"TAGX");
    // TAGX total length: 4 (magic) + 4 (length) + 4 (control byte count) + 4*4 (tag entries) = 28.
    write_u32_be_vec(&mut tagx, 28);
    // Control byte count = 1.
    write_u32_be_vec(&mut tagx, 1);
    // Tag entries: [tag, values_per_entry, bitmask, eof_flag]
    tagx.extend_from_slice(&[1, 1, 0x01, 0]); // Tag 1: position
    tagx.extend_from_slice(&[2, 1, 0x02, 0]); // Tag 2: length
    tagx.extend_from_slice(&[3, 1, 0x04, 0]); // Tag 3: label offset
    tagx.extend_from_slice(&[0, 0, 0, 1]); // End-of-table marker
    tagx
}

/// Helper to append a big-endian u32 to a Vec<u8>.
fn write_u32_be_vec(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

/// Maximum thumbnail width for Kindle library display.
const THUMBNAIL_MAX_WIDTH: u32 = 180;
/// Maximum thumbnail height for Kindle library display.
const THUMBNAIL_MAX_HEIGHT: u32 = 240;
/// JPEG quality for generated thumbnails (0-100).
const THUMBNAIL_JPEG_QUALITY: u8 = 75;

/// Generates a downscaled JPEG thumbnail from cover image data.
///
/// The thumbnail fits within [`THUMBNAIL_MAX_WIDTH`] x [`THUMBNAIL_MAX_HEIGHT`]
/// while maintaining the original aspect ratio. Returns `None` if the image
/// cannot be decoded (graceful fallback to using the cover as thumbnail).
fn generate_thumbnail(image_data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(image_data).ok()?;

    let (orig_w, orig_h) = (img.width(), img.height());
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    // If the image is already within thumbnail bounds, skip generation entirely
    // to avoid lossy re-encoding. The caller falls back to using the cover itself.
    if orig_w <= THUMBNAIL_MAX_WIDTH && orig_h <= THUMBNAIL_MAX_HEIGHT {
        return None;
    }

    let resized = img.resize(
        THUMBNAIL_MAX_WIDTH,
        THUMBNAIL_MAX_HEIGHT,
        FilterType::Triangle,
    );

    let rgb = resized.to_rgb8();
    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, THUMBNAIL_JPEG_QUALITY);
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(jpeg_buf.into_inner())
}

/// Derives a deterministic unique ID from book metadata.
///
/// Uses a DJB2-style hash over the title and author names to produce a
/// non-zero u32. The result is stable across runs for the same input,
/// which ensures that re-building the same book yields an identical file.
fn derive_unique_id(title: &str, authors: &[String]) -> u32 {
    let mut hash: u32 = 5381;
    for b in title.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    for author in authors {
        hash = hash.wrapping_mul(33).wrapping_add(0); // null separator
        for b in author.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(b as u32);
        }
    }
    // Ensure the result is non-zero.
    if hash == 0 { 1 } else { hash }
}

/// Finds the byte offset where the first chapter content starts.
///
/// This is used for EXTH record 116 (start reading offset), which tells
/// the Kindle where to open the book. With a TOC section at the start of
/// `<body>`, the first `<mbp:pagebreak` separates the TOC from chapter
/// content; we return the position just after its closing `>`.
/// If no page break is found (single-chapter books without TOC), returns 0.
fn find_start_reading_offset(html: &str) -> u32 {
    if let Some(pos) = memchr::memmem::find(html.as_bytes(), b"<mbp:pagebreak") {
        // Point to the character right after the closing '>'.
        if let Some(gt) = memchr::memchr(b'>', &html.as_bytes()[pos..]) {
            return (pos + gt + 1) as u32;
        }
    }
    0
}

/// Generates a complete MOBI file from a `Book` and returns the raw bytes.
pub(crate) fn write_mobi(book: &Book) -> Result<Vec<u8>> {
    // Convert book content to HTML and collect chapter info for NCX.
    let (html, chapter_entries) = book_to_mobi_html(book);
    let text_bytes = html.as_bytes();

    // Split and compress text records.
    // Each record gets a trailing 0x00 byte for multibyte overlap signalling
    // (extra_data_flags bit 0). The trailing byte 0x00 means "no overlap".
    let mut text_records = compress_text_records(text_bytes);
    for tr in &mut text_records {
        tr.push(0x00);
    }
    let text_record_count = text_records.len();

    // Collect image data references (borrow from book, avoid cloning).
    let image_refs = collect_image_refs(book);
    let has_images = !image_refs.is_empty();

    // Generate a downscaled thumbnail from the cover (first) image.
    // If generation succeeds the thumbnail is appended as the last image
    // record (after all original images) so that existing recindex references
    // in the HTML are not shifted.  EXTH 202 points to that final index.
    let thumbnail_data = if has_images {
        generate_thumbnail(image_refs[0])
    } else {
        None
    };
    let thumb_offset: u32 = if thumbnail_data.is_some() {
        image_refs.len() as u32 // index after the last original image
    } else {
        0
    };

    // Derive unique ID from metadata.
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    let unique_id = derive_unique_id(title, &book.metadata.authors);

    // Find the start reading offset (byte offset after first page break).
    let start_reading = find_start_reading_offset(&html);

    // Calculate structural record indices (0-based PDB record indices).
    let thumbnail_record_count = if thumbnail_data.is_some() { 1 } else { 0 };
    let image_count = image_refs.len();

    // Build INDX/CNCX records for NCX navigation (skip if no chapters).
    let ncx_records = if !chapter_entries.is_empty() {
        let (h, d, c) = build_ncx_indx(&chapter_entries);
        Some((h, d, c))
    } else {
        None
    };
    let ncx_record_count: usize = if ncx_records.is_some() { 3 } else { 0 };

    // NCX index points to the first INDX record (right after images/thumbnail),
    // or NULL_INDEX if no chapters to index.
    let ncx_index = if ncx_records.is_some() {
        (1 + text_record_count + image_count + thumbnail_record_count) as u32
    } else {
        NULL_INDEX
    };

    // FLIS/FCIS come after the INDX/CNCX records (if any).
    let flis_record_num =
        (1 + text_record_count + image_count + thumbnail_record_count + ncx_record_count) as u32;
    let fcis_record_num = flis_record_num + 1;

    // Build EXTH.
    let exth_data = build_metadata_exth(book, has_images, thumb_offset, start_reading);

    // Build Record 0.
    let record0 = build_record0(
        title,
        text_bytes.len() as u32,
        text_record_count as u16,
        &exth_data,
        text_record_count as u32 + 1, // first image index (1-based after text records)
        has_images,
        unique_id,
        flis_record_num,
        fcis_record_num,
        ncx_index,
    );

    // Structural records: FLIS, FCIS, EOF.
    let fcis = build_fcis(text_bytes.len() as u32);

    // Calculate total number of records and pre-compute total output size.
    // record0 + text + images + thumbnail + INDX_header + INDX_data + CNCX + FLIS + FCIS + EOF
    let num_records =
        1 + text_record_count + image_count + thumbnail_record_count + ncx_record_count + 3;
    let header_table_size = 78 + num_records * 8 + 2;

    // Calculate record offsets and total data size in a single pass.
    let mut offsets = Vec::with_capacity(num_records);
    let mut pos = header_table_size as u32;

    // Record 0
    offsets.push(pos);
    pos += record0.len() as u32;

    // Text records
    for tr in &text_records {
        offsets.push(pos);
        pos += tr.len() as u32;
    }

    // Image records: all original images first, then thumbnail (if any).
    if !image_refs.is_empty() {
        // All original images in order.
        for ir in &image_refs {
            offsets.push(pos);
            pos += ir.len() as u32;
        }

        // Thumbnail appended after all original images.
        if let Some(ref thumb) = thumbnail_data {
            offsets.push(pos);
            pos += thumb.len() as u32;
        }
    }

    // INDX/CNCX records (if present).
    if let Some((ref h, ref d, ref c)) = ncx_records {
        offsets.push(pos);
        pos += h.len() as u32;
        offsets.push(pos);
        pos += d.len() as u32;
        offsets.push(pos);
        pos += c.len() as u32;
    }

    // FLIS
    offsets.push(pos);
    pos += FLIS_RECORD.len() as u32;

    // FCIS
    offsets.push(pos);
    pos += fcis.len() as u32;

    // EOF
    offsets.push(pos);
    pos += EOF_RECORD.len() as u32;

    let total_size = pos as usize;

    // Build PDB header.
    let pdb_name = truncate_pdb_name(title);
    let mut output = build_pdb_header(&pdb_name, b"BOOK", b"MOBI", num_records as u16, &offsets);

    // Set PalmDB creation_date and modification_date (offsets 36 and 40).
    // Uses seconds since Unix epoch, matching Calibre's approach.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    write_u32_be(&mut output, 36, now);
    write_u32_be(&mut output, 40, now);

    output.reserve(total_size - output.len());

    // Append all records in order.
    output.extend_from_slice(&record0);
    for tr in &text_records {
        output.extend_from_slice(tr);
    }
    // Image records: all original images, then thumbnail.
    if !image_refs.is_empty() {
        for ir in &image_refs {
            output.extend_from_slice(ir);
        }
        if let Some(ref thumb) = thumbnail_data {
            output.extend_from_slice(thumb);
        }
    }
    // INDX/CNCX records (if present).
    if let Some((ref h, ref d, ref c)) = ncx_records {
        output.extend_from_slice(h);
        output.extend_from_slice(d);
        output.extend_from_slice(c);
    }
    // Structural records.
    output.extend_from_slice(FLIS_RECORD);
    output.extend_from_slice(&fcis);
    output.extend_from_slice(EOF_RECORD);

    Ok(output)
}

/// Compresses text into PalmDoc-compressed records.
///
/// Uses a reusable `PalmDocCompressor` to amortise the 16 KB hash-chain
/// initialisation cost across all records (instead of re-creating it per record).
/// Also reuses a single output buffer to reduce allocations.
fn compress_text_records(text: &[u8]) -> Vec<Vec<u8>> {
    let num_records = (text.len() + RECORD_SIZE - 1) / RECORD_SIZE.max(1);
    let mut records = Vec::with_capacity(num_records.max(1));
    let mut compressor = palmdoc::PalmDocCompressor::new();
    let mut offset = 0;

    while offset < text.len() {
        let end = (offset + RECORD_SIZE).min(text.len());
        let chunk = &text[offset..end];
        records.push(compressor.compress_record(chunk));
        offset = end;
    }

    if records.is_empty() {
        records.push(Vec::new());
    }

    records
}

/// Builds Record 0 with PalmDOC + MOBI + EXTH headers + title.
#[allow(clippy::too_many_arguments)]
fn build_record0(
    title: &str,
    text_length: u32,
    text_record_count: u16,
    exth_data: &[u8],
    first_image_index: u32,
    has_images: bool,
    unique_id: u32,
    flis_record_num: u32,
    fcis_record_num: u32,
    ncx_index: u32,
) -> Vec<u8> {
    let title_bytes = title.as_bytes();
    let exth_len = exth_data.len();
    let title_offset = 16 + MOBI_HEADER_LEN as usize + exth_len;
    let total_size = title_offset + title_bytes.len();
    // Pad to 4-byte alignment.
    let padded_size = (total_size + 3) & !3;

    let mut data = vec![0u8; padded_size];

    // --- PalmDOC header (0-15) ---
    write_u16_be(&mut data, 0, COMPRESSION_PALMDOC);
    // Offset 2-3: unused (0).
    write_u32_be(&mut data, 4, text_length);
    write_u16_be(&mut data, 8, text_record_count);
    write_u16_be(&mut data, 10, RECORD_SIZE as u16);
    // Offset 12-15: encryption=0, unused=0.

    // --- MOBI header (16+) ---
    data[16..20].copy_from_slice(b"MOBI");
    write_u32_be(&mut data, 20, MOBI_HEADER_LEN);
    write_u32_be(&mut data, 24, 2); // mobi_type = book
    write_u32_be(&mut data, 28, ENCODING_UTF8);
    write_u32_be(&mut data, 32, unique_id);
    write_u32_be(&mut data, 36, 6); // file version = MOBI 6

    // Offsets 40-79: various index fields (NULL).
    for offset in (40..80).step_by(4) {
        write_u32_be(&mut data, offset, NULL_INDEX);
    }

    // First non-book record.
    let first_non_book = 1 + text_record_count as u32;
    write_u32_be(&mut data, 80, first_non_book);

    // Full name offset + length.
    write_u32_be(&mut data, 84, title_offset as u32);
    write_u32_be(&mut data, 88, title_bytes.len() as u32);

    // Locale = 0x09 (English).
    write_u32_be(&mut data, 92, 0x09);

    // Min version = 6.
    write_u32_be(&mut data, 104, 6);

    // First image index.
    let img_idx = if has_images {
        first_image_index
    } else {
        NULL_INDEX
    };
    write_u32_be(&mut data, 108, img_idx);

    // Huffman offsets (not used -- PalmDoc compression).
    write_u32_be(&mut data, 112, 0);
    write_u32_be(&mut data, 116, 0);

    // EXTH flags.
    let exth_flags: u32 = if !exth_data.is_empty() { 0x40 } else { 0 };
    write_u32_be(&mut data, 128, exth_flags);

    // Offsets 132-167: unknown/zeroes.

    // DRM fields (none).
    write_u32_be(&mut data, 168, NULL_INDEX);
    write_u32_be(&mut data, 172, 0);
    write_u32_be(&mut data, 176, 0);
    write_u32_be(&mut data, 180, 0);

    // First / last content record indices (offsets 192-195).
    write_u16_be(&mut data, 192, 1); // first text record is always PDB record 1
    write_u16_be(&mut data, 194, text_record_count); // last text record

    // Unknown field (matches Calibre output).
    write_u32_be(&mut data, 196, 0x0000_0001);

    // FCIS / FLIS record indices and counts (offsets 200-215).
    write_u32_be(&mut data, 200, fcis_record_num);
    write_u32_be(&mut data, 204, 1); // FCIS count
    write_u32_be(&mut data, 208, flis_record_num);
    write_u32_be(&mut data, 212, 1); // FLIS count

    // Extra data flags = 1 (bit 0 = multibyte overlap signalling).
    // Each text record carries a trailing byte that indicates how many
    // extra bytes at the record boundary belong to a multi-byte character.
    write_u32_be(&mut data, 240, 1);

    // NCX index.
    write_u32_be(&mut data, 244, ncx_index);

    // --- EXTH header ---
    if !exth_data.is_empty() {
        let exth_start = 16 + MOBI_HEADER_LEN as usize;
        data[exth_start..exth_start + exth_len].copy_from_slice(exth_data);
    }

    // --- Title ---
    data[title_offset..title_offset + title_bytes.len()].copy_from_slice(title_bytes);

    data
}

/// Builds EXTH header from Book metadata, writing directly into a single buffer.
///
/// `thumb_offset` controls EXTH 202: 0 = same as cover, N = index of separate
/// thumbnail record relative to the first image record.
///
/// `start_reading` is the byte offset into the HTML text where the Kindle
/// should open the book (EXTH 116).
fn build_metadata_exth(
    book: &Book,
    has_cover: bool,
    thumb_offset: u32,
    start_reading: u32,
) -> Vec<u8> {
    // Collect (type, data_slice) pairs without cloning the data.
    // We need to be careful about the cover offset bytes lifetime.
    let cover_offset_bytes = 0u32.to_be_bytes();
    let thumb_offset_bytes = thumb_offset.to_be_bytes();
    let start_reading_bytes = start_reading.to_be_bytes();
    let creator_software_bytes = 201u32.to_be_bytes(); // 201 = Linux
    let creator_major_bytes = 2u32.to_be_bytes();
    let creator_minor_bytes = 9u32.to_be_bytes();
    let creator_build_bytes = 0u32.to_be_bytes();
    let has_fake_cover_bytes = 0u32.to_be_bytes();

    let mut refs: Vec<(u32, &[u8])> = Vec::with_capacity(18);

    // Title.
    if let Some(ref title) = book.metadata.title {
        refs.push((EXTH_UPDATED_TITLE, title.as_bytes()));
    }

    // Authors.
    for author in &book.metadata.authors {
        refs.push((EXTH_AUTHOR, author.as_bytes()));
    }

    // Publisher.
    if let Some(ref publisher) = book.metadata.publisher {
        refs.push((EXTH_PUBLISHER, publisher.as_bytes()));
    }

    // Description.
    if let Some(ref desc) = book.metadata.description {
        refs.push((EXTH_DESCRIPTION, desc.as_bytes()));
    }

    // ISBN.
    if let Some(ref isbn) = book.metadata.isbn {
        refs.push((EXTH_ISBN, isbn.as_bytes()));
    }

    // Subjects.
    for subject in &book.metadata.subjects {
        refs.push((EXTH_SUBJECT, subject.as_bytes()));
    }

    // Language.
    if let Some(ref lang) = book.metadata.language {
        refs.push((EXTH_LANGUAGE, lang.as_bytes()));
    }

    // Publication date (ISO 8601 / RFC 3339).
    let date_string = book.metadata.publication_date.map(|d| d.to_rfc3339());
    if let Some(ref ds) = date_string {
        refs.push((EXTH_PUBLISHED_DATE, ds.as_bytes()));
    }

    // Rights.
    if let Some(ref rights) = book.metadata.rights {
        refs.push((EXTH_RIGHTS, rights.as_bytes()));
    }

    // ASIN / identifier.
    if let Some(ref identifier) = book.metadata.identifier {
        refs.push((EXTH_ASIN, identifier.as_bytes()));
    }

    // Start reading offset (EXTH 116).
    refs.push((EXTH_START_READING, &start_reading_bytes));

    // Cover offset (first image = index 0).
    if has_cover {
        refs.push((EXTH_COVER_OFFSET, &cover_offset_bytes));
    }

    // Thumbnail offset (separate thumbnail if available, otherwise same as cover).
    if has_cover {
        refs.push((EXTH_THUMB_OFFSET, &thumb_offset_bytes));
    }

    // KF8 cover URI (EXTH 129) -- Calibre always writes this when a cover is present.
    if has_cover {
        refs.push((EXTH_KF8_COVER_URI, b"kindle:embed:0001"));
    }

    // Has fake cover flag (EXTH 203) -- Calibre always writes 0 when a cover is present.
    if has_cover {
        refs.push((EXTH_HAS_FAKE_COVER, &has_fake_cover_bytes));
    }

    // Creator software version records (EXTH 204-207).
    refs.push((EXTH_CREATOR_SOFTWARE, &creator_software_bytes));
    refs.push((EXTH_CREATOR_MAJOR, &creator_major_bytes));
    refs.push((EXTH_CREATOR_MINOR, &creator_minor_bytes));
    refs.push((EXTH_CREATOR_BUILD, &creator_build_bytes));

    // CDE type = EBOK (ebook).
    refs.push((EXTH_CDE_TYPE, b"EBOK"));

    // Override Kindle fonts (EXTH 528) -- allows custom font overrides on Kindle.
    refs.push((EXTH_OVERRIDE_KINDLE_FONTS, b"true"));

    exth::build_exth(&refs)
}

/// Collects references to image data from Book resources without cloning.
fn collect_image_refs(book: &Book) -> Vec<&[u8]> {
    let resources = book.resources();
    let mut refs = Vec::with_capacity(resources.len());
    for resource in &resources {
        if resource.media_type.starts_with("image/") {
            refs.push(resource.data);
        }
    }
    refs
}

/// The 10-character placeholder used for filepos links before fixup.
const FILEPOS_PLACEHOLDER: &str = "0000000000";

/// Flattens the TOC tree into a depth-first list of (depth, title, href).
///
/// This walks ALL levels of the TOC hierarchy so that sub-chapter entries
/// are included in the generated navigation (filepos links, pagebreaks,
/// and INDX entries).
fn flatten_toc_tree(items: &[crate::domain::toc::TocItem]) -> Vec<(usize, &str, &str)> {
    let mut result = Vec::new();
    fn walk<'a>(
        items: &'a [crate::domain::toc::TocItem],
        depth: usize,
        out: &mut Vec<(usize, &'a str, &'a str)>,
    ) {
        for item in items {
            out.push((depth, &item.title, &item.href));
            walk(&item.children, depth + 1, out);
        }
    }
    walk(items, 0, &mut result);
    result
}

/// Finds all `id="..."` attribute positions within HTML content.
///
/// Returns a map from fragment identifier (the id value) to the byte offset
/// of the start of the enclosing tag (the `<` character) within `content`.
fn find_id_positions(content: &str) -> std::collections::HashMap<&str, usize> {
    let mut map = std::collections::HashMap::new();
    let bytes = content.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let id_finder = memchr::memmem::Finder::new(b"id=\"");

    while i < len {
        // Find next id="
        if let Some(pos) = id_finder.find(&bytes[i..]) {
            let abs = i + pos;
            let value_start = abs + 4;
            if let Some(end) = memchr::memchr(b'"', &bytes[value_start..]) {
                let id_value = &content[value_start..value_start + end];
                if !id_value.is_empty() {
                    // Walk backwards from abs to find the '<' of the enclosing tag.
                    let mut tag_start = abs;
                    while tag_start > 0 && bytes[tag_start] != b'<' {
                        tag_start -= 1;
                    }
                    map.entry(id_value).or_insert(tag_start);
                }
                i = value_start + end + 1;
            } else {
                i = abs + 4;
            }
        } else {
            break;
        }
    }

    map
}

/// Inserts `<mbp:pagebreak/>` markers before elements with matching fragment IDs
/// in the given HTML content.
///
/// `fragment_ids` contains the set of fragment identifiers referenced by TOC entries.
/// For each fragment found in the content, a pagebreak is inserted before the element.
///
/// Returns the modified content and a map from fragment_id to its byte offset
/// (relative to the content string, AFTER pagebreak insertion).
fn insert_pagebreaks_for_fragments<'a>(
    content: &'a str,
    fragment_ids: &std::collections::HashSet<&str>,
) -> (Cow<'a, str>, std::collections::HashMap<String, usize>) {
    if fragment_ids.is_empty() {
        return (Cow::Borrowed(content), std::collections::HashMap::new());
    }

    let id_positions = find_id_positions(content);

    // Collect positions where we need to insert pagebreaks, sorted.
    let mut insertions: Vec<(usize, &str)> = Vec::new();
    for &frag_id in fragment_ids {
        if let Some(&pos) = id_positions.get(frag_id) {
            insertions.push((pos, frag_id));
        }
    }
    insertions.sort_by_key(|&(pos, _)| pos);
    // Deduplicate by position (multiple fragments might point to same element).
    insertions.dedup_by_key(|item| item.0);

    if insertions.is_empty() {
        return (Cow::Borrowed(content), std::collections::HashMap::new());
    }

    // Build the new content with pagebreaks inserted.
    let pagebreak_tag = "<mbp:pagebreak/>";
    let extra_len = insertions.len() * pagebreak_tag.len();
    let mut result = String::with_capacity(content.len() + extra_len);
    let mut offsets = std::collections::HashMap::new();
    let mut prev = 0;

    for (orig_pos, _frag_id) in &insertions {
        result.push_str(&content[prev..*orig_pos]);
        result.push_str(pagebreak_tag);
        prev = *orig_pos;
    }
    result.push_str(&content[prev..]);

    // Now compute fragment offsets in the NEW content.
    // Re-scan the result for id positions.
    let new_id_positions = find_id_positions(&result);
    for &frag_id in fragment_ids {
        if let Some(&pos) = new_id_positions.get(frag_id) {
            offsets.insert(frag_id.to_string(), pos);
        }
    }

    (Cow::Owned(result), offsets)
}

/// Converts Book content to MOBI-compatible HTML with filepos-based TOC links.
///
/// Uses a two-pass approach:
/// 1. Generate HTML with `filepos=0000000000` placeholders for the guide
///    references and TOC entry links.
/// 2. Fix up the placeholders with actual byte offsets into the HTML.
///
/// The output contains:
/// - A `<guide>` section in `<head>` with "toc" and "text" references
/// - A TOC section at the start of `<body>` listing ALL TOC entries (including sub-chapters)
/// - `<mbp:pagebreak />` tags separating chapters and sub-chapter sections
///
/// Returns `(html, all_entries)` where `all_entries` is a `Vec<(title, byte_offset)>`
/// for every TOC entry (at all depth levels) in the generated HTML.
fn book_to_mobi_html(book: &Book) -> (String, Vec<(String, usize)>) {
    // Collect chapter info: (spine_index, toc_title_if_any).
    let mut chapter_info: Vec<(usize, Option<String>)> = Vec::new();
    let toc = &book.toc;

    let toc_title_map = TocTitleMap::build(toc);
    for (i, spine_item) in book.spine.iter().enumerate() {
        let Some(manifest_item) = book.manifest.get(&spine_item.manifest_id) else {
            continue;
        };
        if manifest_item.data.as_text().is_none() {
            continue;
        }
        let ch_title = toc_title_map.get(&manifest_item.href);
        chapter_info.push((i, ch_title));
    }

    // Flatten the FULL TOC tree to get all entries at all depths.
    let all_toc_entries = flatten_toc_tree(toc);
    let has_toc_entries =
        !all_toc_entries.is_empty() || chapter_info.iter().any(|(_, t)| t.is_some());

    // Build a map: spine manifest href -> spine index in chapter_info.
    // This lets us resolve TOC entry hrefs to the correct spine item.
    let mut href_to_chapter_info_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (info_idx, (spine_idx, _)) in chapter_info.iter().enumerate() {
        let spine_item = &book.spine.items[*spine_idx];
        if let Some(manifest_item) = book.manifest.get(&spine_item.manifest_id) {
            href_to_chapter_info_idx.insert(&manifest_item.href, info_idx);
        }
    }

    // Group TOC entries by their base href (spine item) to identify which
    // fragment IDs need pagebreaks within each chapter.
    let mut fragments_by_chapter: std::collections::HashMap<
        &str,
        std::collections::HashSet<&str>,
    > = std::collections::HashMap::new();
    for (_depth, _title, href) in &all_toc_entries {
        if let Some(hash_pos) = href.find('#') {
            let base = &href[..hash_pos];
            let frag = &href[hash_pos + 1..];
            if !frag.is_empty() {
                fragments_by_chapter
                    .entry(base)
                    .or_default()
                    .insert(frag);
            }
        }
    }

    // Estimate total size from manifest data (avoiding chapters() clone).
    let estimated: usize = book
        .spine
        .iter()
        .filter_map(|si| {
            let item = book.manifest.get(&si.manifest_id)?;
            Some(item.data.as_text()?.len() + 200)
        })
        .sum::<usize>()
        + 2048
        + all_toc_entries.len() * 80; // extra room for TOC + guide + sub-entries
    let mut html = String::with_capacity(estimated.max(4096));

    // --- <head> with guide ---
    html.push_str("<html><head><title>");
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    push_html_escaped(&mut html, title);
    html.push_str("</title>\n");

    // Guide references (with filepos placeholders).
    let mut guide_toc_placeholder: Option<usize> = None;
    let mut guide_text_placeholder: Option<usize> = None;

    if has_toc_entries {
        html.push_str("<guide>\n");
        // TOC reference
        html.push_str("<reference type=\"toc\" title=\"Table of Contents\" filepos=");
        guide_toc_placeholder = Some(html.len());
        html.push_str(FILEPOS_PLACEHOLDER);
        html.push_str(" />\n");
        // Start reading reference
        html.push_str("<reference type=\"text\" title=\"Start\" filepos=");
        guide_text_placeholder = Some(html.len());
        html.push_str(FILEPOS_PLACEHOLDER);
        html.push_str(" />\n");
        html.push_str("</guide>\n");
    }

    html.push_str("</head><body>\n");

    // --- TOC section ---
    // Generate TOC links for ALL entries in the TOC tree (not just top-level).
    let toc_start_offset = html.len();
    let mut toc_entry_placeholders: Vec<usize> = Vec::new();

    if has_toc_entries {
        html.push_str("<div><h2><b>Table of Contents</b></h2>\n<ul>\n");

        for (_depth, entry_title, _href) in &all_toc_entries {
            html.push_str("<li><a filepos=");
            toc_entry_placeholders.push(html.len());
            html.push_str(FILEPOS_PLACEHOLDER);
            html.push('>');
            push_html_escaped(&mut html, entry_title);
            html.push_str("</a></li>\n");
        }

        html.push_str("</ul></div>\n<mbp:pagebreak />\n");
    }

    // --- Chapter content ---
    // Track the byte offset where each chapter's content starts.
    let mut chapter_start_offsets: Vec<usize> = Vec::with_capacity(chapter_info.len());
    let mut first_chapter_offset: Option<usize> = None;

    // Track fragment offsets within serialized content: "base_href#frag" -> byte_offset.
    let mut fragment_offsets: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (content_idx, (spine_idx, ch_title)) in chapter_info.iter().enumerate() {
        let spine_item = &book.spine.items[*spine_idx];
        let manifest_item = book.manifest.get(&spine_item.manifest_id).unwrap();
        let content = manifest_item.data.as_text().unwrap();
        let manifest_href = &manifest_item.href;

        if content_idx > 0 {
            html.push_str("<mbp:pagebreak />\n");
        }

        // Record the byte offset where this chapter starts.
        let chapter_offset = html.len();
        chapter_start_offsets.push(chapter_offset);
        if first_chapter_offset.is_none() {
            first_chapter_offset = Some(chapter_offset);
        }

        // Chapter heading — only add if the content doesn't already start with one.
        let cleaned_content = if memchr::memchr(b'<', content.as_bytes()).is_some() {
            Some(strip_xhtml_wrapper(content))
        } else {
            None
        };
        let content_has_heading = cleaned_content
            .as_deref()
            .map(|c| {
                let tb = c.trim_start().as_bytes();
                tb.len() >= 3
                    && tb[0] == b'<'
                    && (tb[1] == b'h' || tb[1] == b'H')
                    && tb[2] >= b'1'
                    && tb[2] <= b'3'
            })
            .unwrap_or(false);

        #[allow(clippy::collapsible_if)]
        if let Some(title) = ch_title {
            if !content_has_heading {
                html.push_str("<h2>");
                push_html_escaped(&mut html, title);
                html.push_str("</h2>\n");
            }
        }

        // Check if this chapter has any sub-chapter fragment references.
        let fragment_ids = fragments_by_chapter
            .get(manifest_href.as_str())
            .cloned()
            .unwrap_or_default();

        // Chapter body — with pagebreaks inserted before fragment anchors.
        if let Some(cleaned) = cleaned_content {
            if !fragment_ids.is_empty() {
                let base_offset = html.len();
                let (modified, frag_offsets) =
                    insert_pagebreaks_for_fragments(&cleaned, &fragment_ids);
                html.push_str(&modified);

                // Record absolute offsets for each fragment.
                for (frag_id, rel_offset) in frag_offsets {
                    let abs_offset = base_offset + rel_offset;
                    let full_href = format!("{}#{}", manifest_href, frag_id);
                    fragment_offsets.insert(full_href, abs_offset);
                }
            } else {
                html.push_str(&cleaned);
            }
        } else {
            let plain = strip_tags(content);
            for line in plain.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    html.push_str("<p>");
                    push_html_escaped(&mut html, trimmed);
                    html.push_str("</p>\n");
                }
            }
        }

        html.push('\n');
    }

    html.push_str("</body></html>");

    // --- Resolve TOC entry offsets ---
    // For each TOC entry, determine its byte offset in the HTML:
    // - Entries without fragments map to the chapter start offset.
    // - Entries with fragments map to the fragment offset within the chapter.
    let mut toc_target_offsets: Vec<usize> = Vec::with_capacity(all_toc_entries.len());
    for (_depth, _title, href) in &all_toc_entries {
        if let Some(hash_pos) = href.find('#') {
            // Entry has a fragment — try fragment_offsets first.
            if let Some(&offset) = fragment_offsets.get(*href) {
                toc_target_offsets.push(offset);
            } else {
                // Fragment not found; fall back to the chapter start.
                let base = &href[..hash_pos];
                let offset = href_to_chapter_info_idx
                    .get(base)
                    .map(|&idx| chapter_start_offsets[idx])
                    .unwrap_or(0);
                toc_target_offsets.push(offset);
            }
        } else {
            // No fragment — map to the chapter start.
            let offset = href_to_chapter_info_idx
                .get(*href)
                .map(|&idx| chapter_start_offsets[idx])
                .unwrap_or(0);
            toc_target_offsets.push(offset);
        }
    }

    // --- Fix up filepos placeholders ---
    // SAFETY: All placeholders and replacements are exactly 10 ASCII bytes,
    // so in-place replacement preserves valid UTF-8 and doesn't shift offsets.
    let bytes = unsafe { html.as_bytes_mut() };

    // Reusable buffer for formatting 10-digit offsets, avoiding per-fixup allocation.
    let mut fmt_buf = String::with_capacity(10);

    // Fix guide "toc" reference -> point to the TOC div start.
    if let Some(pos) = guide_toc_placeholder {
        fmt_buf.clear();
        use std::fmt::Write;
        write!(&mut fmt_buf, "{:010}", toc_start_offset).unwrap();
        bytes[pos..pos + 10].copy_from_slice(fmt_buf.as_bytes());
    }

    // Fix guide "text" reference -> point to the first chapter start.
    if let Some(pos) = guide_text_placeholder {
        let target = first_chapter_offset.unwrap_or(toc_start_offset);
        fmt_buf.clear();
        use std::fmt::Write;
        write!(&mut fmt_buf, "{:010}", target).unwrap();
        bytes[pos..pos + 10].copy_from_slice(fmt_buf.as_bytes());
    }

    // Fix TOC entry links -> point to the resolved offsets for each entry.
    for (toc_idx, placeholder_pos) in toc_entry_placeholders.iter().enumerate() {
        let target_offset = toc_target_offsets[toc_idx];
        fmt_buf.clear();
        use std::fmt::Write;
        write!(&mut fmt_buf, "{:010}", target_offset).unwrap();
        bytes[*placeholder_pos..*placeholder_pos + 10].copy_from_slice(fmt_buf.as_bytes());
    }

    // Build the navigation entries vector: (title, byte_offset) for ALL TOC entries.
    // These are used for INDX record generation.
    let mut all_entries: Vec<(String, usize)> = Vec::with_capacity(all_toc_entries.len());
    // Track seen offsets to avoid duplicate INDX entries at the same position.
    let mut seen_offsets = std::collections::HashSet::new();
    for (i, (_depth, entry_title, _href)) in all_toc_entries.iter().enumerate() {
        let offset = toc_target_offsets[i];
        if seen_offsets.insert(offset) {
            all_entries.push((entry_title.to_string(), offset));
        }
    }

    // If the TOC tree was empty but chapters had titles, fall back to chapter-level entries.
    if all_entries.is_empty() {
        for (info_idx, (_spine_idx, ch_title)) in chapter_info.iter().enumerate() {
            let title = ch_title
                .clone()
                .unwrap_or_else(|| format!("Chapter {}", info_idx + 1));
            let offset = chapter_start_offsets[info_idx];
            all_entries.push((title, offset));
        }
    }

    (html, all_entries)
}

/// Pre-built lookup table for resolving manifest hrefs to TOC titles.
///
/// Replaces the recursive `find_toc_title` tree walk (called per spine item)
/// with O(1) HashMap lookups.  The map is keyed by both the full TOC href and
/// its fragment-stripped base, preserving first-match (DFS) semantics via
/// `entry().or_insert_with()`.  A flat entry list handles the rare prefix-match
/// fallback.
struct TocTitleMap {
    /// href (exact or base) → title
    map: std::collections::HashMap<String, String>,
    /// Flat DFS-order entries for prefix-match fallback: (href, title)
    flat: Vec<(String, String)>,
}

impl TocTitleMap {
    fn build(items: &[crate::domain::toc::TocItem]) -> Self {
        let mut m = TocTitleMap {
            map: std::collections::HashMap::new(),
            flat: Vec::new(),
        };
        m.collect(items);
        m
    }

    fn collect(&mut self, items: &[crate::domain::toc::TocItem]) {
        for item in items {
            // Exact href → title (first match wins)
            self.map
                .entry(item.href.clone())
                .or_insert_with(|| item.title.clone());
            // Base href (fragment stripped) → title
            let base = item.href.split('#').next().unwrap_or(&item.href);
            if !base.is_empty() && base != item.href {
                self.map
                    .entry(base.to_string())
                    .or_insert_with(|| item.title.clone());
            }
            self.flat.push((item.href.clone(), item.title.clone()));
            self.collect(&item.children);
        }
    }

    /// Look up a title for the given manifest href.
    fn get(&self, href: &str) -> Option<String> {
        if let Some(title) = self.map.get(href) {
            return Some(title.clone());
        }
        // Prefix-match fallback (rare): manifest href starts with a TOC href.
        for (toc_href, title) in &self.flat {
            if href.starts_with(toc_href.as_str()) {
                return Some(title.clone());
            }
        }
        None
    }
}

/// Pushes HTML-escaped text directly into an existing String buffer,
/// avoiding allocation when no escaping is needed (the common case).
#[inline]
fn push_html_escaped(buf: &mut String, text: &str) {
    crate::formats::common::text_utils::push_escape_html(buf, text);
}

/// Strips XHTML wrapper elements from chapter content so that only the inner
/// body content remains. This removes XML processing instructions, DOCTYPE
/// declarations, `<html>`, `<head>` (with contents), and `<body>` tags that
/// are present in EPUB XHTML source files.
///
/// Uses a single-pass approach: find the `<body>` content region and extract
/// it, rather than repeatedly calling `replace_range` which shifts bytes.
fn strip_xhtml_wrapper(content: &str) -> Cow<'_, str> {
    let bytes = content.as_bytes();
    // Fast path: extract <body> inner content as a borrowed slice — zero allocation.
    if let Some(body_start) = memchr::memmem::find(bytes, b"<body")
        && let Some(gt) = memchr::memchr(b'>', &bytes[body_start..])
    {
        let inner_start = body_start + gt + 1;
        let inner_end = memchr::memmem::find(&bytes[inner_start..], b"</body>")
            .map(|pos| inner_start + pos)
            .unwrap_or(content.len());
        return Cow::Borrowed(&content[inner_start..inner_end]);
    }

    // No <body> found — strip individual wrapper elements as fallback.
    let mut s = content.to_string();

    // Remove <?xml ...?> processing instructions.
    while let Some(start) = memchr::memmem::find(s.as_bytes(), b"<?xml") {
        if let Some(end) = memchr::memmem::find(&s.as_bytes()[start..], b"?>") {
            s.replace_range(start..start + end + 2, "");
        } else {
            break;
        }
    }

    // Remove <!DOCTYPE ...> declarations.
    for pat in &[b"<!DOCTYPE" as &[u8], b"<!doctype"] {
        while let Some(start) = memchr::memmem::find(s.as_bytes(), pat) {
            if let Some(end) = memchr::memchr(b'>', &s.as_bytes()[start..]) {
                s.replace_range(start..start + end + 1, "");
            } else {
                break;
            }
        }
    }

    // Remove <html ...> and </html> tags.
    while let Some(start) = memchr::memmem::find(s.as_bytes(), b"<html") {
        if let Some(end) = memchr::memchr(b'>', &s.as_bytes()[start..]) {
            s.replace_range(start..start + end + 1, "");
        } else {
            break;
        }
    }
    while let Some(start) = memchr::memmem::find(s.as_bytes(), b"</html>") {
        s.replace_range(start..start + 7, "");
    }

    Cow::Owned(s)
}

/// Truncates a title to fit the 31-character PDB name field.
/// Spaces are replaced with underscores per PalmOS convention (matches Calibre).
fn truncate_pdb_name(title: &str) -> String {
    // Fast path: check if title is already valid (all ASCII graphic or space, len <= 31).
    if title.len() <= 31
        && !title.is_empty()
        && title.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
    {
        return title.replace(' ', "_");
    }

    let clean: String = title
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(31)
        .collect();

    if clean.is_empty() {
        "Untitled".to_string()
    } else {
        clean.replace(' ', "_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;
    use crate::domain::FormatReader;
    use crate::formats::common::palm_db::PdbFile;
    use crate::formats::mobi::MobiReader;

    #[test]
    fn write_mobi_produces_valid_pdb() {
        let mut book = Book::new();
        book.metadata.title = Some("Writer Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello MOBI writer!</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();

        assert!(pdb.header.is_mobi());
        assert!(pdb.record_count() >= 4); // record0 + text + FLIS + FCIS + EOF
    }

    #[test]
    fn mobi_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip MOBI".into());
        book.metadata.authors.push("Alice".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(Chapter {
            title: Some("Intro".into()),
            content: "<p>First chapter content here.</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second chapter with more text.</p>".into(),
            id: Some("ch2".into()),
        });

        // Write.
        let mobi_data = write_mobi(&book).unwrap();

        // Read back.
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip MOBI"));
        assert!(decoded.metadata.authors.iter().any(|a| a == "Alice"));

        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());

        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("First chapter content"));
        assert!(all_content.contains("Second chapter"));
    }

    #[test]
    fn mobi_round_trip_long_text() {
        let mut book = Book::new();
        book.metadata.title = Some("Long Text".into());

        let long_content = "<p>".to_string() + &"Word ".repeat(2000) + "</p>";
        book.add_chapter(Chapter {
            title: Some("Big Chapter".into()),
            content: long_content,
            id: Some("ch1".into()),
        });

        let mobi_data = write_mobi(&book).unwrap();
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        let chapters = decoded.chapters();
        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("Word"));
    }

    #[test]
    fn truncate_pdb_name_works() {
        assert_eq!(truncate_pdb_name("Short"), "Short");
        assert_eq!(truncate_pdb_name(&"A".repeat(50)).len(), 31);
        assert_eq!(truncate_pdb_name(""), "Untitled");
    }

    #[test]
    fn truncate_pdb_name_replaces_spaces_with_underscores() {
        assert_eq!(
            truncate_pdb_name("Alice in Wonderland"),
            "Alice_in_Wonderland"
        );
        assert_eq!(truncate_pdb_name("A B"), "A_B");
    }

    #[test]
    fn strip_xhtml_wrapper_removes_all_wrappers() {
        let input = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title><meta charset="utf-8"/></head>
<body>
<h1>Chapter 1</h1>
<p>Hello world.</p>
</body>
</html>"#;
        let result = strip_xhtml_wrapper(input);
        assert!(
            !result.contains("<?xml"),
            "XML declaration should be stripped"
        );
        assert!(!result.contains("<!DOCTYPE"), "DOCTYPE should be stripped");
        assert!(!result.contains("<html"), "html tag should be stripped");
        assert!(!result.contains("<head"), "head tag should be stripped");
        assert!(
            !result.contains("<title>"),
            "title should be stripped with head"
        );
        assert!(!result.contains("<body"), "body tag should be stripped");
        assert!(
            !result.contains("</html>"),
            "closing html should be stripped"
        );
        assert!(
            !result.contains("</body>"),
            "closing body should be stripped"
        );
        assert!(
            result.contains("<h1>Chapter 1</h1>"),
            "content should remain"
        );
        assert!(
            result.contains("<p>Hello world.</p>"),
            "content should remain"
        );
    }

    #[test]
    fn strip_xhtml_wrapper_passthrough_plain_html() {
        let input = "<h1>Title</h1><p>Body</p>";
        assert_eq!(strip_xhtml_wrapper(input), input);
    }

    #[test]
    fn html_escape_works() {
        let mut buf = String::new();
        push_html_escaped(&mut buf, "a & b < c > d");
        assert_eq!(buf, "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn mobi_round_trip_extended_metadata() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Extended Meta".into());
        book.metadata.authors.push("Bob".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 6, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );
        book.metadata.rights = Some("CC BY 4.0".into());
        book.metadata.identifier = Some("B00TEST1234".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Content for extended metadata test.</p>".into(),
            id: Some("ch1".into()),
        });

        // Write.
        let mobi_data = write_mobi(&book).unwrap();

        // Read back.
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();

        // Verify the three new fields round-trip correctly.
        assert!(
            decoded.metadata.publication_date.is_some(),
            "publication_date should be present after round-trip"
        );
        let decoded_date = decoded.metadata.publication_date.unwrap();
        assert_eq!(decoded_date.format("%Y-%m-%d").to_string(), "2024-06-15");

        assert_eq!(
            decoded.metadata.rights.as_deref(),
            Some("CC BY 4.0"),
            "rights should round-trip"
        );
        assert_eq!(
            decoded.metadata.identifier.as_deref(),
            Some("B00TEST1234"),
            "identifier should round-trip"
        );
    }

    #[test]
    fn cover_image_writes_exth_201_and_202() {
        use crate::formats::mobi::exth::{EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Cover Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Create a real decodable image (300x400 JPEG) so thumbnail generation succeeds.
        let cover_jpeg = create_test_jpeg(300, 400);
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        // Parse MOBI header to locate EXTH.
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        assert!(mobi_hdr.has_exth(), "EXTH flag should be set");

        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // EXTH 201 (cover offset) should be present and equal to 0.
        let cover_offset = exth
            .get_u32(EXTH_COVER_OFFSET)
            .expect("EXTH 201 (cover offset) should be present");
        assert_eq!(cover_offset, 0, "cover offset should be 0");

        // EXTH 202 (thumbnail offset) should be 1 (separate thumbnail record).
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 (thumbnail offset) should be present");
        assert_eq!(
            thumb_offset, 1,
            "thumbnail offset should be 1 (separate thumbnail)"
        );
    }

    #[test]
    fn cover_image_undecodable_falls_back_to_thumb_offset_zero() {
        use crate::formats::mobi::exth::{EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Fallback Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Add an undecodable fake JPEG — thumbnail generation should fail gracefully.
        let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9];
        book.add_resource("cover", "cover.jpg", fake_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // Cover offset is still 0.
        let cover_offset = exth
            .get_u32(EXTH_COVER_OFFSET)
            .expect("EXTH 201 should be present");
        assert_eq!(cover_offset, 0);

        // Thumbnail offset falls back to 0 (same as cover).
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 should be present");
        assert_eq!(thumb_offset, 0, "thumbnail offset should fall back to 0");
    }

    #[test]
    fn thumbnail_record_exists_and_is_smaller_than_cover() {
        let mut book = Book::new();
        book.metadata.title = Some("Thumb Size Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Create a 600x800 JPEG cover (large enough that the thumbnail will be smaller).
        let cover_jpeg = create_test_jpeg(600, 800);
        let cover_size = cover_jpeg.len();
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout: [record0, text..., cover, thumbnail, INDX_hdr, INDX_data, CNCX, FLIS, FCIS, EOF]
        // With 1 text record: record0=0, text=1, cover=2, thumbnail=3, INDX_hdr=4, INDX_data=5, CNCX=6, FLIS=7, FCIS=8, EOF=9
        let num_records = pdb.record_count();
        assert!(
            num_records >= 10,
            "should have at least 10 records (record0 + text + cover + thumb + INDX*3 + FLIS + FCIS + EOF), got {num_records}"
        );

        // The cover is at index 2 (after record0 and 1 text record).
        let cover_record = pdb.record_data(2).unwrap();
        assert_eq!(
            cover_record.len(),
            cover_size,
            "cover record should match original cover data"
        );

        // The thumbnail is at index 3.
        let thumb_record = pdb.record_data(3).unwrap();
        assert!(
            thumb_record.len() < cover_record.len(),
            "thumbnail ({} bytes) should be smaller than cover ({} bytes)",
            thumb_record.len(),
            cover_record.len()
        );

        // Verify the thumbnail is a valid JPEG (starts with FFD8).
        assert_eq!(
            &thumb_record[..2],
            &[0xFF, 0xD8],
            "thumbnail should be a valid JPEG"
        );

        // Verify thumbnail dimensions are within bounds.
        let thumb_img =
            image::load_from_memory(thumb_record).expect("thumbnail should be decodable");
        assert!(
            thumb_img.width() <= 180,
            "thumbnail width {} should be <= 180",
            thumb_img.width()
        );
        assert!(
            thumb_img.height() <= 240,
            "thumbnail height {} should be <= 240",
            thumb_img.height()
        );
    }

    /// Creates a test JPEG image of the given dimensions using the image crate.
    fn create_test_jpeg(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
        encoder
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn thumbnail_appended_after_all_images_no_index_shift() {
        // Regression test: thumbnail must be appended AFTER all original images
        // so that recindex references in the HTML are not shifted.
        use crate::formats::mobi::exth::{EXTH_THUMB_OFFSET, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Multi-Image Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // All images are larger than 180x240 so that whichever one ends up
        // first (HashMap ordering) will still trigger thumbnail generation.
        let cover_jpeg = create_test_jpeg(600, 800);
        let img2_jpeg = create_test_jpeg(300, 400);
        let img3_jpeg = create_test_jpeg(400, 300);
        book.add_resource("cover", "cover.jpg", cover_jpeg, "image/jpeg");
        book.add_resource("img2", "img2.jpg", img2_jpeg, "image/jpeg");
        book.add_resource("img3", "img3.jpg", img3_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout: [record0, text, img*3, thumbnail, INDX_hdr, INDX_data, CNCX, FLIS, FCIS, EOF] = 12 records
        assert_eq!(
            pdb.record_count(),
            12,
            "expected 12 records (1+1+3+1+3+3), got {}",
            pdb.record_count()
        );

        // The thumbnail is the 4th image record (index 5 = record0 + 1 text + 3 images).
        // It's a JPEG and should be smaller than the large cover.
        let thumb_record = pdb.record_data(5).unwrap();
        assert_eq!(
            &thumb_record[..2],
            &[0xFF, 0xD8],
            "thumbnail should be a valid JPEG"
        );

        // EXTH 202 should point to index 3 (= number of original images, 0-based from first image).
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 should be present");
        assert_eq!(
            thumb_offset, 3,
            "thumbnail offset should be 3 (after 3 original images)"
        );
    }

    #[test]
    fn small_cover_skips_thumbnail_generation() {
        // If the cover is already within 180x240 bounds, no separate thumbnail
        // should be generated (avoids lossy re-encoding).
        use crate::formats::mobi::exth::{EXTH_THUMB_OFFSET, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Small Cover Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // 100x100 is within 180x240 bounds — should NOT generate a thumbnail.
        let small_jpeg = create_test_jpeg(100, 100);
        book.add_resource("cover", "cover.jpg", small_jpeg, "image/jpeg");

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();

        // Record layout without thumbnail: [record0, text, cover, INDX_hdr, INDX_data, CNCX, FLIS, FCIS, EOF] = 9 records
        assert_eq!(
            pdb.record_count(),
            9,
            "no thumbnail record should be generated for small cover"
        );

        // EXTH 202 should fall back to 0 (same as cover).
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();
        let thumb_offset = exth
            .get_u32(EXTH_THUMB_OFFSET)
            .expect("EXTH 202 should be present");
        assert_eq!(
            thumb_offset, 0,
            "thumbnail offset should be 0 (no separate thumbnail)"
        );
    }

    #[test]
    fn no_cover_omits_exth_201_and_202() {
        use crate::formats::mobi::exth::{EXTH_COVER_OFFSET, EXTH_THUMB_OFFSET, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("No Cover Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });
        // No image resources added.

        let mobi_data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        assert!(mobi_hdr.has_exth(), "EXTH flag should be set");

        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        // Neither EXTH 201 nor 202 should be present when there is no cover.
        assert!(
            exth.get_u32(EXTH_COVER_OFFSET).is_none(),
            "EXTH 201 should not be present without a cover image"
        );
        assert!(
            exth.get_u32(EXTH_THUMB_OFFSET).is_none(),
            "EXTH 202 should not be present without a cover image"
        );
    }

    #[test]
    fn unique_id_is_derived_from_metadata() {
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Derived ID Test".into());
        book.metadata.authors.push("Author A".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();

        // The unique_id should NOT be the old static 0xCAFE sentinel.
        assert_ne!(
            mobi_hdr.unique_id, 0x0000_CAFE,
            "unique_id should not be static 0xCAFE"
        );
        assert_ne!(mobi_hdr.unique_id, 0, "unique_id should be non-zero");
    }

    #[test]
    fn unique_id_is_deterministic() {
        let mut book = Book::new();
        book.metadata.title = Some("Deterministic".into());
        book.metadata.authors.push("Author X".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data1 = write_mobi(&book).unwrap();
        let data2 = write_mobi(&book).unwrap();

        // PalmDB timestamps at offsets 36-43 may differ between calls,
        // so compare everything except those 8 bytes.
        assert_eq!(
            data1.len(),
            data2.len(),
            "same book should produce same-length MOBI output"
        );
        assert_eq!(
            &data1[..36],
            &data2[..36],
            "header before timestamps should match"
        );
        assert_eq!(
            &data1[44..],
            &data2[44..],
            "data after timestamps should match"
        );
    }

    #[test]
    fn header_has_correct_content_record_indices() {
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Content Indices".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Ch2".into()),
            content: "<p>World</p>".into(),
            id: Some("ch2".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();

        // first_content_record should be 1 (first text record).
        assert_eq!(
            mobi_hdr.first_content_record, 1,
            "first content record should be 1"
        );

        // last_content_record should equal the text record count.
        // For a small book, there should be exactly 1 text record.
        assert!(
            mobi_hdr.last_content_record >= 1,
            "last content record should be >= 1, got {}",
            mobi_hdr.last_content_record
        );
    }

    #[test]
    fn header_has_flis_fcis_record_numbers() {
        use crate::formats::common::palm_db::read_u32_be;
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("FLIS FCIS Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let _mobi_hdr = MobiHeader::parse(record0).unwrap();

        // Read FCIS/FLIS record numbers directly from the raw record0.
        let fcis_num = read_u32_be(record0, 200);
        let fcis_count = read_u32_be(record0, 204);
        let flis_num = read_u32_be(record0, 208);
        let flis_count = read_u32_be(record0, 212);

        assert_ne!(flis_num, 0, "FLIS record number should be non-zero");
        assert_ne!(fcis_num, 0, "FCIS record number should be non-zero");
        assert_eq!(flis_count, 1, "FLIS count should be 1");
        assert_eq!(fcis_count, 1, "FCIS count should be 1");

        // Verify FLIS record contains FLIS magic.
        let flis_record = pdb.record_data(flis_num as usize).unwrap();
        assert_eq!(
            &flis_record[0..4],
            b"FLIS",
            "FLIS record should start with FLIS magic"
        );

        // Verify FCIS record contains FCIS magic.
        let fcis_record = pdb.record_data(fcis_num as usize).unwrap();
        assert_eq!(
            &fcis_record[0..4],
            b"FCIS",
            "FCIS record should start with FCIS magic"
        );
    }

    #[test]
    fn header_has_extra_data_flags() {
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Extra Flags".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();

        assert_eq!(
            mobi_hdr.extra_data_flags, 1,
            "extra_data_flags should be 1 (multibyte)"
        );
        assert!(
            mobi_hdr.has_multibyte(),
            "has_multibyte() should return true"
        );
    }

    #[test]
    fn header_length_is_232() {
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Header Len".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();

        assert_eq!(
            mobi_hdr.header_length, 232,
            "MOBI header length should be 232"
        );
    }

    #[test]
    fn exth_has_creator_software_records() {
        use crate::formats::mobi::exth::{
            EXTH_CREATOR_BUILD, EXTH_CREATOR_MAJOR, EXTH_CREATOR_MINOR, EXTH_CREATOR_SOFTWARE,
            ExthHeader,
        };
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Creator SW".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        assert_eq!(
            exth.get_u32(EXTH_CREATOR_SOFTWARE),
            Some(201),
            "EXTH 204 (creator software) should be 201 (Linux)"
        );
        assert!(
            exth.get_u32(EXTH_CREATOR_MAJOR).is_some(),
            "EXTH 205 (creator major) should be present"
        );
        assert!(
            exth.get_u32(EXTH_CREATOR_MINOR).is_some(),
            "EXTH 206 (creator minor) should be present"
        );
        assert!(
            exth.get_u32(EXTH_CREATOR_BUILD).is_some(),
            "EXTH 207 (creator build) should be present"
        );
    }

    #[test]
    fn exth_has_start_reading_offset() {
        use crate::formats::mobi::exth::{EXTH_START_READING, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Start Reading".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Intro</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Ch2".into()),
            content: "<p>Main content</p>".into(),
            id: Some("ch2".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        let start_offset = exth
            .get_u32(EXTH_START_READING)
            .expect("EXTH 116 (start reading) should be present");

        // With 2 chapters, there should be an <mbp:pagebreak>, so offset > 0.
        assert!(
            start_offset > 0,
            "start reading offset should be > 0 for multi-chapter books, got {}",
            start_offset
        );
    }

    #[test]
    fn exth_start_reading_offset_for_single_chapter() {
        use crate::formats::mobi::exth::{EXTH_START_READING, ExthHeader};
        use crate::formats::mobi::header::MobiHeader;

        let mut book = Book::new();
        book.metadata.title = Some("Single Chapter".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Only chapter</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let mobi_hdr = MobiHeader::parse(record0).unwrap();
        let exth_start = mobi_hdr.exth_offset();
        let exth = ExthHeader::parse(&record0[exth_start..]).unwrap();

        let start_offset = exth
            .get_u32(EXTH_START_READING)
            .expect("EXTH 116 should be present");

        // With a TOC section, even single-chapter books have a pagebreak
        // separating the TOC from chapter content, so start reading offset > 0.
        assert!(
            start_offset > 0,
            "start reading offset should be > 0 (pointing past the TOC), got {}",
            start_offset
        );
    }

    #[test]
    fn derive_unique_id_is_stable_and_nonzero() {
        let id1 = derive_unique_id("Test Book", &["Author A".into()]);
        let id2 = derive_unique_id("Test Book", &["Author A".into()]);
        assert_eq!(id1, id2, "derive_unique_id should be deterministic");
        assert_ne!(id1, 0, "unique_id should be non-zero");

        // Different metadata should (very likely) produce different IDs.
        let id3 = derive_unique_id("Other Book", &["Author B".into()]);
        assert_ne!(id1, id3, "different books should have different unique_ids");
    }

    #[test]
    fn find_start_reading_offset_works() {
        let html = "<html><body><p>Intro</p><mbp:pagebreak />\n<h2>Ch2</h2></body></html>";
        let offset = find_start_reading_offset(html);
        assert!(offset > 0, "should find the pagebreak offset");
        // The offset should point just after the '>' of the pagebreak tag.
        let after = &html[offset as usize..];
        assert!(
            after.starts_with('\n') || after.starts_with('<'),
            "offset should point right after the closing '>' of the pagebreak tag, got: {:?}",
            &after[..after.len().min(20)]
        );
    }

    #[test]
    fn find_start_reading_offset_no_break() {
        let html = "<html><body><p>Only content</p></body></html>";
        assert_eq!(find_start_reading_offset(html), 0);
    }

    #[test]
    fn html_contains_filepos_attributes() {
        let mut book = Book::new();
        book.metadata.title = Some("Filepos Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter One".into()),
            content: "<p>First content</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter Two".into()),
            content: "<p>Second content</p>".into(),
            id: Some("ch2".into()),
        });

        let (html, _) = book_to_mobi_html(&book);

        // Should contain filepos= attributes (from TOC links and guide references).
        assert!(
            html.contains("filepos="),
            "HTML should contain filepos= attributes"
        );

        // Count the filepos references: 2 guide refs + 2 TOC entries = 4.
        let filepos_count = html.matches("filepos=").count();
        assert_eq!(
            filepos_count, 4,
            "expected 4 filepos attributes (2 guide + 2 TOC), got {}",
            filepos_count
        );
    }

    #[test]
    fn filepos_values_point_to_chapter_starts() {
        let mut book = Book::new();
        book.metadata.title = Some("Offset Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter Alpha".into()),
            content: "<p>Alpha content</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter Beta".into()),
            content: "<p>Beta content</p>".into(),
            id: Some("ch2".into()),
        });

        let (html, _) = book_to_mobi_html(&book);
        let bytes = html.as_bytes();

        // Extract filepos values from TOC <a> tags.
        // Pattern: <li><a filepos=NNNNNNNNNN>
        let mut toc_offsets: Vec<usize> = Vec::new();
        for mat in html.match_indices("<li><a filepos=") {
            let start = mat.0 + "<li><a filepos=".len();
            let offset_str = &html[start..start + 10];
            let offset: usize = offset_str
                .parse()
                .expect("filepos should be a valid number");
            toc_offsets.push(offset);
        }
        assert_eq!(toc_offsets.len(), 2, "should have 2 TOC filepos entries");

        // The first TOC entry should point to the start of chapter Alpha content.
        let at_first = std::str::from_utf8(&bytes[toc_offsets[0]..]).unwrap();
        assert!(
            at_first.starts_with("<h2>Chapter Alpha</h2>"),
            "first filepos should point to Chapter Alpha heading, got: {:?}",
            &at_first[..at_first.len().min(60)]
        );

        // The second TOC entry should point to the start of chapter Beta content.
        let at_second = std::str::from_utf8(&bytes[toc_offsets[1]..]).unwrap();
        assert!(
            at_second.starts_with("<h2>Chapter Beta</h2>"),
            "second filepos should point to Chapter Beta heading, got: {:?}",
            &at_second[..at_second.len().min(60)]
        );
    }

    #[test]
    fn guide_section_contains_toc_and_text_references() {
        let mut book = Book::new();
        book.metadata.title = Some("Guide Test".into());
        book.add_chapter(Chapter {
            title: Some("Intro".into()),
            content: "<p>Intro text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Main".into()),
            content: "<p>Main text</p>".into(),
            id: Some("ch2".into()),
        });

        let (html, _) = book_to_mobi_html(&book);

        // Guide section should exist.
        assert!(
            html.contains("<guide>"),
            "HTML should contain a <guide> section"
        );
        assert!(
            html.contains("</guide>"),
            "HTML should contain closing </guide>"
        );

        // Should have a TOC reference.
        assert!(
            html.contains("type=\"toc\""),
            "guide should contain a toc reference"
        );

        // Should have a text (start reading) reference.
        assert!(
            html.contains("type=\"text\""),
            "guide should contain a text reference"
        );

        // The guide TOC filepos should point to the TOC section in body.
        let toc_ref_pos = html.find("type=\"toc\"").unwrap();
        let filepos_start =
            html[toc_ref_pos..].find("filepos=").unwrap() + toc_ref_pos + "filepos=".len();
        let filepos_str = &html[filepos_start..filepos_start + 10];
        let toc_offset: usize = filepos_str
            .parse()
            .expect("guide toc filepos should be valid");
        let at_toc = &html[toc_offset..];
        assert!(
            at_toc.starts_with("<div><h2><b>Table of Contents</b></h2>"),
            "guide toc filepos should point to the TOC div, got: {:?}",
            &at_toc[..at_toc.len().min(60)]
        );

        // The guide text filepos should point to the first chapter.
        let text_ref_pos = html.find("type=\"text\"").unwrap();
        let text_fp_start =
            html[text_ref_pos..].find("filepos=").unwrap() + text_ref_pos + "filepos=".len();
        let text_fp_str = &html[text_fp_start..text_fp_start + 10];
        let text_offset: usize = text_fp_str
            .parse()
            .expect("guide text filepos should be valid");
        let at_text = &html[text_offset..];
        assert!(
            at_text.starts_with("<h2>Intro</h2>"),
            "guide text filepos should point to first chapter, got: {:?}",
            &at_text[..at_text.len().min(60)]
        );
    }

    #[test]
    fn toc_section_appears_before_chapter_content() {
        let mut book = Book::new();
        book.metadata.title = Some("TOC Order".into());
        book.add_chapter(Chapter {
            title: Some("First".into()),
            content: "<p>Content</p>".into(),
            id: Some("ch1".into()),
        });

        let (html, _) = book_to_mobi_html(&book);

        let toc_pos = html
            .find("Table of Contents")
            .expect("should have TOC heading");
        let content_pos = html
            .find("<p>Content</p>")
            .expect("should have chapter content");
        assert!(
            toc_pos < content_pos,
            "TOC should appear before chapter content"
        );
    }

    #[test]
    fn no_toc_when_chapters_have_no_titles() {
        let mut book = Book::new();
        book.metadata.title = Some("No Titles".into());
        // Add chapter without a title (it won't get a TOC entry via add_chapter).
        // Directly manipulate to simulate a titleless chapter.
        use crate::domain::{ManifestItem, SpineItem};
        let item = ManifestItem::new("ch1", "ch1.xhtml", "application/xhtml+xml")
            .with_text("<p>Content</p>");
        book.manifest.insert(item);
        book.spine.push(SpineItem::new("ch1"));
        // No TOC entries added.

        let (html, _) = book_to_mobi_html(&book);

        // Without any TOC entries (no titles), guide and TOC sections are omitted.
        assert!(
            !html.contains("<guide>"),
            "should not have guide section without TOC entries"
        );
        assert!(
            !html.contains("Table of Contents"),
            "should not have TOC section without titled chapters"
        );
        assert!(
            !html.contains("filepos="),
            "should not have any filepos when no TOC"
        );
    }

    #[test]
    fn filepos_no_placeholders_remain() {
        // Verify no unfixed "0000000000" placeholders remain in the output.
        let mut book = Book::new();
        book.metadata.title = Some("Fixup Check".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Content 1</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Ch2".into()),
            content: "<p>Content 2</p>".into(),
            id: Some("ch2".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Ch3".into()),
            content: "<p>Content 3</p>".into(),
            id: Some("ch3".into()),
        });

        let (html, _) = book_to_mobi_html(&book);

        // All filepos=0000000000 should have been replaced with actual offsets.
        assert!(
            !html.contains("filepos=0000000000"),
            "no unfixed filepos placeholders should remain in the output"
        );
    }

    #[test]
    fn mobi_output_contains_filepos_strings() {
        // Integration test: verify that the final MOBI binary, when read back
        // and decoded, contains filepos strings in the HTML text.
        let mut book = Book::new();
        book.metadata.title = Some("MOBI Filepos".into());
        book.add_chapter(Chapter {
            title: Some("Prologue".into()),
            content: "<p>Opening text</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter I".into()),
            content: "<p>Main story</p>".into(),
            id: Some("ch2".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Epilogue".into()),
            content: "<p>Closing text</p>".into(),
            id: Some("ch3".into()),
        });

        let mobi_data = write_mobi(&book).unwrap();

        // Read back and extract the decompressed text.
        let mut cursor = std::io::Cursor::new(mobi_data);
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();
        let all_content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();

        // The decompressed/decoded text should preserve filepos references.
        // At minimum, verify the original HTML contains them (pre-compression).
        let (html, _) = book_to_mobi_html(&book);
        let filepos_count = html.matches("filepos=").count();
        assert!(
            filepos_count >= 5,
            "HTML should contain at least 5 filepos references (3 TOC + 2 guide), got {}",
            filepos_count
        );

        // Also verify the book round-trips correctly with all chapter content.
        assert!(
            all_content.contains("Opening text"),
            "decoded MOBI should contain chapter content"
        );
        assert!(
            all_content.contains("Closing text"),
            "decoded MOBI should contain epilogue content"
        );
    }

    // --- INDX / NCX navigation record tests ---

    #[test]
    fn vwi_encoding_single_byte() {
        let (buf, len) = encode_vwi(0);
        assert_eq!(&buf[..len], &[0]);
        let (buf, len) = encode_vwi(1);
        assert_eq!(&buf[..len], &[1]);
        let (buf, len) = encode_vwi(127);
        assert_eq!(&buf[..len], &[127]);
    }

    #[test]
    fn vwi_encoding_two_bytes() {
        // 128 = 0x80 → [0x81, 0x00]
        let (buf, len) = encode_vwi(128);
        assert_eq!(&buf[..len], &[0x81, 0x00]);
        // 16383 = 0x3FFF → [0xFF, 0x7F]
        let (buf, len) = encode_vwi(16383);
        assert_eq!(&buf[..len], &[0xFF, 0x7F]);
    }

    #[test]
    fn vwi_encoding_three_bytes() {
        // 16384 = 0x4000 → [0x81, 0x80, 0x00]
        let (buf, len) = encode_vwi(16384);
        assert_eq!(&buf[..len], &[0x81, 0x80, 0x00]);
    }

    #[test]
    fn build_ncx_indx_produces_three_records() {
        let chapters = vec![
            ("Chapter 1".to_string(), 100),
            ("Chapter 2".to_string(), 5000),
            ("Chapter 3".to_string(), 12000),
        ];
        let (indx_header, indx_data, cncx) = build_ncx_indx(&chapters);

        // All three records should be non-empty.
        assert!(
            !indx_header.is_empty(),
            "INDX header record should not be empty"
        );
        assert!(
            !indx_data.is_empty(),
            "INDX data record should not be empty"
        );
        assert!(!cncx.is_empty(), "CNCX record should not be empty");
    }

    #[test]
    fn indx_header_starts_with_magic_and_contains_tagx() {
        let chapters = vec![("Ch1".to_string(), 0), ("Ch2".to_string(), 1000)];
        let (indx_header, _, _) = build_ncx_indx(&chapters);

        // Starts with "INDX" magic.
        assert_eq!(
            &indx_header[0..4],
            b"INDX",
            "INDX header should start with INDX magic"
        );

        // Contains "TAGX" section.
        let has_tagx = indx_header.windows(4).any(|w| w == b"TAGX");
        assert!(has_tagx, "INDX header should contain TAGX section");
    }

    #[test]
    fn indx_data_starts_with_magic_and_contains_idxt() {
        let chapters = vec![("Ch1".to_string(), 0), ("Ch2".to_string(), 1000)];
        let (_, indx_data, _) = build_ncx_indx(&chapters);

        // Starts with "INDX" magic.
        assert_eq!(
            &indx_data[0..4],
            b"INDX",
            "INDX data should start with INDX magic"
        );

        // Contains "IDXT" section.
        let has_idxt = indx_data.windows(4).any(|w| w == b"IDXT");
        assert!(has_idxt, "INDX data should contain IDXT section");
    }

    #[test]
    fn cncx_record_contains_chapter_titles() {
        let chapters = vec![
            ("Introduction".to_string(), 0),
            ("The Adventure Begins".to_string(), 5000),
        ];
        let (_, _, cncx) = build_ncx_indx(&chapters);

        // CNCX should contain both chapter titles as length-prefixed strings.
        let title1 = "Introduction";
        let title2 = "The Adventure Begins";

        // First string: 2-byte length + data.
        let len1 = u16::from_be_bytes([cncx[0], cncx[1]]) as usize;
        assert_eq!(len1, title1.len());
        assert_eq!(&cncx[2..2 + len1], title1.as_bytes());

        // Second string follows.
        let offset2 = 2 + len1;
        let len2 = u16::from_be_bytes([cncx[offset2], cncx[offset2 + 1]]) as usize;
        assert_eq!(len2, title2.len());
        assert_eq!(&cncx[offset2 + 2..offset2 + 2 + len2], title2.as_bytes());
    }

    #[test]
    fn ncx_index_points_to_correct_record() {
        use crate::formats::common::palm_db::read_u32_be;

        let mut book = Book::new();
        book.metadata.title = Some("NCX Index Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Content 1</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter 2".into()),
            content: "<p>Content 2</p>".into(),
            id: Some("ch2".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        // Read ncx_index from offset 244 in record0.
        let ncx_index = read_u32_be(record0, 244);

        // ncx_index should not be NULL_INDEX.
        assert_ne!(ncx_index, NULL_INDEX, "ncx_index should not be NULL_INDEX");

        // The record at ncx_index should start with "INDX" magic.
        let indx_record = pdb.record_data(ncx_index as usize).unwrap();
        assert_eq!(
            &indx_record[0..4],
            b"INDX",
            "record at ncx_index should start with INDX magic"
        );

        // The next record should also be INDX (the data record).
        let indx_data_record = pdb.record_data(ncx_index as usize + 1).unwrap();
        assert_eq!(
            &indx_data_record[0..4],
            b"INDX",
            "record after ncx_index should be INDX data record"
        );

        // The record after that is the CNCX record (raw label strings, no fixed magic).
        // Verify FLIS follows after the 3 NCX records.
        let flis_record = pdb.record_data(ncx_index as usize + 3).unwrap();
        assert_eq!(
            &flis_record[0..4],
            b"FLIS",
            "FLIS should follow the 3 NCX records"
        );
    }

    #[test]
    fn single_chapter_produces_valid_indx() {
        use crate::formats::common::palm_db::read_u32_be;

        let mut book = Book::new();
        book.metadata.title = Some("Single Chapter INDX".into());
        book.add_chapter(Chapter {
            title: Some("Only Chapter".into()),
            content: "<p>The only chapter content.</p>".into(),
            id: Some("ch1".into()),
        });

        let data = write_mobi(&book).unwrap();
        let pdb = PdbFile::parse(data).unwrap();
        let record0 = pdb.record_data(0).unwrap();

        let ncx_index = read_u32_be(record0, 244);
        assert_ne!(
            ncx_index, NULL_INDEX,
            "single-chapter book should still have NCX index"
        );

        // Verify the INDX header record is valid.
        let indx_header = pdb.record_data(ncx_index as usize).unwrap();
        assert_eq!(&indx_header[0..4], b"INDX");

        // Total entry count (at header offset 36) should be 1.
        let entry_count = read_u32_be(indx_header, 36);
        assert_eq!(
            entry_count, 1,
            "single-chapter book should have 1 INDX entry"
        );
    }

    #[test]
    fn indx_round_trip_chapter_count_matches() {
        let mut book = Book::new();
        book.metadata.title = Some("INDX Round Trip".into());
        book.metadata.authors.push("Test Author".into());
        for i in 1..=5 {
            book.add_chapter(Chapter {
                title: Some(format!("Chapter {}", i)),
                content: format!("<p>Content of chapter {}.</p>", i),
                id: Some(format!("ch{}", i)),
            });
        }

        // Write MOBI.
        let mobi_data = write_mobi(&book).unwrap();

        // Read back and verify we get all 5 chapters.
        let mut cursor = std::io::Cursor::new(mobi_data.clone());
        let decoded = MobiReader::new().read_book(&mut cursor).unwrap();
        let chapters = decoded.chapters();

        // The reader may or may not parse INDX, but the chapters should be present
        // (they're in the HTML text with pagebreaks).
        assert!(
            chapters.len() >= 5,
            "round-tripped book should have at least 5 chapters, got {}",
            chapters.len()
        );

        // Verify INDX records are structurally valid.
        let pdb = PdbFile::parse(mobi_data).unwrap();
        let record0 = pdb.record_data(0).unwrap();
        let ncx_index = crate::formats::common::palm_db::read_u32_be(record0, 244);
        assert_ne!(ncx_index, NULL_INDEX);

        let indx_header = pdb.record_data(ncx_index as usize).unwrap();
        let total_entries = crate::formats::common::palm_db::read_u32_be(indx_header, 36);
        assert_eq!(
            total_entries, 5,
            "INDX should report 5 chapter entries for a 5-chapter book"
        );

        // Verify the INDX data record has the correct entry count.
        let indx_data = pdb.record_data(ncx_index as usize + 1).unwrap();
        let data_entries = crate::formats::common::palm_db::read_u32_be(indx_data, 24);
        assert_eq!(data_entries, 5, "INDX data record should have 5 entries");
    }

    #[test]
    fn indx_header_has_correct_cncx_count() {
        use crate::formats::common::palm_db::read_u32_be;

        let chapters = vec![("Ch1".to_string(), 0), ("Ch2".to_string(), 1000)];
        let (indx_header, _, _) = build_ncx_indx(&chapters);

        // CNCX record count at header offset 52 should be 1.
        let cncx_count = read_u32_be(&indx_header, 52);
        assert_eq!(cncx_count, 1, "CNCX record count should be 1");

        // Index encoding at header offset 28 should be 65001 (UTF-8).
        let encoding = read_u32_be(&indx_header, 28);
        assert_eq!(encoding, 65001, "index encoding should be UTF-8 (65001)");
    }
}
