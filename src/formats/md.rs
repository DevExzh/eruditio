//! Markdown ebook reader.
//!
//! Converts Markdown (`.md`, `.markdown`) to HTML using pulldown-cmark,
//! then wraps the result as a single-chapter Book.

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::Result;
use std::io::{Read, Write};

/// Markdown format reader.
#[derive(Default)]
pub struct MdReader;

impl MdReader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for MdReader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut input = String::new();
        reader.read_to_string(&mut input)?;

        let opts = pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_FOOTNOTES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS
            | pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES;

        let parser = pulldown_cmark::Parser::new_ext(&input, opts);
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, parser);

        let mut book = Book::new();

        // Try to extract a title from the first H1 heading.
        if let Some(start) = html.find("<h1")
            && let Some(gt) = html[start..].find('>')
        {
            let after = start + gt + 1;
            if let Some(end) = html[after..].find("</h1>") {
                let title = &html[after..after + end];
                book.metadata.title = Some(title.to_string());
            }
        }

        book.add_chapter(Chapter {
            title: book.metadata.title.clone(),
            content: html,
            id: Some("md_content".into()),
        });

        Ok(book)
    }
}

/// Markdown format writer.
///
/// Converts a Book's HTML chapters into Markdown text.
#[derive(Default)]
pub struct MdWriter;

impl MdWriter {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for MdWriter {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let mut md = String::new();
        for (i, chapter) in book.chapter_views().iter().enumerate() {
            if i > 0 {
                md.push_str("\n---\n\n");
            }
            html_to_markdown(chapter.content, &mut md);
        }
        writer.write_all(md.as_bytes())?;
        Ok(())
    }
}

