//! PDB (Palm Database) ebook reader.
//!
//! Supports reading PDB files containing:
//! - **PalmDOC** (`TEXtREAd`) — PalmDoc LZ77 compressed plain text
//! - **zTXT** (`zTXTGPlm`) — zlib compressed plain text
//! - **eReader** (`PNRdPPrs`/`PNPdPPrs`) — PalmDoc/zlib compressed PML markup with images
//! - **Plucker** (`DataPlkr`) — PalmDoc/zlib compressed PHTML with images

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::compression::palmdoc;
use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be, PdbFile, read_u16_be};
use flate2::bufread::ZlibDecoder;
use std::io::{Read, Write};

/// PDB ebook format reader.
///
/// Detects the PDB subtype from the type/creator identity and delegates
/// to the appropriate parser.
#[derive(Default)]
pub struct PdbReader;

impl PdbReader {
    pub fn new() -> Self {
        Self
    }
}

/// PDB (PalmDOC) format writer.
///
/// Writes a book as a PalmDOC (`TEXtREAd`) PDB file with LZ77 compression.
#[derive(Default)]
pub struct PdbWriter;

impl PdbWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for PdbWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        // Extract plain text from chapters by stripping HTML tags.
        let mut text = String::new();
        for chapter in book.chapters() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&strip_html(&chapter.content));
        }

        let text_bytes = text.as_bytes();
        let max_record_size = 4096usize;

        // Split into records and compress.
        let mut records: Vec<Vec<u8>> = Vec::new();
        let mut offset = 0;
        while offset < text_bytes.len() {
            let end = (offset + max_record_size).min(text_bytes.len());
            records.push(palmdoc::compress(&text_bytes[offset..end]));
            offset = end;
        }
        if records.is_empty() {
            records.push(Vec::new());
        }

        // Build header record (16 bytes).
        let mut header_rec = vec![0u8; 16];
        write_u16_be(&mut header_rec, 0, COMPRESSION_PALMDOC); // compression = 2
        // bytes 2-3: unused
        write_u32_be(&mut header_rec, 4, text_bytes.len() as u32); // uncompressed length
        write_u16_be(&mut header_rec, 8, records.len() as u16); // record count
        write_u16_be(&mut header_rec, 10, max_record_size as u16); // max record size
        // bytes 12-15: current position = 0

        // Calculate record offsets.
        let total_records = 1 + records.len();
        let header_size = 78 + total_records * 8 + 2; // PDB header + table + gap

        let mut offsets = Vec::with_capacity(total_records);
        let mut pos = header_size as u32;
        offsets.push(pos);
        pos += header_rec.len() as u32;
        for rec in &records {
            offsets.push(pos);
            pos += rec.len() as u32;
        }

        let name = book.metadata.title.as_deref().unwrap_or("Untitled");
        let pdb = build_pdb_header(name, b"TEXt", b"REAd", total_records as u16, &offsets);

        output.write_all(&pdb).map_err(EruditioError::Io)?;
        output.write_all(&header_rec).map_err(EruditioError::Io)?;
        for rec in &records {
            output.write_all(rec).map_err(EruditioError::Io)?;
        }

        Ok(())
    }
}
const IDENT_PALMDOC: &[u8; 8] = b"TEXtREAd";
const IDENT_ZTXT: &[u8; 8] = b"zTXTGPlm";
const IDENT_EREADER: &[u8; 8] = b"PNRdPPrs";
const IDENT_EREADER_ALT: &[u8; 8] = b"PNPdPPrs";
const IDENT_PLUCKER: &[u8; 8] = b"DataPlkr";
const IDENT_HAODOO_LEGACY: &[u8; 8] = b"BOOKMTIT";
const IDENT_HAODOO_UNICODE: &[u8; 8] = b"BOOKMTIU";

/// PalmDOC compression types.
const COMPRESSION_NONE: u16 = 1;
const COMPRESSION_PALMDOC: u16 = 2;

