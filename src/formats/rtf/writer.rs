//! RTF writer — generates RTF documents from `Book`.

use crate::domain::Book;
use crate::formats::common::html_utils::strip_tags;

/// Converts a `Book` to an RTF document string.
pub fn book_to_rtf(book: &Book) -> String {
    let mut rtf = String::with_capacity(8192);

    // RTF header.
    rtf.push_str("{\\rtf1\\ansi\\deff0\n");

    // Font table.
    rtf.push_str("{\\fonttbl{\\f0\\froman Times New Roman;}}\n");

    // Color table.
    rtf.push_str("{\\colortbl;\\red0\\green0\\blue0;}\n");

    // Info group (metadata).
    write_info_group(book, &mut rtf);

    // Default font and size.
    rtf.push_str("\\f0\\fs24\n");

    // Content.
    let chapters = book.chapters();
    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            // Page break between chapters.
            rtf.push_str("\\page\n");
        }

        // Chapter title as bold heading.
        if let Some(ref title) = chapter.title {
            rtf.push_str("{\\b\\fs32 ");
            write_rtf_text(&mut rtf, title);
            rtf.push_str("}\\par\\par\n");
        }

        // Convert HTML content to RTF.
        html_to_rtf(&chapter.content, &mut rtf);
    }

    rtf.push_str("}\n");
    rtf
}

/// Writes the `\info` group containing document metadata.
fn write_info_group(book: &Book, rtf: &mut String) {
    let m = &book.metadata;
    let has_info = m.title.is_some()
        || !m.authors.is_empty()
        || m.description.is_some()
        || !m.subjects.is_empty();

    if !has_info {
        return;
    }

    rtf.push_str("{\\info\n");

    if let Some(ref title) = m.title {
        rtf.push_str("{\\title ");
        write_rtf_text(rtf, title);
        rtf.push_str("}\n");
    }

    if !m.authors.is_empty() {
        rtf.push_str("{\\author ");
        write_rtf_text(rtf, &m.authors.join(" & "));
        rtf.push_str("}\n");
    }

    if let Some(ref desc) = m.description {
        rtf.push_str("{\\subject ");
        write_rtf_text(rtf, desc);
        rtf.push_str("}\n");
    }

    if !m.subjects.is_empty() {
        rtf.push_str("{\\keywords ");
        write_rtf_text(rtf, &m.subjects.join(", "));
        rtf.push_str("}\n");
    }

    rtf.push_str("}\n");
}

/// Converts simple HTML content to RTF control words.
///
/// Handles: `<p>`, `<br>`, `<b>`, `<i>`, `<em>`, `<strong>`, `<h1>`-`<h6>`.
/// Strips all other tags and converts entities.
fn html_to_rtf(html: &str, rtf: &mut String) {
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    while pos < len {
        if bytes[pos] == b'<' {
            // Parse the tag.
            let tag_end = match html[pos..].find('>') {
                Some(e) => pos + e + 1,
                None => break,
            };

            let tag_bytes = &html.as_bytes()[pos..tag_end];

            // Handle known tags using case-insensitive byte comparison
            // to avoid per-tag to_lowercase() allocation.
            if tag_bytes.len() >= 2
                && tag_bytes[1].eq_ignore_ascii_case(&b'p')
                && (tag_bytes.len() == 3 || tag_bytes[2] == b' ' || tag_bytes[2] == b'>')
            {
                // Start of paragraph -- already in paragraph mode.
            } else if tag_bytes.eq_ignore_ascii_case(b"</p>") {
                rtf.push_str("\\par\n");
            } else if tag_bytes.eq_ignore_ascii_case(b"<br>")
                || tag_bytes.eq_ignore_ascii_case(b"<br/>")
                || tag_bytes.eq_ignore_ascii_case(b"<br />")
            {
                rtf.push_str("\\line\n");
            } else if tag_bytes.eq_ignore_ascii_case(b"<b>")
                || tag_bytes.eq_ignore_ascii_case(b"<strong>")
            {
                rtf.push_str("{\\b ");
            } else if tag_bytes.eq_ignore_ascii_case(b"</b>")
                || tag_bytes.eq_ignore_ascii_case(b"</strong>")
            {
                rtf.push('}');
            } else if tag_bytes.eq_ignore_ascii_case(b"<i>")
                || tag_bytes.eq_ignore_ascii_case(b"<em>")
            {
                rtf.push_str("{\\i ");
            } else if tag_bytes.eq_ignore_ascii_case(b"</i>")
                || tag_bytes.eq_ignore_ascii_case(b"</em>")
            {
                rtf.push('}');
            } else if tag_bytes.len() >= 4
                && tag_bytes[0] == b'<'
                && tag_bytes[1].eq_ignore_ascii_case(&b'h')
                && tag_bytes[2].is_ascii_digit()
            {
                // Heading -- bold, larger font.
                rtf.push_str("{\\b\\fs32 ");
            } else if tag_bytes.len() >= 5
                && tag_bytes[0] == b'<'
                && tag_bytes[1] == b'/'
                && tag_bytes[2].eq_ignore_ascii_case(&b'h')
            {
                rtf.push_str("}\\par\\par\n");
            }
            // Other tags are silently skipped.

            pos = tag_end;
        } else if bytes[pos] == b'&' {
            // HTML entity.
            let (ch, consumed) = decode_html_entity(html, pos);
            write_rtf_char(rtf, ch);
            pos += consumed;
        } else {
            // Regular character — decode full UTF-8 codepoint.
            let Some(ch) = html[pos..].chars().next() else {
                break;
            };
            write_rtf_char(rtf, ch);
            pos += ch.len_utf8();
        }
    }
}

