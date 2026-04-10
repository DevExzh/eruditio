//! Integration tests for Phase 7 read-only formats: PDB, RB, LRF, SNB.
//!
//! Each test builds a synthetic file, reads it through the Pipeline,
//! verifies metadata/content, and converts to EPUB.

use eruditio::{ConversionOptions, Format, Pipeline};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::{Cursor, Write};

// ---------------------------------------------------------------------------
// PDB (PalmDOC) helpers
// ---------------------------------------------------------------------------

const PDB_HEADER_SIZE: usize = 78;
const PDB_RECORD_ENTRY_SIZE: usize = 8;

fn write_u16_be(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32_be(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn build_pdb_header(
    name: &str,
    db_type: &[u8; 4],
    creator: &[u8; 4],
    num_records: u16,
    record_offsets: &[u32],
) -> Vec<u8> {
    let table_size = num_records as usize * PDB_RECORD_ENTRY_SIZE;
    let total = PDB_HEADER_SIZE + table_size + 2;
    let mut buf = vec![0u8; total];

    let name_bytes = name.as_bytes();
    let copy_len = name_bytes.len().min(31);
    buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    buf[60..64].copy_from_slice(db_type);
    buf[64..68].copy_from_slice(creator);
    write_u32_be(&mut buf, 68, num_records as u32);
    write_u16_be(&mut buf, 76, num_records);

    for (i, &offset) in record_offsets.iter().enumerate() {
        let base = PDB_HEADER_SIZE + i * PDB_RECORD_ENTRY_SIZE;
        write_u32_be(&mut buf, base, offset);
        buf[base + 5] = ((i >> 16) & 0xFF) as u8;
        buf[base + 6] = ((i >> 8) & 0xFF) as u8;
        buf[base + 7] = (i & 0xFF) as u8;
    }

    buf
}

/// Builds an uncompressed PalmDOC PDB file for testing.
fn build_palmdoc_pdb(title: &str, text: &str) -> Vec<u8> {
    let text_bytes = text.as_bytes();
    let max_rec = 4096usize;

    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut offset = 0;
    while offset < text_bytes.len() {
        let end = (offset + max_rec).min(text_bytes.len());
        records.push(text_bytes[offset..end].to_vec());
        offset = end;
    }
    if records.is_empty() {
        records.push(Vec::new());
    }

    // PalmDOC header record (record 0).
    let mut hdr_rec = vec![0u8; 16];
    write_u16_be(&mut hdr_rec, 0, 1); // compression: 1 = none
    write_u32_be(&mut hdr_rec, 4, text_bytes.len() as u32);
    write_u16_be(&mut hdr_rec, 8, records.len() as u16);
    write_u16_be(&mut hdr_rec, 10, max_rec as u16);

    let total_records = 1 + records.len();
    let header_size = PDB_HEADER_SIZE + total_records * PDB_RECORD_ENTRY_SIZE + 2;

    let mut offsets = Vec::with_capacity(total_records);
    let mut pos = header_size as u32;
    offsets.push(pos);
    pos += hdr_rec.len() as u32;
    for rec in &records {
        offsets.push(pos);
        pos += rec.len() as u32;
    }

    let mut data = build_pdb_header(title, b"TEXt", b"REAd", total_records as u16, &offsets);
    data.extend_from_slice(&hdr_rec);
    for rec in &records {
        data.extend_from_slice(rec);
    }
    data
}

// ---------------------------------------------------------------------------
// RB (RocketBook) helpers
// ---------------------------------------------------------------------------

const RB_HEADER_SIZE: usize = 0x128;
const RB_TOC_ENTRY_SIZE: usize = 44;
const RB_FLAG_RAW: u32 = 0;
const RB_FLAG_INFO: u32 = 2;

fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_u16_le(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}

fn build_rb_file(pages: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let total = pages.len();
    let toc_offset = RB_HEADER_SIZE;
    let toc_size = 4 + total * RB_TOC_ENTRY_SIZE;

    let entry_data: Vec<Vec<u8>> = pages.iter().map(|&(_, c, _)| c.to_vec()).collect();

    let data_start = toc_offset + toc_size;
    let mut offsets = Vec::new();
    let mut pos = data_start;
    for ed in &entry_data {
        offsets.push(pos as u32);
        pos += ed.len();
    }

    let mut file = vec![0u8; pos];
    // Header
    file[0..4].copy_from_slice(&[0xB0, 0x0C, 0xB0, 0x0C]);
    write_u16_le(&mut file, 4, 2);
    file[6..10].copy_from_slice(b"NUVO");
    write_u32_le(&mut file, 0x18, toc_offset as u32);
    write_u32_le(&mut file, 0x1C, pos as u32);

    // TOC
    write_u32_le(&mut file, toc_offset, total as u32);
    for (i, &(name, _, flags)) in pages.iter().enumerate() {
        let base = toc_offset + 4 + i * RB_TOC_ENTRY_SIZE;
        let nb = name.as_bytes();
        let len = nb.len().min(32);
        file[base..base + len].copy_from_slice(&nb[..len]);
        write_u32_le(&mut file, base + 32, entry_data[i].len() as u32);
        write_u32_le(&mut file, base + 36, offsets[i]);
        write_u32_le(&mut file, base + 40, flags);
    }

    // Data
    for (i, ed) in entry_data.iter().enumerate() {
        let off = offsets[i] as usize;
        file[off..off + ed.len()].copy_from_slice(ed);
    }

    file
}

// ---------------------------------------------------------------------------
// LRF helpers
// ---------------------------------------------------------------------------

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn build_lrf_object_bytes(obj_id: u32, obj_type: u16, stream: Option<&[u8]>) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0x00, 0xF5]);
    b.extend_from_slice(&obj_id.to_le_bytes());
    b.extend_from_slice(&obj_type.to_le_bytes());
    if let Some(s) = stream {
        b.extend_from_slice(&[0x54, 0xF5, 0x00, 0x00]); // StreamFlags (no compression)
        b.extend_from_slice(&[0x04, 0xF5]);
        b.extend_from_slice(&(s.len() as u32).to_le_bytes());
        b.extend_from_slice(&[0x05, 0xF5]); // StreamStart
        b.extend_from_slice(s);
        b.extend_from_slice(&[0x06, 0xF5]); // StreamEnd
    }
    b.extend_from_slice(&[0x01, 0xF5]); // ObjectEnd
    b
}