impl FormatReader for PdbReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data).map_err(EruditioError::Io)?;

        let pdb = PdbFile::parse(data)?;
        let identity = pdb.header.identity();

        if &identity == IDENT_PALMDOC {
            read_palmdoc(&pdb)
        } else if &identity == IDENT_ZTXT {
            read_ztxt(&pdb)
        } else if &identity == IDENT_EREADER || &identity == IDENT_EREADER_ALT {
            read_ereader(&pdb)
        } else if &identity == IDENT_PLUCKER {
            read_plucker(&pdb)
        } else if &identity == IDENT_HAODOO_LEGACY {
            read_haodoo(&pdb, false)
        } else if &identity == IDENT_HAODOO_UNICODE {
            read_haodoo(&pdb, true)
        } else if pdb.header.is_mobi() {
            Err(EruditioError::Format(
                "MOBI files should use MobiReader, not PdbReader".into(),
            ))
        } else {
            Err(EruditioError::Unsupported(format!(
                "Unknown PDB subtype: {:?}",
                String::from_utf8_lossy(&identity)
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// zlib helper
// ---------------------------------------------------------------------------

/// Decompresses zlib-compressed data.
fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|e| EruditioError::Compression(format!("zlib decompression failed: {}", e)))?;
    Ok(output)
}

// ---------------------------------------------------------------------------
// PalmDOC (`TEXtREAd`)
// ---------------------------------------------------------------------------

/// Reads a PalmDOC (`TEXtREAd`) PDB file.
fn read_palmdoc(pdb: &PdbFile) -> Result<Book> {
    if pdb.record_count() < 2 {
        return Err(EruditioError::Format(
            "PalmDOC file has no text records".into(),
        ));
    }

    let header_rec = pdb.record_data(0)?;
    if header_rec.len() < 16 {
        return Err(EruditioError::Format(
            "PalmDOC header record too short".into(),
        ));
    }

    let compression = read_u16_be(header_rec, 0);
    let num_text_records = read_u16_be(header_rec, 8) as usize;

    let mut text = String::new();
    let record_limit = num_text_records.min(pdb.record_count() - 1);

    for i in 1..=record_limit {
        let record = pdb.record_data(i)?;
        let decompressed = match compression {
            COMPRESSION_NONE => record.to_vec(),
            COMPRESSION_PALMDOC => palmdoc::decompress(record)?,
            other => {
                return Err(EruditioError::Unsupported(format!(
                    "Unsupported PalmDOC compression type: {}",
                    other
                )));
            }
        };
        text.push_str(&String::from_utf8_lossy(&decompressed));
    }

    let mut book = Book::new();
    book.metadata.title = Some(pdb.header.name.clone());

    let html = text_to_html(&text);
    book.add_chapter(&Chapter {
        title: Some(pdb.header.name.clone()),
        content: html,
        id: Some("main".into()),
    });

    Ok(book)
}

// ---------------------------------------------------------------------------
// zTXT (`zTXTGPlm`)
// ---------------------------------------------------------------------------

/// Reads a zTXT PDB file.
///
/// zTXT uses zlib compression with a streaming decompressor. Each record
/// is independently decompressible (random-access mode, flag bit 0).
fn read_ztxt(pdb: &PdbFile) -> Result<Book> {
    if pdb.record_count() < 2 {
        return Err(EruditioError::Format(
            "zTXT file has no data records".into(),
        ));
    }

    let header_rec = pdb.record_data(0)?;
    if header_rec.len() < 19 {
        return Err(EruditioError::Format(
            "zTXT header record too short".into(),
        ));
    }

    let version = read_u16_be(header_rec, 0);
    let num_records = read_u16_be(header_rec, 2) as usize;
    let flags = header_rec[18];

    // Check version >= 1.40.
    let vmajor = (version >> 8) & 0xFF;
    let vminor = version & 0xFF;
    if vmajor < 1 || (vmajor == 1 && vminor < 40) {
        return Err(EruditioError::Unsupported(format!(
            "Unsupported zTXT version {}.{} (need >= 1.40)",
            vmajor, vminor
        )));
    }

    // Only random-access compression (flag bit 0) is supported.
    if flags & 0x01 == 0 {
        return Err(EruditioError::Unsupported(
            "Only random-access zTXT compression is supported".into(),
        ));
    }

    // Decompress text records.
    let mut text = Vec::new();
    let record_limit = num_records.min(pdb.record_count() - 1);

    for i in 1..=record_limit {
        let record = pdb.record_data(i)?;
        let decompressed = zlib_decompress(record)?;
        text.extend_from_slice(&decompressed);
    }

    let text_str = String::from_utf8_lossy(&text);

    let mut book = Book::new();
    book.metadata.title = Some(pdb.header.name.clone());

    let html = text_to_html(&text_str);
    book.add_chapter(&Chapter {
        title: Some(pdb.header.name.clone()),
        content: html,
        id: Some("main".into()),
    });

    Ok(book)
}

// ---------------------------------------------------------------------------
// eReader (`PNRdPPrs` / `PNPdPPrs`)
// ---------------------------------------------------------------------------

/// eReader 132-byte header fields.
struct EreaderHeader {
    compression: u16,
    non_text_offset: u16,
    image_count: u16,
    footnote_count: u16,
    sidebar_count: u16,
    image_data_offset: u16,
    footnote_offset: u16,
    sidebar_offset: u16,
}

impl EreaderHeader {
    fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 54 {
            return Err(EruditioError::Format(
                "eReader header record too short".into(),
            ));
        }
        Ok(Self {
            compression: read_u16_be(data, 0),
            non_text_offset: read_u16_be(data, 12),
            image_count: read_u16_be(data, 20),
            footnote_count: read_u16_be(data, 28),
            sidebar_count: read_u16_be(data, 30),
            image_data_offset: read_u16_be(data, 40),
            footnote_offset: read_u16_be(data, 48),
            sidebar_offset: read_u16_be(data, 50),
        })
    }

    fn num_text_pages(&self) -> u16 {
        self.non_text_offset.saturating_sub(1)
    }
}

/// Reads an eReader PDB file.
///
/// eReader text is PML markup, compressed with PalmDoc or zlib.
/// Images are stored in dedicated records after the text.
fn read_ereader(pdb: &PdbFile) -> Result<Book> {
    if pdb.record_count() < 2 {
        return Err(EruditioError::Format(
            "eReader file has no data records".into(),
        ));
    }

    let header_rec = pdb.record_data(0)?;
    let header = EreaderHeader::parse(header_rec)?;

    // Check for DRM.
    if header.compression == 260 || header.compression == 272 {
        return Err(EruditioError::Unsupported(
            "eReader DRM-protected files are not supported".into(),
        ));
    }
    if header.compression != 2 && header.compression != 10 {
        return Err(EruditioError::Unsupported(format!(
            "Unknown eReader compression type: {}",
            header.compression
        )));
    }

    // Decompress and decode text pages (PML markup in CP-1252).
    let mut pml_text = String::new();
    let num_pages = header.num_text_pages() as usize;

    for i in 1..=num_pages {
        if i >= pdb.record_count() {
            break;
        }
        let record = pdb.record_data(i)?;
        let decompressed = ereader_decompress(record, header.compression)?;
        pml_text.push_str(&decode_cp1252(&decompressed));
    }

    // Convert PML to HTML.
    let html = crate::formats::pml::parser::pml_to_html(&pml_text);

    // Extract footnotes if present.
    let mut footnote_html = String::new();
    if header.footnote_count > 0 {
        let fn_offset = header.footnote_offset as usize;
        // The first record at footnote_offset contains footnote IDs (null-separated).
        let fn_ids = if fn_offset < pdb.record_count() {
            extract_null_separated_ids(pdb.record_data(fn_offset)?)
        } else {
            Vec::new()
        };

        footnote_html.push_str("<h2>Footnotes</h2>");
        for (idx, i) in
            (fn_offset + 1..fn_offset + header.footnote_count as usize).enumerate()
        {
            if i >= pdb.record_count() {
                break;
            }
            let record = pdb.record_data(i)?;
            let decompressed = ereader_decompress(record, header.compression)?;
            let fn_text = decode_cp1252(&decompressed);
            let fn_html = crate::formats::pml::parser::pml_to_html(&fn_text);
            let fn_id = fn_ids.get(idx).cloned().unwrap_or_default();
            footnote_html.push_str(&format!(
                "<div class=\"footnote\" id=\"fn_{}\"><p><b>[{}]</b> {}</p></div>",
                fn_id, fn_id, fn_html
            ));
        }
    }

    // Extract sidebars if present.
    let mut sidebar_html = String::new();
    if header.sidebar_count > 0 {
        let sb_offset = header.sidebar_offset as usize;
        let sb_ids = if sb_offset < pdb.record_count() {
            extract_null_separated_ids(pdb.record_data(sb_offset)?)
        } else {
            Vec::new()
        };

        sidebar_html.push_str("<h2>Sidebars</h2>");
        for (idx, i) in
            (sb_offset + 1..sb_offset + header.sidebar_count as usize).enumerate()
        {
            if i >= pdb.record_count() {
                break;
            }
            let record = pdb.record_data(i)?;
            let decompressed = ereader_decompress(record, header.compression)?;
            let sb_text = decode_cp1252(&decompressed);
            let sb_html = crate::formats::pml::parser::pml_to_html(&sb_text);
            let sb_id = sb_ids.get(idx).cloned().unwrap_or_default();
            sidebar_html.push_str(&format!(
                "<div class=\"sidebar\" id=\"sb_{}\"><p><b>[{}]</b> {}</p></div>",
                sb_id, sb_id, sb_html
            ));
        }
    }

    // Build the book.
    let mut book = Book::new();
    book.metadata.title = Some(pdb.header.name.clone());

    let full_html = if footnote_html.is_empty() && sidebar_html.is_empty() {
        html
    } else {
        format!("{}{}{}", html, footnote_html, sidebar_html)
    };

    book.add_chapter(&Chapter {
        title: Some(pdb.header.name.clone()),
        content: full_html,
        id: Some("main".into()),
    });

    // Extract images.
    if header.image_count > 0 {
        let img_offset = header.image_data_offset as usize;
        let num_images = header.image_count as usize;

        for i in 0..num_images {
            let rec_idx = img_offset + i;
            if rec_idx >= pdb.record_count() {
                break;
            }
            let data = pdb.record_data(rec_idx)?;
            if data.len() < 62 {
                continue;
            }

            // Image name: 32 bytes starting at offset 4, null-terminated.
            let name_bytes = &data[4..36];
            let name = name_bytes
                .split(|&b| b == 0)
                .next()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| format!("image_{}.bin", i));

            let img_data = &data[62..];
            if img_data.is_empty() {
                continue;
            }

            let media_type = detect_image_media_type(img_data);
            let id = format!("ereader_img_{}", i);
            book.add_resource(&id, &format!("images/{}", name), img_data.to_vec(), media_type);
        }
    }

    Ok(book)
}