/// Converts HTML content to Markdown markup.
fn html_to_markdown(html: &str, md: &mut String) {
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut link_href = String::new();
    let mut list_stack: Vec<bool> = Vec::new(); // true = ordered
    let mut ol_counter: Vec<u32> = Vec::new();
    let mut in_pre = false;
    let mut at_line_start = true;
    let mut in_blockquote = false;

    while pos < len {
        if bytes[pos] == b'<' {
            let tag_end = match html[pos..].find('>') {
                Some(e) => pos + e + 1,
                None => break,
            };
            let raw_tag = &html[pos..tag_end];
            // Stack-based lowercasing avoids heap allocation per tag.
            // Tags longer than the buffer are truncated—this is safe because
            // all equality/prefix comparisons are against short patterns.
            let tag_bytes = raw_tag.as_bytes();
            let mut lower_buf = [0u8; 128];
            let lower_len = tag_bytes.len().min(128);
            for i in 0..lower_len {
                lower_buf[i] = tag_bytes[i].to_ascii_lowercase();
            }
            // SAFETY: ASCII lowercasing preserves UTF-8 validity.
            let tag = unsafe { std::str::from_utf8_unchecked(&lower_buf[..lower_len]) };

            if tag == "<p>" || tag.starts_with("<p ") {
                ensure_blank_line(md);
                at_line_start = true;
            } else if tag == "</p>" {
                md.push_str("\n\n");
                at_line_start = true;
            } else if tag.starts_with("<h") && tag.len() >= 4 {
                let level_byte = tag.as_bytes()[2];
                if (b'1'..=b'6').contains(&level_byte) {
                    let level = (level_byte - b'0') as usize;
                    ensure_blank_line(md);
                    for _ in 0..level {
                        md.push('#');
                    }
                    md.push(' ');
                    at_line_start = false;
                }
            } else if tag.starts_with("</h") && tag.len() >= 5 {
                md.push_str("\n\n");
                at_line_start = true;
            } else if tag == "<strong>"
                || tag == "<b>"
                || tag.starts_with("<strong ")
                || tag.starts_with("<b ")
                || tag == "</strong>"
                || tag == "</b>"
            {
                md.push_str("**");
            } else if tag == "<em>"
                || tag == "<i>"
                || tag.starts_with("<em ")
                || tag.starts_with("<i ")
                || tag == "</em>"
                || tag == "</i>"
            {
                md.push('*');
            } else if tag == "<s>"
                || tag == "<del>"
                || tag == "<strike>"
                || tag == "</s>"
                || tag == "</del>"
                || tag == "</strike>"
            {
                md.push_str("~~");
            } else if tag == "<code>" || tag == "</code>" {
                if !in_pre {
                    md.push('`');
                }
            } else if tag == "<pre>" || tag.starts_with("<pre ") {
                ensure_newline(md);
                md.push_str("```\n");
                in_pre = true;
                at_line_start = true;
            } else if tag == "</pre>" {
                ensure_newline(md);
                md.push_str("```\n\n");
                in_pre = false;
                at_line_start = true;
            } else if tag == "<blockquote>" || tag.starts_with("<blockquote ") {
                in_blockquote = true;
                ensure_newline(md);
                md.push_str("> ");
                at_line_start = false;
            } else if tag == "</blockquote>" {
                in_blockquote = false;
                md.push_str("\n\n");
                at_line_start = true;
            } else if tag == "<br>" || tag == "<br/>" || tag == "<br />" {
                if in_pre {
                    md.push('\n');
                } else {
                    md.push_str("  \n");
                    if in_blockquote {
                        md.push_str("> ");
                    }
                }
                at_line_start = true;
            } else if tag == "<hr>" || tag == "<hr/>" || tag == "<hr />" || tag.starts_with("<hr ")
            {
                ensure_newline(md);
                md.push_str("\n---\n\n");
                at_line_start = true;
            } else if tag == "<ul>" || tag.starts_with("<ul ") {
                list_stack.push(false);
                ol_counter.push(0);
                ensure_newline(md);
            } else if tag == "</ul>" {
                list_stack.pop();
                ol_counter.pop();
                ensure_newline(md);
                at_line_start = true;
            } else if tag == "<ol>" || tag.starts_with("<ol ") {
                list_stack.push(true);
                ol_counter.push(0);
                ensure_newline(md);
            } else if tag == "</ol>" {
                list_stack.pop();
                ol_counter.pop();
                ensure_newline(md);
                at_line_start = true;
            } else if tag == "<li>" || tag.starts_with("<li ") {
                let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                ensure_newline(md);
                md.push_str(&indent);
                if let Some(&is_ordered) = list_stack.last() {
                    if is_ordered {
                        if let Some(counter) = ol_counter.last_mut() {
                            *counter += 1;
                            md.push_str(&format!("{}. ", counter));
                        }
                    } else {
                        md.push_str("- ");
                    }
                } else {
                    md.push_str("- ");
                }
                at_line_start = false;
            } else if tag == "</li>" {
                ensure_newline(md);
                at_line_start = true;
            } else if tag.starts_with("<a ") {
                link_href.clear();
                if let Some(href) = extract_html_attr(raw_tag, "href") {
                    link_href = href;
                }
                md.push('[');
            } else if tag == "</a>" {
                md.push_str(&format!("]({})", link_href));
                link_href.clear();
            } else if tag.starts_with("<img ") {
                let src = extract_html_attr(raw_tag, "src").unwrap_or_default();
                let alt = extract_html_attr(raw_tag, "alt").unwrap_or_default();
                md.push_str(&format!("![{}]({})", alt, src));
            }
            // Other tags: skip silently.

            pos = tag_end;
        } else if bytes[pos] == b'&' {
            let (ch, consumed) = decode_md_entity(html, pos);
            md.push(ch);
            pos += consumed;
            at_line_start = false;
        } else {
            let Some(ch) = html[pos..].chars().next() else {
                break;
            };
            if in_pre {
                md.push(ch);
            } else if ch.is_whitespace() {
                if !at_line_start && !md.ends_with(' ') && !md.ends_with('\n') {
                    md.push(' ');
                }
            } else {
                md.push(ch);
                at_line_start = false;
            }
            pos += ch.len_utf8();
        }
    }

    // Trim trailing blank lines, ensure single trailing newline.
    let trimmed = md.trim_end();
    let keep = trimmed.len();
    md.truncate(keep);
    md.push('\n');
}

/// Ensures the output ends with at least one newline.
fn ensure_newline(md: &mut String) {
    if !md.is_empty() && !md.ends_with('\n') {
        md.push('\n');
    }
}