fn build_lrf_object_with_contained(obj_id: u32, obj_type: u16, ids: &[u32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0x00, 0xF5]);
    b.extend_from_slice(&obj_id.to_le_bytes());
    b.extend_from_slice(&obj_type.to_le_bytes());
    b.extend_from_slice(&[0x0B, 0xF5]);
    b.extend_from_slice(&(ids.len() as u16).to_le_bytes());
    for &id in ids {
        b.extend_from_slice(&id.to_le_bytes());
    }
    b.extend_from_slice(&[0x01, 0xF5]);
    b
}

fn build_minimal_lrf(title: &str, author: &str, text_content: &str) -> Vec<u8> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Info version=\"1.1\">\n\
         <BookInfo><Title>{}</Title><Author>{}</Author></BookInfo>\n\
         <DocInfo><Language>en</Language></DocInfo>\n\
         </Info>",
        title, author
    );
    let compressed_xml = zlib_compress(xml.as_bytes());
    let compressed_info_size = (compressed_xml.len() + 4) as u16;

    // Text stream: P_START + UTF-16LE text + P_END
    let text_utf16: Vec<u8> = text_content
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    let mut text_stream = Vec::new();
    text_stream.extend_from_slice(&[0xA1, 0xF5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    text_stream.extend_from_slice(&text_utf16);
    text_stream.extend_from_slice(&[0xA2, 0xF5]);

    // Block stream: Link tag → Text obj 10
    let mut block_stream = Vec::new();
    block_stream.extend_from_slice(&[0x03, 0xF5]);
    block_stream.extend_from_slice(&10u32.to_le_bytes());

    let text_obj = build_lrf_object_bytes(10, 0x0A, Some(&text_stream));
    let block_obj = build_lrf_object_bytes(5, 0x06, Some(&block_stream));
    let page_obj = build_lrf_object_with_contained(2, 0x02, &[5]);
    let ptree_obj = build_lrf_object_with_contained(1, 0x01, &[2]);

    let info_start = 0x58usize;
    let objects_start = info_start + compressed_xml.len();
    let obj_data = [&ptree_obj[..], &page_obj[..], &block_obj[..], &text_obj[..]];
    let obj_ids: [u32; 4] = [1, 2, 5, 10];

    let mut obj_offsets = Vec::new();
    let mut pos = objects_start;
    for od in &obj_data {
        obj_offsets.push(pos);
        pos += od.len();
    }
    let obj_index_offset = pos;
    let total_size = obj_index_offset + 4 * 16;

    let mut file = vec![0u8; total_size];
    file[0..6].copy_from_slice(&[0x4C, 0x00, 0x52, 0x00, 0x46, 0x00]);
    file[0x08..0x0A].copy_from_slice(&1000u16.to_le_bytes());
    file[0x0C..0x10].copy_from_slice(&1u32.to_le_bytes());
    file[0x10..0x18].copy_from_slice(&4u64.to_le_bytes());
    file[0x18..0x20].copy_from_slice(&(obj_index_offset as u64).to_le_bytes());
    file[0x24] = 1;
    file[0x26..0x28].copy_from_slice(&166u16.to_le_bytes());
    file[0x2A..0x2C].copy_from_slice(&600u16.to_le_bytes());
    file[0x2C..0x2E].copy_from_slice(&775u16.to_le_bytes());
    file[0x2E] = 24;
    file[0x4C..0x4E].copy_from_slice(&compressed_info_size.to_le_bytes());

    file[info_start..info_start + compressed_xml.len()].copy_from_slice(&compressed_xml);

    for (i, od) in obj_data.iter().enumerate() {
        file[obj_offsets[i]..obj_offsets[i] + od.len()].copy_from_slice(od);
    }

    for i in 0..4 {
        let base = obj_index_offset + i * 16;
        file[base..base + 4].copy_from_slice(&obj_ids[i].to_le_bytes());
        file[base + 4..base + 8].copy_from_slice(&(obj_offsets[i] as u32).to_le_bytes());
        file[base + 8..base + 12].copy_from_slice(&(obj_data[i].len() as u32).to_le_bytes());
    }

    file
}

// ---------------------------------------------------------------------------
// SNB helpers
// ---------------------------------------------------------------------------

fn bz2_compress(data: &[u8]) -> Vec<u8> {
    use bzip2::Compression as BzCompression;
    use bzip2::write::BzEncoder;
    let mut enc = BzEncoder::new(Vec::new(), BzCompression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn write_i32_be(buf: &mut [u8], offset: usize, val: i32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_be_bytes());
}

fn build_snb_file(title: &str, author: &str, chapters: &[(&str, &str)]) -> Vec<u8> {
    let snb_magic: &[u8; 8] = b"SNBP000B";
    let header_size = 0x2C_usize;
    let attr_plain: u32 = 0x41000000;

    let meta_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <book-snbf version=\"1.0\">\n\
         <head><name>{}</name><author>{}</author><language>en</language></head>\n\
         </book-snbf>",
        title, author
    );

    let mut toc_xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<toc-snbf>\n");
    for (i, (ch_title, _)) in chapters.iter().enumerate() {
        toc_xml.push_str(&format!(
            "  <chapter src=\"ch{}.snbc\">{}</chapter>\n",
            i, ch_title
        ));
    }
    toc_xml.push_str("</toc-snbf>");

    let mut all_files: Vec<(String, Vec<u8>)> = vec![
        ("snbf/book.snbf".into(), meta_xml.into_bytes()),
        ("snbf/toc.snbf".into(), toc_xml.into_bytes()),
    ];
    for (i, (ch_title, ch_text)) in chapters.iter().enumerate() {
        let snbc = format!(
            "<snbc><head><title>{}</title></head><body><text>{}</text></body></snbc>",
            ch_title, ch_text
        );
        all_files.push((format!("snbc/ch{}.snbc", i), snbc.into_bytes()));
    }

    let file_count = all_files.len();

    // Build VFAT
    let mut vfat = Vec::new();
    let mut name_table = Vec::new();
    for (name, data) in &all_files {
        let name_offset = name_table.len() as u32;
        vfat.extend_from_slice(&attr_plain.to_be_bytes());
        vfat.extend_from_slice(&name_offset.to_be_bytes());
        vfat.extend_from_slice(&(data.len() as u32).to_be_bytes());
        name_table.extend_from_slice(name.as_bytes());
        name_table.push(0);
    }
    vfat.extend_from_slice(&name_table);
    let vfat_uncompressed = vfat.len();
    let vfat_compressed = zlib_compress(&vfat);

    // Build plain stream (all files in one bz2 block)
    let mut plain_raw = Vec::new();
    let mut file_offsets: Vec<usize> = Vec::new();
    for (_, data) in &all_files {
        file_offsets.push(plain_raw.len());
        plain_raw.extend_from_slice(data);
    }
    let plain_compressed = bz2_compress(&plain_raw);

    let plain_start = header_size + vfat_compressed.len();

    // Build tail block
    let mut tail_data = Vec::new();
    tail_data.extend_from_slice(&0i32.to_be_bytes()); // one block at offset 0
    for off in &file_offsets {
        tail_data.extend_from_slice(&0i32.to_be_bytes()); // block_index = 0
        tail_data.extend_from_slice(&(*off as i32).to_be_bytes());
    }
    let tail_compressed = zlib_compress(&tail_data);
    let tail_offset = plain_start + plain_compressed.len();

    let total_size = tail_offset + tail_compressed.len() + 16;
    let mut file = vec![0u8; total_size];

    // Header
    file[0..8].copy_from_slice(snb_magic);
    write_i32_be(&mut file, 0x08, 0x00008000);
    write_i32_be(&mut file, 0x0C, 0x00A3A3A3);
    write_i32_be(&mut file, 0x14, file_count as i32);
    write_i32_be(&mut file, 0x18, vfat_uncompressed as i32);
    write_i32_be(&mut file, 0x1C, vfat_compressed.len() as i32);
    write_i32_be(&mut file, 0x20, 0); // no binary stream
    write_i32_be(&mut file, 0x24, plain_raw.len() as i32);

    // VFAT
    file[header_size..header_size + vfat_compressed.len()].copy_from_slice(&vfat_compressed);

    // Plain stream
    file[plain_start..plain_start + plain_compressed.len()].copy_from_slice(&plain_compressed);

    // Tail
    file[tail_offset..tail_offset + tail_compressed.len()].copy_from_slice(&tail_compressed);

    // Tail footer
    let footer = tail_offset + tail_compressed.len();
    write_i32_be(&mut file, footer, tail_compressed.len() as i32);
    write_i32_be(&mut file, footer + 4, tail_offset as i32);
    file[footer + 8..footer + 16].copy_from_slice(snb_magic);

    file
}

// ===========================================================================
// Tests
// ===========================================================================

#[test]
fn pdb_read_through_pipeline() {
    let pipeline = Pipeline::new();
    let data = build_palmdoc_pdb("PDB Pipeline Book", "Hello from PDB reader!");

    let mut cursor = Cursor::new(data);
    let book = pipeline
        .read(Format::Pdb, &mut cursor, &ConversionOptions::none())
        .expect("read PDB through pipeline");

    assert_eq!(book.metadata.title.as_deref(), Some("PDB Pipeline Book"));
    let content: String = book.chapter_views().iter().map(|c| c.content).collect();
    assert!(content.contains("Hello from PDB reader!"));
}

#[test]
fn pdb_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let data = build_palmdoc_pdb("PDB to EPUB", "PDB content for conversion.");

    let mut input = Cursor::new(data);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Pdb,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert PDB → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("PDB to EPUB"));

    // Verify EPUB output is readable.
    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB from PDB");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn rb_read_through_pipeline() {
    let pipeline = Pipeline::new();
    let html = b"<html><body><p>Hello from RocketBook!</p></body></html>";
    let info = b"TITLE=RB Pipeline Book\nAUTHOR=RB Author\nBODY=page.html";
    let data = build_rb_file(&[
        ("info", info, RB_FLAG_INFO),
        ("page.html", html, RB_FLAG_RAW),
    ]);

    let mut cursor = Cursor::new(data);
    let book = pipeline
        .read(Format::Rb, &mut cursor, &ConversionOptions::none())
        .expect("read RB through pipeline");

    assert_eq!(book.metadata.title.as_deref(), Some("RB Pipeline Book"));
    assert_eq!(book.metadata.authors, vec!["RB Author"]);
    let content: String = book.chapter_views().iter().map(|c| c.content).collect();
    assert!(content.contains("Hello from RocketBook!"));
}