/// Decodes an HTML entity starting at `pos`. Returns (decoded_char, bytes_consumed).
fn decode_html_entity(html: &str, pos: usize) -> (char, usize) {
    let rest = &html[pos..];

    // Named entities.
    let entities = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&nbsp;", '\u{00A0}'),
        ("&mdash;", '\u{2014}'),
        ("&ndash;", '\u{2013}'),
        ("&lsquo;", '\u{2018}'),
        ("&rsquo;", '\u{2019}'),
        ("&ldquo;", '\u{201C}'),
        ("&rdquo;", '\u{201D}'),
        ("&hellip;", '\u{2026}'),
    ];

    for (entity, ch) in &entities {
        if rest.starts_with(entity) {
            return (*ch, entity.len());
        }
    }

    // Numeric entity: &#NNN; or &#xHHH;
    if rest.starts_with("&#")
        && let Some(semi) = rest.find(';')
    {
        let num_str = &rest[2..semi];
        let value = if let Some(hex) = num_str.strip_prefix('x') {
            u32::from_str_radix(hex, 16).ok()
        } else {
            num_str.parse::<u32>().ok()
        };
        if let Some(v) = value
            && let Some(ch) = char::from_u32(v)
        {
            return (ch, semi + 1);
        }
    }

    // Unknown entity — pass through the ampersand.
    ('&', 1)
}

/// Writes a single character to RTF, escaping as needed.
fn write_rtf_char(rtf: &mut String, ch: char) {
    match ch {
        '\\' => rtf.push_str("\\\\"),
        '{' => rtf.push_str("\\{"),
        '}' => rtf.push_str("\\}"),
        '\n' => rtf.push_str("\\par\n"),
        c if c as u32 > 127 => {
            // Unicode character: \uN followed by ? as replacement.
            // Write directly to avoid format!() allocation per character.
            use std::fmt::Write;
            let _ = write!(rtf, "\\u{}?", c as i32);
        },
        c => rtf.push(c),
    }
}

/// Writes a string to RTF, escaping special characters.
fn write_rtf_text(rtf: &mut String, text: &str) {
    for ch in text.chars() {
        write_rtf_char(rtf, ch);
    }
}

/// Extracts plain text from RTF for simple preview purposes.
/// This is the inverse of book_to_rtf but simplified.
pub fn rtf_to_plain_text(rtf: &str) -> String {
    // Simple approach: strip RTF control words and extract text.
    strip_tags(rtf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Chapter;

    #[test]
    fn basic_rtf_output() {
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.add_chapter(&Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(rtf.starts_with("{\\rtf1"));
        assert!(rtf.contains("\\title Test"));
        assert!(rtf.contains("Hello world"));
        assert!(rtf.ends_with("}\n"));
    }

    #[test]
    fn rtf_escapes_special_chars() {
        let mut rtf = String::new();
        write_rtf_text(&mut rtf, "a\\b{c}d");
        assert_eq!(rtf, "a\\\\b\\{c\\}d");
    }

    #[test]
    fn rtf_encodes_unicode() {
        let mut rtf = String::new();
        write_rtf_char(&mut rtf, '\u{2014}'); // em dash
        assert!(rtf.contains("\\u8212?"));
    }

    #[test]
    fn html_to_rtf_handles_paragraphs() {
        let mut rtf = String::new();
        html_to_rtf("<p>Hello</p><p>World</p>", &mut rtf);
        assert!(rtf.contains("Hello"));
        assert!(rtf.contains("\\par"));
        assert!(rtf.contains("World"));
    }

    #[test]
    fn html_to_rtf_handles_bold_italic() {
        let mut rtf = String::new();
        html_to_rtf("<b>Bold</b> and <i>Italic</i>", &mut rtf);
        assert!(rtf.contains("{\\b Bold}"));
        assert!(rtf.contains("{\\i Italic}"));
    }

    #[test]
    fn html_entity_decoding() {
        let (ch, consumed) = decode_html_entity("&amp;rest", 0);
        assert_eq!(ch, '&');
        assert_eq!(consumed, 5);

        let (ch, consumed) = decode_html_entity("&#8212;rest", 0);
        assert_eq!(ch, '\u{2014}');
        assert_eq!(consumed, 7);

        let (ch, consumed) = decode_html_entity("&#x2014;rest", 0);
        assert_eq!(ch, '\u{2014}');
        assert_eq!(consumed, 8);
    }

    #[test]
    fn info_group_includes_metadata() {
        let mut book = Book::new();
        book.metadata.title = Some("My Title".into());
        book.metadata.authors.push("Alice".into());

        let rtf = book_to_rtf(&book);
        assert!(rtf.contains("{\\title My Title}"));
        assert!(rtf.contains("{\\author Alice}"));
    }

    #[test]
    fn multiple_authors_joined_with_ampersand() {
        let mut book = Book::new();
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.authors.push("John Smith".into());

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("{\\author Jane Doe & John Smith}"),
            "Expected authors joined with ' & ', got: {rtf}"
        );
    }
}