/// Decompresses an eReader record using the specified compression type.
fn ereader_decompress(data: &[u8], compression: u16) -> Result<Vec<u8>> {
    match compression {
        2 => palmdoc::decompress(data),
        10 => zlib_decompress(data),
        _ => Err(EruditioError::Unsupported(format!(
            "Unknown eReader compression: {}",
            compression
        ))),
    }
}

/// Extracts null-separated ID strings from a record.
fn extract_null_separated_ids(data: &[u8]) -> Vec<String> {
    let text = decode_cp1252(data);
    text.split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Plucker (`DataPlkr`)
// ---------------------------------------------------------------------------

/// Plucker data types (section types).
const PLUCKER_PHTML: u8 = 0;
const PLUCKER_PHTML_COMPRESSED: u8 = 1;
const PLUCKER_TBMP: u8 = 2;
const PLUCKER_TBMP_COMPRESSED: u8 = 3;
const PLUCKER_METADATA: u8 = 10;

/// Plucker section header (8 bytes at start of each record after record 0).
struct PluckerSectionHeader {
    uid: u16,
    paragraphs: u16,
    _size: u16,
    data_type: u8,
    _flags: u8,
}

impl PluckerSectionHeader {
    fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(EruditioError::Format(
                "Plucker section header too short".into(),
            ));
        }
        Ok(Self {
            uid: read_u16_be(data, 0),
            paragraphs: read_u16_be(data, 2),
            _size: read_u16_be(data, 4),
            data_type: data[6],
            _flags: data[7],
        })
    }
}