#[test]
fn rb_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let html = b"<html><body><p>RB content for EPUB.</p></body></html>";
    let info = b"TITLE=RB to EPUB\nAUTHOR=Author\nBODY=ch.html";
    let data = build_rb_file(&[("info", info, RB_FLAG_INFO), ("ch.html", html, RB_FLAG_RAW)]);

    let mut input = Cursor::new(data);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Rb,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert RB → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("RB to EPUB"));

    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB from RB");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn lrf_read_through_pipeline() {
    let pipeline = Pipeline::new();
    let data = build_minimal_lrf("LRF Pipeline Book", "LRF Author", "Hello from LRF reader!");

    let mut cursor = Cursor::new(data);
    let book = pipeline
        .read(Format::Lrf, &mut cursor, &ConversionOptions::none())
        .expect("read LRF through pipeline");

    assert_eq!(book.metadata.title.as_deref(), Some("LRF Pipeline Book"));
    assert_eq!(book.metadata.authors, vec!["LRF Author"]);
    let content: String = book.chapter_views().iter().map(|c| c.content).collect();
    assert!(content.contains("Hello from LRF reader!"));
}

#[test]
fn lrf_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let data = build_minimal_lrf("LRF to EPUB", "LRF Writer", "LRF text for conversion.");

    let mut input = Cursor::new(data);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Lrf,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert LRF → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("LRF to EPUB"));

    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB from LRF");
    assert!(!decoded.chapters().is_empty());
}