/// Ensures the output ends with a blank line (double newline).
fn ensure_blank_line(md: &mut String) {
    if md.is_empty() {
        return;
    }
    ensure_newline(md);
    if !md.ends_with("\n\n") {
        md.push('\n');
    }
}

/// Extracts an HTML attribute value from a tag string (case-insensitive name).
fn extract_html_attr(tag: &str, attr_name: &str) -> Option<String> {
    use crate::formats::common::text_utils;
    for quote in [b'"', b'\''] {
        let attr_bytes = attr_name.as_bytes();
        let mut pattern = [0u8; 64];
        let pat_len = attr_bytes.len() + 2;
        pattern[..attr_bytes.len()].copy_from_slice(attr_bytes);
        pattern[attr_bytes.len()] = b'=';
        pattern[attr_bytes.len() + 1] = quote;

        if let Some(start) = text_utils::find_case_insensitive(tag.as_bytes(), &pattern[..pat_len])
        {
            let val_start = start + pat_len;
            if let Some(end) = tag[val_start..].find(quote as char) {
                return Some(tag[val_start..val_start + end].to_string());
            }
        }
    }
    None
}

/// Decodes an HTML entity at position `pos`. Returns (char, bytes_consumed).
fn decode_md_entity(html: &str, pos: usize) -> (char, usize) {
    let rest = &html[pos..];
    let entities = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&nbsp;", ' '),
    ];
    for (entity, ch) in &entities {
        if rest.starts_with(entity) {
            return (*ch, entity.len());
        }
    }
    if rest.starts_with("&#x") || rest.starts_with("&#X") {
        if let Some(semi) = rest[..rest.len().min(12)].find(';')
            && let Ok(code) = u32::from_str_radix(&rest[3..semi], 16)
            && let Some(ch) = char::from_u32(code)
        {
            return (ch, semi + 1);
        }
    } else if rest.starts_with("&#")
        && let Some(semi) = rest[..rest.len().min(12)].find(';')
        && let Ok(code) = rest[2..semi].parse::<u32>()
        && let Some(ch) = char::from_u32(code)
    {
        return (ch, semi + 1);
    }
    ('&', 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn basic_markdown_to_html() {
        let md = "# Hello\n\nThis is **bold** text.\n";
        let mut cursor = Cursor::new(md.as_bytes());
        let book = MdReader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Hello"));
        let chapters = book.chapters();
        assert_eq!(chapters.len(), 1);
        assert!(chapters[0].content.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_with_table() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let mut cursor = Cursor::new(md.as_bytes());
        let book = MdReader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();
        assert!(chapters[0].content.contains("<table>"));
    }

    #[test]
    fn md_writer_basic() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Test".into()),
            content: "<h1>Title</h1><p>Hello <strong>bold</strong> and <em>italic</em>.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        MdWriter::new().write_book(&book, &mut output).unwrap();
        let md = String::from_utf8(output).unwrap();

        assert!(md.contains("# Title"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
    }

    #[test]
    fn md_writer_lists() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<ul><li>One</li><li>Two</li></ul>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        MdWriter::new().write_book(&book, &mut output).unwrap();
        let md = String::from_utf8(output).unwrap();

        assert!(md.contains("- One"));
        assert!(md.contains("- Two"));
    }

    #[test]
    fn md_writer_links_and_images() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: None,
            content: "<p><a href=\"https://example.com\">link</a> and <img src=\"img.png\" alt=\"photo\" /></p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        MdWriter::new().write_book(&book, &mut output).unwrap();
        let md = String::from_utf8(output).unwrap();

        assert!(md.contains("[link](https://example.com)"));
        assert!(md.contains("![photo](img.png)"));
    }

    #[test]
    fn html_to_markdown_entities() {
        let mut md = String::new();
        html_to_markdown("<p>A &amp; B &lt; C</p>", &mut md);
        assert!(md.contains("A & B < C"));
    }

    #[test]
    fn html_to_markdown_code_block() {
        let mut md = String::new();
        html_to_markdown("<pre><code>fn main() {}</code></pre>", &mut md);
        assert!(md.contains("```\nfn main() {}\n```"));
    }
}