/// Reads a Plucker PDB file.
///
/// Plucker uses PHTML (Plucker HTML) — a binary markup format with escape
/// codes for formatting, hyperlinks, and embedded images.
fn read_plucker(pdb: &PdbFile) -> Result<Book> {
    if pdb.record_count() < 2 {
        return Err(EruditioError::Format(
            "Plucker file has no data records".into(),
        ));
    }

    // Parse header record (record 0).
    let header_rec = pdb.record_data(0)?;
    if header_rec.len() < 6 {
        return Err(EruditioError::Format(
            "Plucker header record too short".into(),
        ));
    }
    let compression = read_u16_be(header_rec, 2); // 1=PalmDoc, 2=zlib
    let num_reserved = read_u16_be(header_rec, 4) as usize;

    // Determine the home page UID from the reserved table.
    let mut home_uid: Option<u16> = None;
    for i in 0..num_reserved {
        let base = 6 + i * 4;
        if base + 4 <= header_rec.len() {
            let name = read_u16_be(header_rec, base);
            let uid = read_u16_be(header_rec, base + 2);
            if name == 0 {
                home_uid = Some(uid);
            }
        }
    }

    // Scan all records and classify by type.
    let mut text_sections: Vec<(u16, String)> = Vec::new(); // (uid, html)
    let mut image_data: Vec<(u16, Vec<u8>)> = Vec::new(); // (uid, raw_image_bytes)

    // Detect default encoding from metadata section.
    let mut _default_encoding = "latin-1";

    for rec_idx in 1..pdb.record_count() {
        let raw = pdb.record_data(rec_idx)?;
        if raw.len() < 8 {
            continue;
        }

        let section_header = PluckerSectionHeader::parse(raw)?;
        let section_data = &raw[8..];

        match section_header.data_type {
            PLUCKER_PHTML => {
                let paragraph_offsets =
                    parse_paragraph_offsets(section_data, section_header.paragraphs);
                let phtml_data = &section_data[section_header.paragraphs as usize * 4..];
                let html = process_phtml(phtml_data, &paragraph_offsets);
                text_sections.push((section_header.uid, html));
            }
            PLUCKER_PHTML_COMPRESSED => {
                let paragraph_offsets =
                    parse_paragraph_offsets(section_data, section_header.paragraphs);
                let compressed_data =
                    &section_data[section_header.paragraphs as usize * 4..];
                let decompressed = plucker_decompress(compressed_data, compression)?;
                let html = process_phtml(&decompressed, &paragraph_offsets);
                text_sections.push((section_header.uid, html));
            }
            PLUCKER_TBMP => {
                image_data.push((section_header.uid, section_data.to_vec()));
            }
            PLUCKER_TBMP_COMPRESSED => {
                if let Ok(decompressed) = plucker_decompress(section_data, compression) {
                    image_data.push((section_header.uid, decompressed));
                }
            }
            PLUCKER_METADATA => {
                // Parse metadata for encoding info.
                if section_data.len() >= 2 {
                    let record_count = read_u16_be(section_data, 0) as usize;
                    let mut adv = 0usize;
                    for _ in 0..record_count {
                        if 2 + adv + 4 > section_data.len() {
                            break;
                        }
                        let mtype = read_u16_be(section_data, 2 + adv);
                        let mlength = read_u16_be(section_data, 4 + adv) as usize;
                        if mtype == 1 && 6 + adv + 2 <= section_data.len() {
                            // CharSet MIBenum — we'll use UTF-8 as fallback.
                            let _charset_id = read_u16_be(section_data, 6 + adv);
                        }
                        adv += mlength * 2;
                    }
                }
            }
            _ => {
                // Skip unknown section types.
            }
        }
    }

    // Build book from collected sections.
    let mut book = Book::new();
    book.metadata.title = Some(pdb.header.name.clone());

    // Order: home page first, then remaining pages.
    let mut ordered_html = Vec::new();
    if let Some(home) = home_uid {
        if let Some(pos) = text_sections.iter().position(|(uid, _)| *uid == home) {
            let (_, html) = text_sections.remove(pos);
            ordered_html.push(html);
        }
    }
    for (_, html) in text_sections {
        ordered_html.push(html);
    }

    if ordered_html.is_empty() {
        ordered_html.push("<p></p>".to_string());
    }

    // Combine all text into chapters (one per text section, or single if only one).
    if ordered_html.len() == 1 {
        book.add_chapter(&Chapter {
            title: Some(pdb.header.name.clone()),
            content: ordered_html.into_iter().next().unwrap(),
            id: Some("main".into()),
        });
    } else {
        for (i, html) in ordered_html.into_iter().enumerate() {
            book.add_chapter(&Chapter {
                title: Some(format!("Section {}", i + 1)),
                content: html,
                id: Some(format!("section_{}", i)),
            });
        }
    }

    // Add images as resources.
    for (uid, data) in &image_data {
        let media_type = detect_image_media_type(data);
        let ext = media_type_to_ext(media_type);
        let id = format!("plucker_img_{}", uid);
        let href = format!("images/{}.{}", uid, ext);
        book.add_resource(&id, &href, data.clone(), media_type);
    }

    Ok(book)
}