#[test]
fn snb_read_through_pipeline() {
    let pipeline = Pipeline::new();
    let data = build_snb_file(
        "SNB Pipeline Book",
        "SNB Author",
        &[("Chapter One", "Hello from SNB reader!")],
    );

    let mut cursor = Cursor::new(data);
    let book = pipeline
        .read(Format::Snb, &mut cursor, &ConversionOptions::none())
        .expect("read SNB through pipeline");

    assert_eq!(book.metadata.title.as_deref(), Some("SNB Pipeline Book"));
    assert_eq!(book.metadata.authors, vec!["SNB Author"]);
    let content: String = book.chapter_views().iter().map(|c| c.content).collect();
    assert!(content.contains("Hello from SNB reader!"));
}

#[test]
fn snb_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let data = build_snb_file(
        "SNB to EPUB",
        "SNB Writer",
        &[
            ("First", "SNB first chapter."),
            ("Second", "SNB second chapter."),
        ],
    );

    let mut input = Cursor::new(data);
    let mut epub_buf = Vec::new();
    let result = pipeline
        .convert(
            Format::Snb,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert SNB → EPUB");

    assert_eq!(result.metadata.title.as_deref(), Some("SNB to EPUB"));

    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB from SNB");
    assert!(!decoded.chapters().is_empty());
}

