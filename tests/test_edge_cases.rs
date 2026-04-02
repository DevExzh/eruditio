//! Edge-case parser tests: empty input, garbage bytes, truncated headers.
//!
//! Every registered format reader must return `Err`, never panic, when
//! fed invalid data.

use eruditio::{ConversionOptions, Format, Pipeline};
use std::io::Cursor;

/// All formats that have readers registered in the default pipeline.
/// Excludes feature-gated formats (Cb7, Cbr) to avoid conditional compilation.
const READABLE_FORMATS: &[Format] = &[
    Format::Epub,
    Format::Fb2,
    Format::Cbz,
    Format::Cbc,
    Format::Djvu,
    Format::Chm,
    Format::Lit,
    Format::Txt,
    Format::Tcr,
    Format::Mobi,
    Format::Azw,
    Format::Azw3,
    Format::Prc,
    Format::Azw4,
    Format::Pdf,
    Format::Fbz,
    Format::Txtz,
    Format::Html,
    Format::Htmlz,
    Format::Rtf,
    Format::Kepub,
    Format::Lrf,
    Format::Pdb,
    Format::Pml,
    Format::Pmlz,
    Format::Rb,
    Format::Snb,
    Format::Md,
    Format::Oeb,
];

// ---------------------------------------------------------------------------
// Empty input
// ---------------------------------------------------------------------------

#[test]
fn all_readers_reject_empty_input() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();

    // Text-based formats legitimately accept empty input (an empty file
    // is a valid text/html/markdown document), so skip them here.
    const TEXT_FORMATS: &[Format] = &[
        Format::Txt,
        Format::Html,
        Format::Md,
        Format::Pml,
        Format::Kepub,
    ];

    for &fmt in READABLE_FORMATS {
        if TEXT_FORMATS.contains(&fmt) {
            continue;
        }
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let result = pipeline.read(fmt, &mut cursor, &opts);
        assert!(
            result.is_err(),
            "{:?} reader should reject empty input but returned Ok",
            fmt
        );
    }
}

// ---------------------------------------------------------------------------
// Garbage bytes (not valid for any format)
// ---------------------------------------------------------------------------

#[test]
fn all_readers_reject_garbage_input() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    let garbage: Vec<u8> = (0..256).map(|i| (i as u8).wrapping_mul(37)).collect();

    for &fmt in READABLE_FORMATS {
        let mut cursor = Cursor::new(garbage.clone());
        let result = pipeline.read(fmt, &mut cursor, &opts);
        // Text-based formats (Txt, Html, Md, Pml, Rtf) may accept any bytes
        // as valid input. Only check binary/structured formats.
        match fmt {
            Format::Txt | Format::Html | Format::Md | Format::Pml => continue,
            _ => {
                assert!(
                    result.is_err(),
                    "{:?} reader should reject garbage input but returned Ok",
                    fmt
                );
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Truncated headers for binary formats
// ---------------------------------------------------------------------------

#[test]
fn mobi_rejects_truncated_pdb_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // Valid PDB header is 78+ bytes; feed only 30.
    let short = vec![0u8; 30];
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Mobi, &mut cursor, &opts).is_err());
}

#[test]
fn lrf_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // LRF needs at least 0x58 bytes; feed only 0x20.
    let mut short = vec![0u8; 0x20];
    // Set valid magic so we get past magic check to the size check.
    short[0..6].copy_from_slice(&[0x4C, 0x00, 0x52, 0x00, 0x46, 0x00]);
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Lrf, &mut cursor, &opts).is_err());
}

#[test]
fn snb_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // SNB magic is "SNBP" at offset 0, needs full header.
    let mut short = vec![0u8; 10];
    short[0..4].copy_from_slice(b"SNBP");
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Snb, &mut cursor, &opts).is_err());
}

#[test]
fn rb_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // RB magic is "NUVO" at offset 0, full header is much larger.
    let mut short = vec![0u8; 16];
    short[0..4].copy_from_slice(b"NUVO");
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Rb, &mut cursor, &opts).is_err());
}

#[test]
fn djvu_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // DjVu starts with "AT&TFORM" but needs more data.
    let mut short = vec![0u8; 12];
    short[0..4].copy_from_slice(b"AT&T");
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Djvu, &mut cursor, &opts).is_err());
}

#[test]
fn chm_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // CHM/ITSF magic is "ITSF" but needs much more data.
    let mut short = vec![0u8; 20];
    short[0..4].copy_from_slice(b"ITSF");
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Chm, &mut cursor, &opts).is_err());
}

#[test]
fn lit_rejects_truncated_header() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // LIT magic is "ITOLITLS" but needs header + directory.
    let mut short = vec![0u8; 16];
    short[0..8].copy_from_slice(b"ITOLITLS");
    let mut cursor = Cursor::new(short);
    assert!(pipeline.read(Format::Lit, &mut cursor, &opts).is_err());
}

// ---------------------------------------------------------------------------
// ZIP-based formats with corrupt ZIP data
// ---------------------------------------------------------------------------

#[test]
fn zip_formats_reject_corrupt_zip() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // PK header but truncated — not a valid ZIP.
    let corrupt_zip = vec![0x50, 0x4B, 0x03, 0x04, 0xFF, 0xFF];

    let zip_formats = [
        Format::Epub,
        Format::Cbz,
        Format::Cbc,
        Format::Fbz,
        Format::Txtz,
        Format::Htmlz,
        Format::Pmlz,
        Format::Oeb,
    ];

    for &fmt in &zip_formats {
        let mut cursor = Cursor::new(corrupt_zip.clone());
        let result = pipeline.read(fmt, &mut cursor, &opts);
        assert!(
            result.is_err(),
            "{:?} reader should reject corrupt ZIP but returned Ok",
            fmt
        );
    }
}

// ---------------------------------------------------------------------------
// RTF with missing header
// ---------------------------------------------------------------------------

#[test]
fn rtf_rejects_non_rtf_content() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    let not_rtf = b"This is plain text, not RTF at all.";
    let mut cursor = Cursor::new(not_rtf.to_vec());
    assert!(pipeline.read(Format::Rtf, &mut cursor, &opts).is_err());
}

// ---------------------------------------------------------------------------
// TCR with invalid header
// ---------------------------------------------------------------------------

#[test]
fn tcr_rejects_invalid_magic() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    let not_tcr = b"NOT_TCR_MAGIC_HEADER_DATA";
    let mut cursor = Cursor::new(not_tcr.to_vec());
    assert!(pipeline.read(Format::Tcr, &mut cursor, &opts).is_err());
}

// ---------------------------------------------------------------------------
// PDB with valid header but wrong type identity
// ---------------------------------------------------------------------------

#[test]
fn pdb_rejects_mobi_typed_file() {
    let pipeline = Pipeline::new();
    let opts = ConversionOptions::none();
    // Build a minimal PDB header with MOBI identity — PdbReader should
    // reject it (MOBI has its own reader).
    let mut data = vec![0u8; 80];
    // type = "BOOK", creator = "MOBI" at offsets 60-67
    data[60..64].copy_from_slice(b"BOOK");
    data[64..68].copy_from_slice(b"MOBI");
    let mut cursor = Cursor::new(data);
    assert!(pipeline.read(Format::Pdb, &mut cursor, &opts).is_err());
}