/// Parses paragraph offset/size table from a Plucker text section.
fn parse_paragraph_offsets(data: &[u8], count: u16) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut running = 0usize;
    for i in 0..count as usize {
        let base = i * 4;
        if base + 2 > data.len() {
            break;
        }
        let size = read_u16_be(data, base) as usize;
        running += size;
        offsets.push(running);
    }
    offsets
}

/// Decompresses Plucker data using the file-level compression setting.
fn plucker_decompress(data: &[u8], compression: u16) -> Result<Vec<u8>> {
    match compression {
        1 => palmdoc::decompress(data),
        2 => zlib_decompress(data),
        _ => Err(EruditioError::Unsupported(format!(
            "Unknown Plucker compression type: {}",
            compression
        ))),
    }
}

/// Processes Plucker PHTML binary markup into HTML.
///
/// PHTML uses escape sequences (0x00 prefix byte) for formatting, links,
/// images, and structural elements.
fn process_phtml(data: &[u8], paragraph_offsets: &[usize]) -> String {
    let mut html = String::with_capacity(data.len() * 2);
    html.push_str("<p>");
    let mut offset = 0usize;
    let mut paragraph_open = true;
    let mut link_open = false;
    let mut p_num = 1u32;
    let mut font_close = "";

    while offset < data.len() {
        if !paragraph_open {
            html.push_str(&format!("<p id=\"p{}\">", p_num));
            p_num += 1;
            paragraph_open = true;
        }

        let c = data[offset];

        if c == 0x00 && offset + 1 < data.len() {
            offset += 1;
            let func = data[offset];
            match func {
                // Page link begins (2 bytes: record ID).
                0x0a => {
                    if offset + 2 < data.len() {
                        offset += 1;
                        let uid = read_u16_be(data, offset);
                        html.push_str(&format!("<a href=\"{}.html\">", uid));
                        link_open = true;
                        offset += 1;
                    }
                }
                // Targeted page link (3 bytes).
                0x0b => {
                    offset += 3;
                }
                // Paragraph link (4 bytes: record ID + paragraph number).
                0x0c => {
                    if offset + 4 < data.len() {
                        offset += 1;
                        let uid = read_u16_be(data, offset);
                        offset += 2;
                        let pid = read_u16_be(data, offset);
                        html.push_str(&format!("<a href=\"{}.html#p{}\">", uid, pid));
                        link_open = true;
                        offset += 1;
                    }
                }
                // Targeted paragraph link (5 bytes).
                0x0d => {
                    offset += 5;
                }
                // Link ends.
                0x08 => {
                    if link_open {
                        html.push_str("</a>");
                        link_open = false;
                    }
                }
                // Set font (1 byte: specifier).
                0x11 => {
                    offset += 1;
                    if offset < data.len() {
                        html.push_str(font_close);
                        let specifier = data[offset];
                        font_close = match specifier {
                            0 => "",
                            1 => { html.push_str("<h1>"); "</h1>" }
                            2 => { html.push_str("<h2>"); "</h2>" }
                            3 => { html.push_str("<h3>"); "</h3>" }
                            4 => { html.push_str("<h4>"); "</h4>" }
                            5 => { html.push_str("<h5>"); "</h5>" }
                            6 => { html.push_str("<h6>"); "</h6>" }
                            7 => { html.push_str("<b>"); "</b>" }
                            8 => { html.push_str("<tt>"); "</tt>" }
                            9 => { html.push_str("<small>"); "</small>" }
                            10 => { html.push_str("<sub>"); "</sub>" }
                            11 => { html.push_str("<sup>"); "</sup>" }
                            _ => "",
                        };
                    }
                }
                // Embedded image (2 bytes: image record UID).
                0x1a => {
                    if offset + 2 < data.len() {
                        offset += 1;
                        let uid = read_u16_be(data, offset);
                        html.push_str(&format!("<img src=\"images/{}.jpg\" />", uid));
                        offset += 1;
                    }
                }
                // Set margin (2 bytes).
                0x22 => {
                    offset += 2;
                }
                // Alignment (1 byte).
                0x29 => {
                    offset += 1;
                }
                // Horizontal rule (3 bytes).
                0x33 => {
                    offset += 3;
                    if paragraph_open {
                        html.push_str("</p>");
                        paragraph_open = false;
                    }
                    html.push_str("<hr />");
                }
                // New line.
                0x38 => {
                    if paragraph_open {
                        html.push_str("</p>\n");
                        paragraph_open = false;
                    }
                }
                // Italic begins.
                0x40 => html.push_str("<i>"),
                // Italic ends.
                0x48 => html.push_str("</i>"),
                // Set text color (3 bytes).
                0x53 => {
                    offset += 3;
                }
                // Multiple embedded image (4 bytes).
                0x5c => {
                    if offset + 4 < data.len() {
                        offset += 3;
                        let uid = read_u16_be(data, offset);
                        html.push_str(&format!("<img src=\"images/{}.jpg\" />", uid));
                        offset += 1;
                    }
                }
                // Underline begins.
                0x60 => html.push_str("<u>"),
                // Underline ends.
                0x68 => html.push_str("</u>"),
                // Strikethrough begins.
                0x70 => html.push_str("<s>"),
                // Strikethrough ends.
                0x78 => html.push_str("</s>"),
                // 16-bit Unicode character (3 bytes).
                0x83 => {
                    offset += 3;
                }
                // 32-bit Unicode character (5 bytes).
                0x85 => {
                    offset += 5;
                }
                // Begin custom font span (6 bytes).
                0x8e => {
                    offset += 6;
                }
                // Adjust font glyph position (4 bytes).
                0x8c => {
                    offset += 4;
                }
                // Change font page (2 bytes).
                0x8a => {
                    offset += 2;
                }
                // End custom font span (0 bytes).
                0x88 => {}
                // Table-related functions.
                0x90 => {} // Begin table row
                0x92 => {
                    offset += 2;
                } // Insert table
                0x97 => {
                    offset += 7;
                } // Table cell
                // Exact link modifier (2 bytes).
                0x9a => {
                    offset += 2;
                }
                _ => {
                    // Unknown function code — skip.
                }
            }
        } else if c == 0xa0 {
            html.push_str("&nbsp;");
        } else if c == b'<' {
            html.push_str("&lt;");
        } else if c == b'>' {
            html.push_str("&gt;");
        } else if c == b'&' {
            html.push_str("&amp;");
        } else if c >= 0x20 || c == b'\t' {
            html.push(c as char);
        }

        offset += 1;

        // Check for paragraph boundaries.
        if paragraph_offsets.contains(&offset) && paragraph_open {
            html.push_str("</p>\n");
            paragraph_open = false;
        }
    }

    if paragraph_open {
        html.push_str("</p>");
    }

    html
}

