//! RTF format reader and writer.
//!
//! Reads RTF documents into the `Book` intermediate representation and writes
//! books back as RTF documents. Handles control words, Unicode escapes,
//! hex-encoded characters, and basic formatting (bold, italic, headings).

pub mod tokenizer;
pub mod writer;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use std::io::{Read, Write};
use tokenizer::{RtfToken, tokenize};

/// RTF format reader.
///
/// Tokenizes the RTF stream, extracts metadata from the `\info` group,
/// and converts text content to HTML paragraphs with basic formatting.
#[derive(Default)]
pub struct RtfReader;

impl RtfReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for RtfReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut data = Vec::new();
        reader.read_to_end(&mut data)?;

        // Verify RTF magic.
        if !data.starts_with(b"{\\rtf") {
            return Err(EruditioError::Format("Not a valid RTF document".into()));
        }

        let tokens = tokenize(&data).map_err(|e| EruditioError::Parse(e.to_string()))?;
        let mut book = Book::new();

        // Parse tokens into book content.
        let state = parse_rtf_tokens(&tokens);

        book.metadata.title = state.title;
        if let Some(author) = state.author {
            book.metadata.authors.push(author);
        }
        book.metadata.description = state.subject;

        // Add extracted images as resources.
        for (id, data, media_type) in &state.images {
            let ext = if media_type == "image/jpeg" {
                "jpg"
            } else {
                "png"
            };
            let href = format!("images/{}.{}", id, ext);
            book.add_resource(id, &href, data.clone(), media_type);
        }

        // Split content into chapters at page breaks, or use single chapter.
        let chapters = split_rtf_content(&state.html_content);
        if chapters.is_empty() {
            book.add_chapter(&Chapter {
                title: book.metadata.title.clone(),
                content: state.html_content,
                id: Some("main".into()),
            });
        } else {
            for (i, (title, content)) in chapters.into_iter().enumerate() {
                book.add_chapter(&Chapter {
                    title,
                    content,
                    id: Some(format!("chapter_{}", i)),
                });
            }
        }

        if book.metadata.title.is_none() {
            book.metadata.title = Some("Unknown RTF Document".into());
        }

        Ok(book)
    }
}

/// RTF format writer.
#[derive(Default)]
pub struct RtfWriter;

impl RtfWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for RtfWriter {
    fn write_book(&self, book: &Book, output: &mut dyn Write) -> Result<()> {
        let rtf = writer::book_to_rtf(book);
        output.write_all(rtf.as_bytes())?;
        Ok(())
    }
}

/// Internal state accumulated while parsing RTF tokens.
struct RtfParseState {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    html_content: String,
    images: Vec<(String, Vec<u8>, String)>, // (id, data, media_type)
}