// ---------------------------------------------------------------------------
// DJVU helpers
// ---------------------------------------------------------------------------

fn build_djvu_file(text: &str) -> Vec<u8> {
    let text_bytes = text.as_bytes();
    let text_len_bytes = [0u8, 0, text_bytes.len() as u8]; // 3-byte BE length
    let txta_data_len = 3 + text_bytes.len();

    let mut buf = Vec::new();
    buf.extend_from_slice(b"AT&T");
    buf.extend_from_slice(b"FORM");
    let form_size = 4 + 8 + txta_data_len; // subtype(4) + TXTa header(8) + data
    buf.extend_from_slice(&(form_size as u32).to_be_bytes());
    buf.extend_from_slice(b"DJVU");
    buf.extend_from_slice(b"TXTa");
    buf.extend_from_slice(&(txta_data_len as u32).to_be_bytes());
    buf.extend_from_slice(&text_len_bytes);
    buf.extend_from_slice(text_bytes);
    buf
}

// ===========================================================================
// DJVU Tests
// ===========================================================================

#[test]
fn djvu_read_through_pipeline() {
    let pipeline = Pipeline::new();
    let data = build_djvu_file("Hello from DJVU reader!");

    let mut cursor = Cursor::new(data);
    let book = pipeline
        .read(Format::Djvu, &mut cursor, &ConversionOptions::none())
        .expect("read DJVU through pipeline");

    let content: String = book.chapter_views().iter().map(|c| c.content).collect();
    assert!(content.contains("Hello from DJVU reader!"));
}