// ---------------------------------------------------------------------------
// Shared utilities
// ---------------------------------------------------------------------------

/// Converts plain text to simple HTML paragraphs.
fn text_to_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + text.len() / 4);

    for paragraph in text.split("\n\n") {
        let trimmed = paragraph.trim();
        if !trimmed.is_empty() {
            html.push_str("<p>");
            html.push_str(
                &crate::formats::common::text_utils::escape_html(trimmed)
                    .replace('\n', "<br />"),
            );
            html.push_str("</p>\n");
        }
    }

    if html.is_empty() {
        html.push_str("<p></p>");
    }

    html
}

/// Decodes CP-1252 bytes to UTF-8.
fn decode_cp1252(data: &[u8]) -> String {
    crate::formats::common::text_utils::decode_cp1252(data)
}

/// Detects image media type from magic bytes.
fn detect_image_media_type(data: &[u8]) -> &'static str {
    if data.len() >= 3 && data[0..3] == [0xFF, 0xD8, 0xFF] {
        "image/jpeg"
    } else if data.len() >= 8 && data[0..8] == *b"\x89PNG\r\n\x1a\n" {
        "image/png"
    } else if data.len() >= 4 && data[0..4] == *b"GIF8" {
        "image/gif"
    } else if data.len() >= 2 && data[0..2] == *b"BM" {
        "image/bmp"
    } else {
        "application/octet-stream"
    }
}

/// Maps a media type to a file extension.
fn media_type_to_ext(media_type: &str) -> &str {
    match media_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "bin",
    }
}

/// Strips HTML tags from a string, returning plain text.
fn strip_html(html: &str) -> String {
    let stripped = crate::formats::common::text_utils::strip_tags(html);
    crate::formats::common::text_utils::unescape_basic_entities(&stripped)
}

// ---------------------------------------------------------------------------
// Haodoo (`BOOKMTIT` / `BOOKMTIU`)
// ---------------------------------------------------------------------------