/// Parses RTF tokens into content and metadata.
fn parse_rtf_tokens(tokens: &[RtfToken]) -> RtfParseState {
    let mut state = RtfParseState {
        title: None,
        author: None,
        subject: None,
        html_content: String::new(),
        images: Vec::new(),
    };

    let mut group_depth = 0i32;
    let mut in_info = false;
    let mut info_depth = 0i32;
    let mut current_info_field: Option<String> = None;
    let mut info_text = String::new();

    // Formatting state.
    let mut bold = false;
    let mut italic = false;
    let mut in_paragraph = false;

    // Track ignorable destinations (groups starting with \*).
    let mut skip_depth: Option<i32> = None;
    let mut saw_star = false;

    // Image extraction state.
    let mut in_pict = false;
    let mut pict_depth: i32 = 0;
    let mut pict_format: &str = "png"; // default
    let mut pict_hex = String::new();
    let mut pict_count: usize = 0;

    // Known destination keywords to skip.
    let skip_destinations = [
        "fonttbl",
        "colortbl",
        "stylesheet",
        "header",
        "footer",
        "footnote",
        "field",
        "fldinst",
        "fldrslt",
        "datafield",
        "listtable",
        "listoverridetable",
        "revtbl",
        "rsidtbl",
    ];

    for token in tokens {
        // Handle skip mode — skip everything in ignorable destinations.
        if let Some(sd) = skip_depth {
            match token {
                RtfToken::GroupStart => {
                    skip_depth = Some(sd + 1);
                },
                RtfToken::GroupEnd => {
                    if sd <= 1 {
                        skip_depth = None;
                    } else {
                        skip_depth = Some(sd - 1);
                    }
                },
                _ => {},
            }
            continue;
        }

        match token {
            RtfToken::GroupStart => {
                group_depth += 1;
                if in_info {
                    info_depth += 1;
                }
                saw_star = false;
            },
            RtfToken::GroupEnd => {
                // Finalize image if closing a pict group.
                if in_pict && group_depth <= pict_depth {
                    in_pict = false;
                    if !pict_format.is_empty() && pict_hex.len() >= 2 {
                        // Decode hex pairs to bytes.
                        let bytes = crate::formats::common::text_utils::decode_hex_pairs(&pict_hex);
                        if !bytes.is_empty() {
                            let media_type = match pict_format {
                                "jpeg" => "image/jpeg",
                                _ => "image/png",
                            };
                            let ext = match pict_format {
                                "jpeg" => "jpg",
                                _ => "png",
                            };
                            let id = format!("rtf_img_{}", pict_count);
                            let href = format!("images/{}.{}", id, ext);
                            // Add image reference to HTML.
                            state
                                .html_content
                                .push_str(&format!("<img src=\"{}\" />", href));
                            state.images.push((id, bytes, media_type.to_string()));
                            pict_count += 1;
                        }
                    }
                    pict_hex.clear();
                }

                // Flush info field if we're closing one.
                if in_info && let Some(field) = current_info_field.take() {
                    let text = info_text.trim().to_string();
                    match field.as_str() {
                        "title" => state.title = Some(text),
                        "author" => state.author = Some(text),
                        "subject" => state.subject = Some(text),
                        _ => {},
                    }
                    info_text.clear();
                }

                if in_info {
                    info_depth -= 1;
                    if info_depth <= 0 {
                        in_info = false;
                    }
                }

                // Close formatting tags.
                if bold {
                    state.html_content.push_str("</b>");
                    bold = false;
                }
                if italic {
                    state.html_content.push_str("</i>");
                    italic = false;
                }

                group_depth -= 1;
                saw_star = false;
            },
            RtfToken::ControlSymbol('*') => {
                saw_star = true;
            },
            RtfToken::ControlWord { name, param } => {
                // Check if this starts a known skip destination.
                if (saw_star || skip_destinations.contains(&name.as_str()))
                    && group_depth > 0
                    && skip_destinations.contains(&name.as_str())
                {
                    skip_depth = Some(1);
                    saw_star = false;
                    continue;
                }
                saw_star = false;

                match name.as_str() {
                    "info" => {
                        in_info = true;
                        info_depth = 1;
                    },
                    "title" | "author" | "subject" | "keywords" if in_info => {
                        current_info_field = Some(name.clone());
                        info_text.clear();
                    },
                    "par" | "pard" if !in_info => {
                        if in_paragraph {
                            state.html_content.push_str("</p>\n");
                        }
                        state.html_content.push_str("<p>");
                        in_paragraph = true;
                    },
                    "line" if !in_info => {
                        state.html_content.push_str("<br />");
                    },
                    "page" if !in_info => {
                        if in_paragraph {
                            state.html_content.push_str("</p>\n");
                            in_paragraph = false;
                        }
                        state.html_content.push_str("<!-- pagebreak -->\n");
                    },
                    "b" if !in_info => {
                        let on = param.unwrap_or(1) != 0;
                        if on && !bold {
                            state.html_content.push_str("<b>");
                            bold = true;
                        } else if !on && bold {
                            state.html_content.push_str("</b>");
                            bold = false;
                        }
                    },
                    "i" if !in_info => {
                        let on = param.unwrap_or(1) != 0;
                        if on && !italic {
                            state.html_content.push_str("<i>");
                            italic = true;
                        } else if !on && italic {
                            state.html_content.push_str("</i>");
                            italic = false;
                        }
                    },
                    "tab" if !in_info => {
                        state.html_content.push('\t');
                    },
                    "pict" if !in_info => {
                        in_pict = true;
                        pict_depth = group_depth;
                        pict_format = "png";
                        pict_hex.clear();
                    },
                    "pngblip" if in_pict => {
                        pict_format = "png";
                    },
                    "jpegblip" if in_pict => {
                        pict_format = "jpeg";
                    },
                    "emfblip" | "wmetafile" if in_pict => {
                        pict_format = "";
                    }, // unsupported
                    _ => {
                        // Ignore other control words.
                    },
                }
            },
            RtfToken::Text(text) => {
                if in_pict {
                    // Collect hex digits, skipping whitespace
                    for &b in text.as_bytes() {
                        if b.is_ascii_hexdigit() {
                            pict_hex.push(b as char);
                        }
                    }
                    continue; // don't add to HTML
                }
                if in_info && current_info_field.is_some() {
                    info_text.push_str(text);
                } else if !in_info {
                    // Ensure we're in a paragraph.
                    if !in_paragraph {
                        state.html_content.push_str("<p>");
                        in_paragraph = true;
                    }
                    // Escape for HTML.
                    let escaped = crate::formats::common::text_utils::escape_html(text);
                    state.html_content.push_str(&escaped);
                }
            },
            RtfToken::Unicode(code) => {
                if in_info && current_info_field.is_some() {
                    if let Some(ch) = i32_to_char(*code) {
                        info_text.push(ch);
                    }
                } else if !in_info {
                    if !in_paragraph {
                        state.html_content.push_str("<p>");
                        in_paragraph = true;
                    }
                    if let Some(ch) = i32_to_char(*code) {
                        state.html_content.push(ch);
                    }
                }
            },
            RtfToken::HexByte(byte) => {
                if in_pict {
                    pict_hex.push_str(&format!("{:02x}", byte));
                    continue;
                }
                // Treat as Windows-1252 (the default RTF encoding).
                let ch = cp1252_to_char(*byte);
                if in_info && current_info_field.is_some() {
                    info_text.push(ch);
                } else if !in_info {
                    if !in_paragraph {
                        state.html_content.push_str("<p>");
                        in_paragraph = true;
                    }
                    state.html_content.push(ch);
                }
            },
            RtfToken::ControlSymbol(sym) => {
                let ch = match sym {
                    '~' => '\u{00A0}', // non-breaking space
                    '-' => '\u{00AD}', // soft hyphen
                    '_' => '\u{2011}', // non-breaking hyphen
                    c => *c,
                };
                if in_info && current_info_field.is_some() {
                    info_text.push(ch);
                } else if !in_info {
                    if !in_paragraph {
                        state.html_content.push_str("<p>");
                        in_paragraph = true;
                    }
                    state.html_content.push(ch);
                }
            },
        }
    }

    // Close trailing paragraph.
    if in_paragraph {
        state.html_content.push_str("</p>\n");
    }

    state
}