#[test]
fn djvu_to_epub_conversion() {
    let pipeline = Pipeline::new();
    let data = build_djvu_file("DJVU content for EPUB conversion.");

    let mut input = Cursor::new(data);
    let mut epub_buf = Vec::new();
    let _ = pipeline
        .convert(
            Format::Djvu,
            Format::Epub,
            &mut input,
            &mut epub_buf,
            &ConversionOptions::all(),
        )
        .expect("convert DJVU -> EPUB");

    let mut verify = Cursor::new(epub_buf);
    let decoded = pipeline
        .read(Format::Epub, &mut verify, &ConversionOptions::none())
        .expect("read back EPUB from DJVU");
    assert!(!decoded.chapters().is_empty());
}

// ===========================================================================
// CB7/CBR/CBC/CHM/LIT error handling tests
// ===========================================================================

#[test]
fn cb7_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a 7z archive".to_vec());
    let result = pipeline.read(Format::Cb7, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

#[test]
fn cbr_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a rar archive".to_vec());
    let result = pipeline.read(Format::Cbr, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

#[test]
fn cbc_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a zip archive".to_vec());
    let result = pipeline.read(Format::Cbc, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

#[test]
fn chm_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a CHM file at all".to_vec());
    let result = pipeline.read(Format::Chm, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

#[test]
fn lit_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a LIT file at all".to_vec());
    let result = pipeline.read(Format::Lit, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

#[test]
fn djvu_rejects_invalid_data_through_pipeline() {
    let pipeline = Pipeline::new();
    let mut cursor = Cursor::new(b"not a DJVU file".to_vec());
    let result = pipeline.read(Format::Djvu, &mut cursor, &ConversionOptions::none());
    assert!(result.is_err());
}

// ===========================================================================
// Registry check
// ===========================================================================

#[test]
fn phase7_formats_registered_in_registry() {
    let pipeline = Pipeline::new();
    let reg = pipeline.registry();
    // All Phase 7 formats should be readable.
    assert!(reg.can_read(&Format::Pdb));
    assert!(reg.can_read(&Format::Rb));
    assert!(reg.can_read(&Format::Lrf));
    assert!(reg.can_read(&Format::Snb));
    assert!(reg.can_read(&Format::Cb7));
    assert!(reg.can_read(&Format::Cbr));
    assert!(reg.can_read(&Format::Cbc));
    assert!(reg.can_read(&Format::Djvu));
    assert!(reg.can_read(&Format::Chm));
    assert!(reg.can_read(&Format::Lit));
    // PDB, RB, SNB now have writers.
    assert!(reg.can_write(&Format::Pdb));
    assert!(reg.can_write(&Format::Rb));
    assert!(reg.can_write(&Format::Snb));
    // LRF and LIT now have writers too.
    assert!(reg.can_write(&Format::Lrf));
    assert!(reg.can_write(&Format::Lit));
    // The rest remain read-only.
    assert!(!reg.can_write(&Format::Cb7));
    assert!(!reg.can_write(&Format::Cbr));
    assert!(!reg.can_write(&Format::Cbc));
    assert!(!reg.can_write(&Format::Djvu));
    assert!(!reg.can_write(&Format::Chm));
}