/// Reads a Haodoo PDB file.
///
/// Haodoo uses two variants:
/// - `BOOKMTIT` (legacy): Big5/CP950 encoding
/// - `BOOKMTIU` (unicode): UTF-16LE encoding
fn read_haodoo(pdb: &PdbFile, is_unicode: bool) -> Result<Book> {
    if pdb.record_count() < 1 {
        return Err(EruditioError::Format("Haodoo file has no records".into()));
    }

    // Record 0 is the header with metadata and chapter titles.
    let header_rec = pdb.record_data(0)?;
    let header_text = if is_unicode {
        decode_haodoo_unicode(header_rec)
    } else {
        decode_haodoo_big5(header_rec)
    };

    // Fields are separated by ESC (0x1B).
    let fields: Vec<&str> = header_text.split('\x1b').collect();

    let title = fields.first().map(|s| s.trim().to_string());
    // Field 1 is typically the number of text records.
    let _num_records = fields
        .get(1)
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);

    // Remaining fields are chapter titles.
    let chapter_titles: Vec<String> = fields.iter().skip(2).map(|s| s.trim().to_string()).collect();

    let mut book = Book::new();
    book.metadata.title = title;
    book.metadata.language = Some("zh".into());

    // Records 1..N are chapter text.
    let num_text_records = (pdb.record_count() - 1).min(chapter_titles.len().max(1));

    for i in 0..num_text_records {
        let rec_idx = i + 1;
        if rec_idx >= pdb.record_count() {
            break;
        }
        let record = pdb.record_data(rec_idx)?;
        let text = if is_unicode {
            decode_haodoo_unicode(record)
        } else {
            decode_haodoo_big5(record)
        };

        let ch_title = chapter_titles.get(i).cloned();
        let html = haodoo_text_to_html(&text);

        book.add_chapter(&Chapter {
            title: ch_title,
            content: html,
            id: Some(format!("haodoo_ch_{}", i)),
        });
    }

    if book.chapters().is_empty() {
        book.add_chapter(&Chapter {
            title: book.metadata.title.clone(),
            content: "<p></p>".into(),
            id: Some("haodoo_empty".into()),
        });
    }

    Ok(book)
}

/// Decodes Big5 (CP950) encoded bytes to UTF-8.
fn decode_haodoo_big5(data: &[u8]) -> String {
    let (text, _, _) = encoding_rs::BIG5.decode(data);
    text.into_owned()
}

/// Decodes UTF-16LE encoded bytes to UTF-8.
fn decode_haodoo_unicode(data: &[u8]) -> String {
    let u16s: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
}

