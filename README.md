# Eruditio

A safe, fast, and production-ready Rust library for parsing and generating ebook files across 30+ formats. Eruditio provides a unified API to read, transform, and write ebooks regardless of their underlying format.

## Supported Formats

### Reading (25 formats)

| Category | Formats | Notes |
|----------|---------|-------|
| **EPUB** | `.epub` | EPUB 2/3 with full metadata, spine, manifest, and TOC |
| **Kindle** | `.mobi`, `.azw`, `.azw3`, `.prc` | MOBI/KF8 with EXTH metadata extraction |
| **FictionBook** | `.fb2`, `.fbz` | XML-based FictionBook 2.0; FBZ is ZIP-compressed FB2 |
| **Comic Books** | `.cbz`, `.cb7`, `.cbr`, `.cbc` | ZIP, 7z, RAR archives; CBC supports multi-comic collections |
| **Plain Text** | `.txt`, `.txtz`, `.tcr` | Raw text, ZIP-compressed text, TCR-compressed text |
| **Rich Text** | `.rtf` | RTF with style preservation |
| **HTML** | `.html`, `.htm`, `.xhtml`, `.htmlz` | Single-file HTML; HTMLZ is ZIP-bundled HTML with resources |
| **Kobo** | `.kepub`, `.kepub.epub` | Kobo-enhanced EPUB |
| **Palm** | `.pdb` | Palm Database with PalmDOC and Plucker support |
| **eReader** | `.pml`, `.pmlz` | Palm Markup Language; PMLZ is ZIP-compressed PML |
| **Rocket eBook** | `.rb` | NuvoMedia Rocket eBook |
| **Sony BBeB** | `.lrf` | Sony Broadband eBook with LRF object stream parsing |
| **Shanda Bambook** | `.snb` | Shanda Bambook proprietary format |
| **DjVu** | `.djvu`, `.djv` | Text layer extraction via IFF85 chunk parsing + BZZ decompression |
| **CHM** | `.chm` | Microsoft Compiled HTML Help via ITSS container + LZX decompression |
| **LIT** | `.lit` | Microsoft Reader with binary-to-HTML conversion; DRM level 1/3 decryption |

### Writing (15 formats)

| Format | Extensions |
|--------|-----------|
| EPUB | `.epub` |
| MOBI | `.mobi` |
| FB2 / FBZ | `.fb2`, `.fbz` |
| CBZ | `.cbz` |
| TXT / TXTZ | `.txt`, `.txtz` |
| TCR | `.tcr` |
| RTF | `.rtf` |
| HTML / HTMLZ | `.html`, `.htmlz` |
| Kepub | `.kepub` |
| PML / PMLZ | `.pml`, `.pmlz` |
| PDF | `.pdf` |

### Planned

| Format | Status |
|--------|--------|
| PDF reading | Stub (returns unsupported error) |
| DOCX | Planned |
| ODT | Planned |

## Usage

Add `eruditio` to your `Cargo.toml`:

```toml
[dependencies]
eruditio = "0.1.0"
```

### Basic Example

```rust
use eruditio::EruditioParser;

fn main() -> Result<(), eruditio::EruditioError> {
    // Parse an ebook file, automatically detecting format by extension
    let book = EruditioParser::parse_file("path/to/book.epub")?;

    // Access metadata
    println!("Title: {:?}", book.metadata.title);
    println!("Authors: {:?}", book.metadata.authors);

    // Iterate chapters
    for chapter in book.chapters() {
        println!("Chapter: {:?}", chapter.title);
    }

    Ok(())
}
```

### Format Detection

```rust
use eruditio::domain::Format;

// By extension
let fmt = Format::from_extension("djvu"); // Some(Format::Djvu)

// By magic bytes
let fmt = Format::from_magic_bytes(b"AT&TFORM..."); // Some(Format::Djvu)
let fmt = Format::from_magic_bytes(b"ITOLITLS..."); // Some(Format::Lit)
let fmt = Format::from_magic_bytes(b"ITSF...."); // Some(Format::Chm)
```

### Read from a Stream

```rust
use eruditio::EruditioParser;
use std::io::Cursor;

let data = std::fs::read("book.fb2").unwrap();
let mut cursor = Cursor::new(data);
let book = EruditioParser::parse(&mut cursor, Some("fb2"))?;
```

## Architecture

```
src/
├── domain/          Core models (Book, Chapter, Metadata, Resource, Format)
├── formats/         Format-specific readers and writers
│   ├── common/      Shared utilities (XML, HTML, ZIP, ITSS container parser)
│   ├── epub/        EPUB reader/writer
│   ├── mobi/        MOBI/KF8 reader/writer
│   ├── djvu/        DjVu reader (IFF85 parser + BZZ decompressor)
│   ├── chm/         CHM reader (ITSS + LZX decompression)
│   ├── lit/         LIT reader (ITSS + unbinary + MS DES/SHA1 for DRM)
│   ├── lrf/         Sony LRF reader
│   ├── html/        HTML reader/writer
│   ├── rtf/         RTF reader/writer
│   ├── pml/         PML reader/writer
│   └── ...          Other single-file format modules
├── transforms/      Book-to-Book transformation pipeline
├── pipeline/        Format registry and conversion orchestration
├── parser.rs        EruditioParser — unified entry point
├── error.rs         Error types (EruditioError)
└── lib.rs           Public API re-exports
```

Key design principles:

- **Immutability**: `Book` and all domain types are treated as immutable values. Transforms produce new `Book` instances.
- **Trait-based**: `FormatReader` and `FormatWriter` traits allow uniform handling across all formats.
- **Pure Rust**: No C/C++ dependencies for format parsing. BZZ, LZX, MS DES, and MS SHA-1 are implemented in pure Rust.
- **Stream-oriented**: All readers accept `&mut dyn Read`, enabling parsing from files, network streams, or in-memory buffers.

## Testing

```bash
# Run all tests (requires nightly for edition 2024)
cargo +nightly test

# Run tests for a specific format
cargo +nightly test djvu
cargo +nightly test lit
cargo +nightly test chm
```

The test suite includes 400+ tests covering unit tests within each format module and integration tests in `tests/`.

## License

This project is licensed under the MIT License.