/// Splits HTML content at pagebreak markers into separate chapters.
fn split_rtf_content(html: &str) -> Vec<(Option<String>, String)> {
    let parts: Vec<&str> = html.split("<!-- pagebreak -->").collect();

    if parts.len() <= 1 {
        return Vec::new();
    }

    parts
        .into_iter()
        .enumerate()
        .filter(|(_, part)| !part.trim().is_empty())
        .map(|(i, part)| {
            let title = if i == 0 {
                None
            } else {
                Some(format!("Section {}", i))
            };
            (title, part.trim().to_string())
        })
        .collect()
}

/// Converts a signed RTF Unicode value to a char.
fn i32_to_char(code: i32) -> Option<char> {
    let unsigned = if code < 0 {
        (code + 65536) as u32
    } else {
        code as u32
    };
    char::from_u32(unsigned)
}

/// Converts a CP-1252 byte to a Unicode character.
fn cp1252_to_char(byte: u8) -> char {
    crate::formats::common::text_utils::cp1252_byte_to_char(byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rtf_reader_extracts_text() {
        let rtf =
            b"{\\rtf1\\ansi\\deff0 {\\fonttbl{\\f0 Times;}}\\f0\\fs24 Hello World\\par Goodbye}";
        let mut cursor = Cursor::new(rtf.as_slice());
        let book = RtfReader::new().read_book(&mut cursor).unwrap();

        let chapters = book.chapters();
        assert!(!chapters.is_empty());
        let all_content: String = chapters.iter().map(|c| c.content.clone()).collect();
        assert!(all_content.contains("Hello World"));
        assert!(all_content.contains("Goodbye"));
    }

    #[test]
    fn rtf_reader_extracts_metadata() {
        let rtf = b"{\\rtf1\\ansi {\\info{\\title My Book}{\\author Jane Doe}} Hello}";
        let mut cursor = Cursor::new(rtf.as_slice());
        let book = RtfReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("My Book"));
        assert_eq!(book.metadata.authors, vec!["Jane Doe"]);
    }

    #[test]
    fn rtf_reader_handles_unicode() {
        let rtf = b"{\\rtf1\\ansi em dash: \\u8212?done}";
        let mut cursor = Cursor::new(rtf.as_slice());
        let book = RtfReader::new().read_book(&mut cursor).unwrap();

        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains('\u{2014}'));
    }

    #[test]
    fn rtf_reader_handles_hex_escapes() {
        let rtf = b"{\\rtf1\\ansi e-acute: \\'e9}";
        let mut cursor = Cursor::new(rtf.as_slice());
        let book = RtfReader::new().read_book(&mut cursor).unwrap();

        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains('\u{00E9}')); // é
    }

    #[test]
    fn rtf_reader_rejects_non_rtf() {
        let mut cursor = Cursor::new(b"Not an RTF file".as_slice());
        let result = RtfReader::new().read_book(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn rtf_round_trip() {
        let mut book = Book::new();
        book.metadata.title = Some("RTF Test".into());
        book.metadata.authors.push("Author".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        // Write RTF.
        let mut output = Vec::new();
        RtfWriter::new().write_book(&book, &mut output).unwrap();

        // Read back.
        let mut cursor = Cursor::new(output);
        let decoded = RtfReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("RTF Test"));
        assert_eq!(decoded.metadata.authors, vec!["Author"]);
        let content: String = decoded
            .chapters()
            .iter()
            .map(|c| c.content.clone())
            .collect();
        assert!(content.contains("Hello world"));
    }

    #[test]
    fn cp1252_special_chars() {
        assert_eq!(cp1252_to_char(0x80), '\u{20AC}'); // Euro
        assert_eq!(cp1252_to_char(0x93), '\u{201C}'); // Left double quote
        assert_eq!(cp1252_to_char(0x94), '\u{201D}'); // Right double quote
        assert_eq!(cp1252_to_char(0x97), '\u{2014}'); // Em dash
        assert_eq!(cp1252_to_char(0x41), 'A'); // Regular ASCII
    }

    #[test]
    fn i32_to_char_negative_unicode() {
        // RTF uses signed 16-bit for Unicode, so values > 32767 are negative.
        assert_eq!(i32_to_char(-4), Some('\u{FFFC}')); // Object replacement char
        assert_eq!(i32_to_char(8212), Some('\u{2014}')); // Em dash
    }

    #[test]
    fn rtf_reader_extracts_images() {
        // Build RTF with a small PNG image (1x1 pixel) encoded as hex
        let png_hex = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489\
                       0000000a49444154789c626000000002000198e195280000000049454e44ae426082";
        let rtf = format!(
            "{{\\rtf1\\ansi Hello {{\\pict\\pngblip {}}}World}}",
            png_hex
        );
        let mut cursor = std::io::Cursor::new(rtf.as_bytes());
        let book = RtfReader::new().read_book(&mut cursor).unwrap();

        // Should have extracted one image.
        let resources = book.resources();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].media_type, "image/png");

        // HTML should contain img tag.
        let content: String = book.chapters().iter().map(|c| c.content.clone()).collect();
        assert!(content.contains("<img src="));
        assert!(content.contains("Hello"));
        assert!(content.contains("World"));
    }
}