/// Converts Haodoo plain text to HTML paragraphs.
fn haodoo_text_to_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + text.len() / 4);
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            html.push_str("<p>");
            html.push_str(
                &crate::formats::common::text_utils::escape_html(trimmed),
            );
            html.push_str("</p>\n");
        }
    }
    if html.is_empty() {
        html.push_str("<p></p>");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::common::compression::palmdoc::compress;
    use crate::formats::common::palm_db::{build_pdb_header, write_u16_be, write_u32_be};
    use std::io::Cursor;

    /// Builds a minimal PalmDOC PDB file for testing.
    fn build_palmdoc_pdb(text: &str, use_compression: bool) -> Vec<u8> {
        let max_record_size = 4096usize;
        let text_bytes = text.as_bytes();

        let mut records: Vec<Vec<u8>> = Vec::new();
        let mut offset = 0;
        while offset < text_bytes.len() {
            let end = (offset + max_record_size).min(text_bytes.len());
            let chunk = &text_bytes[offset..end];
            if use_compression {
                records.push(compress(chunk));
            } else {
                records.push(chunk.to_vec());
            }
            offset = end;
        }

        if records.is_empty() {
            records.push(Vec::new());
        }

        let mut header_rec = vec![0u8; 16];
        let comp = if use_compression { 2u16 } else { 1u16 };
        write_u16_be(&mut header_rec, 0, comp);
        write_u32_be(&mut header_rec, 4, text_bytes.len() as u32);
        write_u16_be(&mut header_rec, 8, records.len() as u16);
        write_u16_be(&mut header_rec, 10, max_record_size as u16);

        let total_records = 1 + records.len();
        let header_size = 78 + total_records * 8 + 2;

        let mut offsets = Vec::with_capacity(total_records);
        let mut pos = header_size as u32;
        offsets.push(pos);
        pos += header_rec.len() as u32;
        for rec in &records {
            offsets.push(pos);
            pos += rec.len() as u32;
        }

        let mut data =
            build_pdb_header("Test PalmDOC", b"TEXt", b"REAd", total_records as u16, &offsets);

        data.extend_from_slice(&header_rec);
        for rec in &records {
            data.extend_from_slice(rec);
        }

        data
    }

    /// Builds a minimal zTXT PDB file for testing.
    fn build_ztxt_pdb(text: &str) -> Vec<u8> {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let text_bytes = text.as_bytes();

        // Compress each chunk with zlib.
        let max_record_size = 4096usize;
        let mut records: Vec<Vec<u8>> = Vec::new();
        let mut offset = 0;
        while offset < text_bytes.len() {
            let end = (offset + max_record_size).min(text_bytes.len());
            let chunk = &text_bytes[offset..end];
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(chunk).unwrap();
            records.push(encoder.finish().unwrap());
            offset = end;
        }
        if records.is_empty() {
            let encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            records.push(encoder.finish().unwrap());
        }

        // Build zTXT header record (>= 19 bytes).
        let mut header_rec = vec![0u8; 20];
        // Version 1.44 = 0x012C.
        write_u16_be(&mut header_rec, 0, 0x012C);
        // Number of text records.
        write_u16_be(&mut header_rec, 2, records.len() as u16);
        // Uncompressed size.
        write_u32_be(&mut header_rec, 4, text_bytes.len() as u32);
        // Max record size.
        write_u16_be(&mut header_rec, 8, max_record_size as u16);
        // Flags: bit 0 = random access.
        header_rec[18] = 0x01;

        let total_records = 1 + records.len();
        let header_size = 78 + total_records * 8 + 2;

        let mut offsets = Vec::with_capacity(total_records);
        let mut pos = header_size as u32;
        offsets.push(pos);
        pos += header_rec.len() as u32;
        for rec in &records {
            offsets.push(pos);
            pos += rec.len() as u32;
        }

        let mut data =
            build_pdb_header("Test zTXT", b"zTXT", b"GPlm", total_records as u16, &offsets);
        data.extend_from_slice(&header_rec);
        for rec in &records {
            data.extend_from_slice(rec);
        }

        data
    }

    #[test]
    fn reads_uncompressed_palmdoc() {
        let text = "Hello, PalmDOC world!\n\nSecond paragraph.";
        let data = build_palmdoc_pdb(text, false);

        let mut cursor = Cursor::new(data);
        let book = PdbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test PalmDOC"));
        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains("Hello, PalmDOC world!"));
        assert!(content.contains("Second paragraph"));
    }

    #[test]
    fn reads_compressed_palmdoc() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let data = build_palmdoc_pdb(&text, true);

        let mut cursor = Cursor::new(data);
        let book = PdbReader::new().read_book(&mut cursor).unwrap();

        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains("quick brown fox"));
    }

    #[test]
    fn reads_ztxt_pdb() {
        let text = "Hello, zTXT world!\n\nSecond paragraph with more text.";
        let data = build_ztxt_pdb(text);

        let mut cursor = Cursor::new(data);
        let book = PdbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Test zTXT"));
        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains("Hello, zTXT world!"));
        assert!(content.contains("Second paragraph"));
    }

    #[test]
    fn rejects_mobi_identity() {
        let data = build_pdb_header("MOBI Book", b"BOOK", b"MOBI", 1, &[88]);
        let mut full = data;
        full.extend_from_slice(&[0u8; 16]);

        let mut cursor = Cursor::new(full);
        let result = PdbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("MOBI") || msg.contains("MobiReader"));
    }

    #[test]
    fn rejects_unknown_identity() {
        let data = build_pdb_header("Unknown", b"UNKN", b"OWNR", 1, &[88]);
        let mut full = data;
        full.extend_from_slice(&[0u8; 16]);

        let mut cursor = Cursor::new(full);
        let result = PdbReader::new().read_book(&mut cursor);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Unknown PDB subtype"));
    }

    #[test]
    fn text_to_html_wraps_paragraphs() {
        let html = text_to_html("First paragraph\n\nSecond paragraph");
        assert!(html.contains("<p>First paragraph</p>"));
        assert!(html.contains("<p>Second paragraph</p>"));
    }

    #[test]
    fn text_to_html_escapes_special_chars() {
        let html = text_to_html("A & B < C > D");
        assert!(html.contains("A &amp; B &lt; C &gt; D"));
    }

    #[test]
    fn plucker_phtml_basic_text() {
        let data = b"Hello world";
        let html = process_phtml(data, &[]);
        assert!(html.contains("Hello world"));
    }

    #[test]
    fn plucker_phtml_italic() {
        let mut data = Vec::new();
        data.push(0x00);
        data.push(0x40); // italic on
        data.extend_from_slice(b"text");
        data.push(0x00);
        data.push(0x48); // italic off
        let html = process_phtml(&data, &[]);
        assert!(html.contains("<i>text</i>"));
    }

    #[test]
    fn plucker_phtml_horizontal_rule() {
        let mut data = Vec::new();
        data.extend_from_slice(b"before");
        data.push(0x00);
        data.push(0x33); // hr
        data.extend_from_slice(&[0, 0, 0]); // 3 bytes params
        data.extend_from_slice(b"after");
        let html = process_phtml(&data, &[]);
        assert!(html.contains("<hr />"));
    }

    #[test]
    fn decode_cp1252_smart_quotes() {
        let input = &[0x93, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x94];
        let result = decode_cp1252(input);
        assert_eq!(result, "\u{201C}Hello\u{201D}");
    }

    #[test]
    fn detect_jpeg_magic() {
        assert_eq!(
            detect_image_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            "image/jpeg"
        );
    }

    #[test]
    fn detect_png_magic() {
        assert_eq!(
            detect_image_media_type(b"\x89PNG\r\n\x1a\nmore"),
            "image/png"
        );
    }

    #[test]
    fn pdb_writer_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("PDB Write Test".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello PalmDOC world!</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        PdbWriter::new().write_book(&book, &mut output).unwrap();

        // Read it back.
        let mut cursor = Cursor::new(output);
        let decoded = PdbReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("PDB Write Test"));
        let content: String = decoded.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains("Hello PalmDOC world!"));
    }

    #[test]
    fn strip_html_basic() {
        assert_eq!(strip_html("<p>Hello <b>world</b></p>"), "Hello world");
        assert_eq!(strip_html("No tags"), "No tags");
        assert_eq!(strip_html("&amp; &lt; &gt;"), "& < >");
    }
}
