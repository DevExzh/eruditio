use std::fmt::Write as FmtWrite;

use crate::domain::{Book, Chapter, FormatReader, FormatWriter};
use crate::error::{EruditioError, Result};
use crate::formats::common::MAX_INPUT_SIZE;
use crate::formats::common::text_utils::{contains_ascii_ci, find_case_insensitive, push_escape_html};
use base64::Engine;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{Read, Write};

/// FB2 format reader.
#[derive(Default)]
pub struct Fb2Reader;

impl Fb2Reader {
    pub fn new() -> Self {
        Self
    }
}

impl FormatReader for Fb2Reader {
    fn read_book(&self, reader: &mut dyn Read) -> Result<Book> {
        let mut contents = String::new();
        (&mut *reader)
            .take(MAX_INPUT_SIZE)
            .read_to_string(&mut contents)?;

        if contents.trim().is_empty() {
            return Err(EruditioError::Format("Empty FB2 input".into()));
        }

        let mut xml_reader = XmlReader::from_str(&contents);
        xml_reader.config_mut().trim_text(true);

        let mut book = Book::new();
        let mut buf = Vec::with_capacity(256);

        // State tracking -- incremental path buffer avoids join("/") allocation per element.
        let mut path_buf = String::with_capacity(128);
        let mut current_text = String::new();
        let mut in_body = false;
        let mut current_section_title = None;
        let mut current_section_content = String::new();
        let mut section_counter: u32 = 0;
        // Reusable buffer for small format strings (section IDs, image hrefs).
        let mut fmt_buf = String::with_capacity(32);
        // Track nested section depth within <body> so that content inside
        // `<section>` elements at any depth is captured, not just the first level.
        let mut section_depth: u32 = 0;
        let mut in_section_title = false;

        let mut current_binary_id = None;
        let mut current_binary_ctype = None;

        loop {
            match xml_reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name_raw = e.name();
                    let tag = std::str::from_utf8(name_raw.as_ref()).unwrap_or("");
                    if tag == "body" {
                        in_body = true;
                    } else if tag == "binary" {
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"id" => {
                                    current_binary_id =
                                        Some(crate::formats::common::text_utils::bytes_to_string(
                                            &attr.value,
                                        ));
                                },
                                b"content-type" => {
                                    current_binary_ctype =
                                        Some(crate::formats::common::text_utils::bytes_to_string(
                                            &attr.value,
                                        ));
                                },
                                _ => {},
                            }
                        }
                    } else if tag == "section" && in_body {
                        // Entering a (possibly nested) section. If there is
                        // already accumulated content from the parent section,
                        // flush it as its own chapter before starting the child.
                        if section_depth > 0
                            && (current_section_title.is_some()
                                || !current_section_content.is_empty())
                        {
                            section_counter += 1;
                            fmt_buf.clear();
                            let _ = write!(fmt_buf, "section_{}", section_counter);
                            book.add_chapter(Chapter {
                                title: current_section_title.take(),
                                content: std::mem::take(&mut current_section_content),
                                id: Some(fmt_buf.clone()),
                            });
                        }
                        section_depth += 1;
                    } else if tag == "title" && section_depth > 0 {
                        in_section_title = true;
                    }
                    if !path_buf.is_empty() {
                        path_buf.push('/');
                    }
                    path_buf.push_str(tag);
                    current_text.clear();
                },
                Ok(Event::Text(ref e)) => {
                    // Reuse the buffer instead of allocating a new String per event.
                    current_text.clear();
                    match std::str::from_utf8(e.as_ref()) {
                        Ok(s) => current_text.push_str(s),
                        Err(_) => current_text.push_str(&String::from_utf8_lossy(e.as_ref())),
                    }
                },
                Ok(Event::End(ref e)) => {
                    let name_raw = e.name();
                    let tag = std::str::from_utf8(name_raw.as_ref()).unwrap_or("");

                    if tag == "binary" {
                        if let Some(id) = current_binary_id.take() {
                            // Strip newlines in-place to avoid allocating a copy
                            // of potentially large (100KB+) base64 blocks.
                            current_text.retain(|c| c != '\n' && c != '\r');
                            let decoded = base64::engine::general_purpose::STANDARD
                                .decode(current_text.trim())
                                .unwrap_or_default();

                            let media_type = current_binary_ctype
                                .take()
                                .unwrap_or_else(|| "application/octet-stream".into());

                            fmt_buf.clear();
                            let _ = write!(fmt_buf, "images/{}", &id);
                            book.add_resource(&id, &fmt_buf, decoded, media_type);
                        }
                    } else if !in_body {
                        // Parse metadata
                        if path_buf == "FictionBook/description/title-info/book-title" {
                            book.metadata.title = Some(std::mem::take(&mut current_text));
                        } else if path_buf == "FictionBook/description/title-info/author/first-name"
                            || path_buf == "FictionBook/description/title-info/author/last-name"
                            || path_buf == "FictionBook/description/title-info/author/middle-name"
                        {
                            if tag == "first-name" {
                                book.metadata.authors.push(std::mem::take(&mut current_text));
                            } else if tag == "last-name" || tag == "middle-name" {
                                if let Some(last) = book.metadata.authors.last_mut() {
                                    last.push(' ');
                                    last.push_str(&current_text);
                                } else {
                                    book.metadata.authors.push(std::mem::take(&mut current_text));
                                }
                            }
                        } else if path_buf == "FictionBook/description/title-info/lang" {
                            book.metadata.language = Some(std::mem::take(&mut current_text));
                        } else if path_buf == "FictionBook/description/title-info/annotation/p" {
                            let desc = book.metadata.description.get_or_insert_with(String::new);
                            if !desc.is_empty() {
                                desc.push('\n');
                            }
                            desc.push_str(&current_text);
                        } else if path_buf == "FictionBook/description/publish-info/publisher" {
                            book.metadata.publisher = Some(std::mem::take(&mut current_text));
                        } else if path_buf == "FictionBook/description/publish-info/isbn" {
                            book.metadata.isbn = Some(std::mem::take(&mut current_text));
                        } else if path_buf == "FictionBook/description/publish-info/year"
                            && let Ok(year) = current_text.trim().parse::<i32>()
                        {
                            use chrono::NaiveDate;
                            if let Some(date) = NaiveDate::from_ymd_opt(year, 1, 1) {
                                book.metadata.publication_date =
                                    Some(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
                            }
                        }
                    } else {
                        // Parse content
                        if tag == "p" && in_section_title {
                            current_section_title = Some(std::mem::take(&mut current_text));
                        } else if tag == "title" && section_depth > 0 {
                            in_section_title = false;
                        } else if section_depth > 0 && tag == "p" {
                            current_section_content.push_str("<p>");
                            current_section_content.push_str(&current_text);
                            current_section_content.push_str("</p>\n");
                        } else if tag == "section" && section_depth > 0 {
                            section_depth -= 1;
                            // Only emit a chapter when there is a title or
                            // content. This avoids empty chapters for wrapper
                            // sections whose content was already flushed when
                            // their child sections started.
                            if current_section_title.is_some()
                                || !current_section_content.is_empty()
                            {
                                section_counter += 1;
                                fmt_buf.clear();
                                let _ = write!(fmt_buf, "section_{}", section_counter);
                                book.add_chapter(Chapter {
                                    title: current_section_title.take(),
                                    content: std::mem::take(&mut current_section_content),
                                    id: Some(fmt_buf.clone()),
                                });
                            }
                        } else if tag == "body" {
                            in_body = false;
                        }
                    }

                    // Truncate path_buf back to parent.
                    if let Some(pos) = path_buf.rfind('/') {
                        path_buf.truncate(pos);
                    } else {
                        path_buf.clear();
                    }
                    current_text.clear();
                },
                Ok(Event::Empty(ref e)) => {
                    if in_body && e.name().as_ref() == b"empty-line" {
                        current_section_content.push_str("<br/>\n");
                    }
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(EruditioError::Parse(format!("XML error: {}", e))),
                _ => (),
            }
            buf.clear();
        }

        Ok(book)
    }
}

/// FB2 format writer.
#[derive(Default)]
pub struct Fb2Writer;

impl Fb2Writer {
    pub fn new() -> Self {
        Self
    }
}

impl FormatWriter for Fb2Writer {
    fn write_book(&self, book: &Book, writer: &mut dyn Write) -> Result<()> {
        let xml = generate_fb2(book);
        writer.write_all(xml.as_bytes())?;
        Ok(())
    }
}

#[inline]
fn close_inline_formatting(buf: &mut String, in_strong: bool, in_emphasis: bool) {
    if in_strong {
        buf.push_str("</strong>");
    }
    if in_emphasis {
        buf.push_str("</emphasis>");
    }
}

#[inline]
fn reopen_inline_formatting(buf: &mut String, in_emphasis: bool, in_strong: bool) {
    if in_emphasis {
        buf.push_str("<emphasis>");
    }
    if in_strong {
        buf.push_str("<strong>");
    }
}

/// Writes FB2 `<author>` elements for each author in the slice.
///
/// Each author string is split on the first space into `<first-name>` and
/// `<last-name>`. If there is no space, only `<first-name>` is emitted.
/// When the slice is empty a single `<author><first-name>Unknown</first-name></author>`
/// fallback is written.
fn write_fb2_author_elements(xml: &mut String, authors: &[String], indent: &str) {
    for author in authors {
        xml.push_str(indent);
        xml.push_str("<author>\n");
        if let Some((first, last)) = author.split_once(' ') {
            xml.push_str(indent);
            xml.push_str("  <first-name>");
            push_escape_html(xml, first);
            xml.push_str("</first-name>\n");
            xml.push_str(indent);
            xml.push_str("  <last-name>");
            push_escape_html(xml, last);
            xml.push_str("</last-name>\n");
        } else {
            xml.push_str(indent);
            xml.push_str("  <first-name>");
            push_escape_html(xml, author);
            xml.push_str("</first-name>\n");
        }
        xml.push_str(indent);
        xml.push_str("</author>\n");
    }
    if authors.is_empty() {
        xml.push_str(indent);
        xml.push_str("<author><first-name>Unknown</first-name></author>\n");
    }
}

/// Case-insensitive ASCII `starts_with` — zero allocation.
#[inline]
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Case-insensitive ASCII equality — zero allocation.
#[inline]
fn eq_ci(s: &str, target: &str) -> bool {
    s.len() == target.len() && s.as_bytes().eq_ignore_ascii_case(target.as_bytes())
}

/// Checks whether an opening HTML tag has an attribute that marks it as a
/// Project Gutenberg page-header or page-footer block (which should be
/// suppressed in FB2 output).
///
/// Matches: `id="pg-header"`, `id="pg-footer"`, or a `class` attribute
/// containing the word `pgheader`.
fn is_pg_boilerplate_tag(tag_str: &str) -> bool {
    if contains_ascii_ci(tag_str, "id=\"pg-header\"")
        || contains_ascii_ci(tag_str, "id=\"pg-footer\"")
        || contains_ascii_ci(tag_str, "id='pg-header'")
        || contains_ascii_ci(tag_str, "id='pg-footer'")
    {
        return true;
    }
    // Check class attribute for "pgheader"
    if contains_ascii_ci(tag_str, "pgheader") {
        return true;
    }
    false
}

/// Converts HTML content into FB2 paragraph elements.
///
/// - Wraps text inside `<p>` tags as FB2 `<p>` elements.
/// - Converts `<a href="...">text</a>` to `<a l:href="...">text</a>`.
/// - Emits `<empty-line/>` only for explicit `<br>` / `<br/>` tags in the source,
///   NOT after every paragraph boundary.
/// - Text outside any `<p>` is accumulated per block element (not split
///   per-newline) to avoid paragraph inflation.
/// - Skips `<head>` content (e.g. `<title>` text from XHTML pages).
/// - Skips Project Gutenberg page-header/footer boilerplate divs.
fn html_to_fb2_paragraphs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    // We accumulate inline content (text + converted links) per paragraph.
    // When we encounter </p> or end-of-input, we flush the paragraph.
    let mut inline_buf = String::new();
    let mut in_p = false;
    let mut in_anchor = false;
    let mut in_emphasis = false;
    let mut in_strong = false;
    // Track depth inside <head> to suppress page-title text injection.
    let mut head_depth: u32 = 0;
    // Track depth inside Project Gutenberg boilerplate divs (pg-header/pg-footer).
    let mut pg_boilerplate_depth: u32 = 0;
    // Track depth inside block-level elements (<div>) to avoid
    // splitting their text content into one paragraph per newline.
    let mut block_depth: u32 = 0;

    while pos < len {
        if bytes[pos] == b'<' {
            // Parse the tag
            if let Some(gt) = memchr::memchr(b'>', &bytes[pos..]) {
                let tag_bytes = &bytes[pos..pos + gt + 1];
                let tag_str = std::str::from_utf8(tag_bytes).unwrap_or("");

                // --- <head> / </head>: skip everything in the HTML head ---
                if starts_with_ci(tag_str, "<head")
                    && (tag_bytes.len() < 6 || tag_bytes[5] == b'>' || tag_bytes[5] == b' ')
                {
                    head_depth += 1;
                    pos += gt + 1;
                    continue;
                }
                if starts_with_ci(tag_str, "</head") {
                    head_depth = head_depth.saturating_sub(1);
                    pos += gt + 1;
                    continue;
                }
                if head_depth > 0 {
                    // Inside <head>: skip all tags and text
                    pos += gt + 1;
                    continue;
                }

                // --- Skip <html>, </html>, <body>, </body>, <!DOCTYPE>, <?xml?> etc. ---
                if starts_with_ci(tag_str, "<html")
                    || starts_with_ci(tag_str, "</html")
                    || starts_with_ci(tag_str, "<body")
                    || starts_with_ci(tag_str, "</body")
                    || starts_with_ci(tag_str, "<!")
                    || starts_with_ci(tag_str, "<?")
                {
                    pos += gt + 1;
                    continue;
                }

                // --- Project Gutenberg boilerplate div tracking ---
                let is_div_open = starts_with_ci(tag_str, "<div")
                    && (tag_bytes.len() < 5 || tag_bytes[4] == b'>' || tag_bytes[4] == b' ');
                let is_div_close = starts_with_ci(tag_str, "</div");
                if is_div_open && pg_boilerplate_depth > 0 {
                    // Nested div inside boilerplate — increase depth
                    pg_boilerplate_depth += 1;
                    pos += gt + 1;
                    continue;
                }
                if is_div_open && is_pg_boilerplate_tag(tag_str) {
                    pg_boilerplate_depth = 1;
                    pos += gt + 1;
                    continue;
                }
                if is_div_close && pg_boilerplate_depth > 0 {
                    pg_boilerplate_depth = pg_boilerplate_depth.saturating_sub(1);
                    pos += gt + 1;
                    continue;
                }
                if pg_boilerplate_depth > 0 {
                    // Inside a PG boilerplate block: skip everything
                    pos += gt + 1;
                    continue;
                }

                if starts_with_ci(tag_str, "<p")
                    && (tag_bytes.len() < 3 || tag_bytes[2] == b'>' || tag_bytes[2] == b' ')
                {
                    // Opening <p> tag – start accumulating inline content
                    close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    in_p = true;
                    pos += gt + 1;
                } else if starts_with_ci(tag_str, "</p") {
                    // Closing </p> tag – flush current paragraph
                    close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    in_p = false;
                    pos += gt + 1;
                } else if starts_with_ci(tag_str, "<br") {
                    // <br> or <br/> handling depends on context:
                    // Inside a <p> or block element: treat as soft break (space)
                    // Outside: emit empty-line in FB2
                    if in_p || block_depth > 0 {
                        // Soft break within a paragraph/block – just add a space if needed
                        let trimmed = inline_buf.trim_end();
                        if !trimmed.is_empty() && !trimmed.ends_with('>') {
                            inline_buf.push(' ');
                        }
                    } else {
                        close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                        if in_anchor {
                            inline_buf.push_str("</a>");
                            in_anchor = false;
                        }
                        flush_paragraph(&mut out, &mut inline_buf);
                        out.push_str("      <empty-line/>\n");
                        reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    }
                    pos += gt + 1;
                } else if starts_with_ci(tag_str, "<a ") || starts_with_ci(tag_str, "<a>") {
                    // Opening <a> tag – extract href and convert to l:href
                    if let Some(href) = extract_href(tag_str) {
                        // Only emit link for external URLs; internal EPUB references
                        // are meaningless in FB2 context
                        if is_external_url(href) {
                            inline_buf.push_str("<a l:href=\"");
                            push_escape_html(&mut inline_buf, href);
                            inline_buf.push_str("\">");
                            in_anchor = true;
                        }
                        // else: skip the <a> tag, text content will flow through as plain text
                    }
                    // If no href, just skip the tag (keep the text content)
                    pos += gt + 1;
                } else if starts_with_ci(tag_str, "</a") {
                    // Closing </a> tag
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    pos += gt + 1;
                } else if tag_str.len() >= 4
                    && tag_str.as_bytes()[1].eq_ignore_ascii_case(&b'h')
                    && matches!(tag_str.as_bytes()[2], b'1'..=b'6')
                    && (tag_str.as_bytes()[3] == b'>' || tag_str.as_bytes()[3] == b' ')
                    && tag_str.as_bytes()[1] != b'/'
                {
                    // Opening <h1>..<h6> tag – treat as paragraph boundary + wrap in <strong>
                    close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    inline_buf.push_str("<strong>");
                    in_p = true;
                    pos += gt + 1;
                } else if tag_str.len() >= 5
                    && tag_str.as_bytes()[1] == b'/'
                    && tag_str.as_bytes()[2].eq_ignore_ascii_case(&b'h')
                    && matches!(tag_str.as_bytes()[3], b'1'..=b'6')
                    && tag_str.as_bytes()[4] == b'>'
                {
                    // Closing </h1>..</h6> tag
                    inline_buf.push_str("</strong>");
                    close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                    if in_anchor {
                        inline_buf.push_str("</a>");
                        in_anchor = false;
                    }
                    flush_paragraph(&mut out, &mut inline_buf);
                    reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    in_p = false;
                    pos += gt + 1;
                } else if eq_ci(tag_str, "<b>")
                    || eq_ci(tag_str, "<strong>")
                    || starts_with_ci(tag_str, "<b ")
                    || starts_with_ci(tag_str, "<strong ")
                {
                    // Opening bold tag → FB2 <strong>
                    inline_buf.push_str("<strong>");
                    in_strong = true;
                    pos += gt + 1;
                } else if eq_ci(tag_str, "</b>") || eq_ci(tag_str, "</strong>") {
                    // Closing bold tag
                    inline_buf.push_str("</strong>");
                    in_strong = false;
                    pos += gt + 1;
                } else if eq_ci(tag_str, "<i>")
                    || eq_ci(tag_str, "<em>")
                    || starts_with_ci(tag_str, "<i ")
                    || starts_with_ci(tag_str, "<em ")
                {
                    // Opening italic tag → FB2 <emphasis>
                    inline_buf.push_str("<emphasis>");
                    in_emphasis = true;
                    pos += gt + 1;
                } else if eq_ci(tag_str, "</i>") || eq_ci(tag_str, "</em>") {
                    // Closing italic tag
                    inline_buf.push_str("</emphasis>");
                    in_emphasis = false;
                    pos += gt + 1;
                } else if is_div_open {
                    // Opening <div> tag – treat as block boundary; flush any
                    // pending text and start accumulating within this block so
                    // that multi-line text inside a single <div> stays in one
                    // paragraph instead of being split per-newline.
                    if !in_p {
                        close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                        if in_anchor {
                            inline_buf.push_str("</a>");
                            in_anchor = false;
                        }
                        flush_paragraph(&mut out, &mut inline_buf);
                        reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    }
                    block_depth += 1;
                    pos += gt + 1;
                } else if is_div_close {
                    // Closing </div> – flush accumulated block content
                    block_depth = block_depth.saturating_sub(1);
                    if !in_p && block_depth == 0 {
                        close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
                        if in_anchor {
                            inline_buf.push_str("</a>");
                            in_anchor = false;
                        }
                        flush_paragraph(&mut out, &mut inline_buf);
                        reopen_inline_formatting(&mut inline_buf, in_emphasis, in_strong);
                    }
                    pos += gt + 1;
                } else {
                    // Other tags (e.g. <span>, <ul>, <ol>, <table>, etc.) – skip the tag, keep going
                    pos += gt + 1;
                }
            } else {
                // Unclosed '<' – treat as text
                push_escape_html(&mut inline_buf, &html[pos..pos + 1]);
                pos += 1;
            }
        } else {
            // Regular text content
            if head_depth > 0 || pg_boilerplate_depth > 0 {
                // Inside <head> or PG boilerplate: skip text entirely
                let next_lt = memchr::memchr(b'<', &bytes[pos..]).unwrap_or(len - pos);
                pos += next_lt;
                continue;
            }
            let next_lt = memchr::memchr(b'<', &bytes[pos..]).unwrap_or(len - pos);
            let text = &html[pos..pos + next_lt];
            if in_p {
                // Inside a <p>: accumulate text as-is (preserving whitespace
                // that is meaningful for inline formatting boundaries).
                push_escape_html(&mut inline_buf, text);
            } else if block_depth > 0 {
                // Inside a block element (<div>): accumulate text, joining
                // newlines with spaces to avoid paragraph splits.
                for (i, segment) in text.split('\n').enumerate() {
                    if i > 0 {
                        let buf_trimmed = inline_buf.trim_end();
                        if !buf_trimmed.is_empty() && !buf_trimmed.ends_with('>') {
                            inline_buf.push(' ');
                        }
                    }
                    let trimmed = segment.trim();
                    if !trimmed.is_empty() {
                        push_escape_html(&mut inline_buf, trimmed);
                    }
                }
            } else {
                // Outside <p> and outside block elements: accumulate text as a
                // single implicit paragraph (join lines with spaces instead of
                // flushing each line as a separate paragraph).
                for (i, line) in text.split('\n').enumerate() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if i > 0 {
                            let buf_trimmed = inline_buf.trim_end();
                            if !buf_trimmed.is_empty() && !buf_trimmed.ends_with('>') {
                                inline_buf.push(' ');
                            }
                        }
                        push_escape_html(&mut inline_buf, trimmed);
                    }
                }
            }
            pos += next_lt;
        }
    }

    // Flush any trailing inline content
    close_inline_formatting(&mut inline_buf, in_strong, in_emphasis);
    if in_anchor {
        inline_buf.push_str("</a>");
        // in_anchor = false; // not needed, end of function
    }
    flush_paragraph(&mut out, &mut inline_buf);
    out
}

/// If the inline buffer has content, wrap it in `<p>...</p>` and append to `out`.
fn flush_paragraph(out: &mut String, inline_buf: &mut String) {
    let trimmed = inline_buf.trim();
    if !trimmed.is_empty() {
        // Quick check: only do the expensive scan if content could be markup-only
        let is_empty_markup = trimmed.starts_with('<') && is_only_empty_markup(trimmed.as_bytes());
        if !is_empty_markup {
            out.push_str("      <p>");
            out.push_str(trimmed);
            out.push_str("</p>\n");
        }
    }
    inline_buf.clear();
}

/// Returns `true` if `s` contains only `<emphasis>`, `</emphasis>`,
/// `<strong>`, `</strong>` tags and whitespace — i.e., no actual text content.
/// Single-pass, zero-allocation replacement for the 4× `.replace()` chain.
fn is_only_empty_markup(bytes: &[u8]) -> bool {
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b'<' => {
                if i + 10 <= len && bytes[i..i + 10] == *b"<emphasis>" {
                    i += 10;
                } else if i + 11 <= len && bytes[i..i + 11] == *b"</emphasis>" {
                    i += 11;
                } else if i + 8 <= len && bytes[i..i + 8] == *b"<strong>" {
                    i += 8;
                } else if i + 9 <= len && bytes[i..i + 9] == *b"</strong>" {
                    i += 9;
                } else {
                    return false;
                }
            },
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => return false,
        }
    }
    true
}

/// Extracts the `href` attribute value from an `<a ...>` tag string.
fn extract_href(tag: &str) -> Option<&str> {
    let href_pos = find_case_insensitive(tag.as_bytes(), b"href=")?;
    let after_eq = href_pos + 5; // length of "href="
    let bytes = tag.as_bytes();
    if after_eq >= bytes.len() {
        return None;
    }
    let quote = bytes[after_eq];
    if quote == b'"' || quote == b'\'' {
        let start = after_eq + 1;
        let end = memchr::memchr(quote, &bytes[start..])?;
        Some(&tag[start..start + end])
    } else {
        // Unquoted value – take until whitespace or '>'
        let start = after_eq;
        let end = tag[start..]
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(tag.len() - start);
        Some(&tag[start..start + end])
    }
}

/// Returns true if the URL is an external link (http, https, ftp, mailto).
fn is_external_url(url: &str) -> bool {
    starts_with_ci(url, "http://")
        || starts_with_ci(url, "https://")
        || starts_with_ci(url, "ftp://")
        || starts_with_ci(url, "mailto:")
}

/// Generates a deterministic UUID-like identifier from book metadata.
///
/// Uses FNV-1a hashing (stable across Rust versions, unlike `DefaultHasher`)
/// of the title, authors, and language to produce a reproducible ID.
fn generate_document_id(book: &Book) -> String {
    // FNV-1a 64-bit: offset_basis and prime are fixed constants.
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;

    let mut h = FNV_OFFSET;
    for b in book.metadata.title.as_deref().unwrap_or("").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    for author in &book.metadata.authors {
        h ^= 0xFF; // separator
        h = h.wrapping_mul(FNV_PRIME);
        for b in author.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    // Mix in language for additional differentiation.
    h ^= 0xFE; // separator
    h = h.wrapping_mul(FNV_PRIME);
    for b in book.metadata.language.as_deref().unwrap_or("").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }

    // Derive a second 64-bit value by continuing the hash with a different seed byte.
    let mut h2 = h;
    h2 ^= 0xFD;
    h2 = h2.wrapping_mul(FNV_PRIME);

    // Format as UUID-like: 8-4-4-4-12
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (h >> 32) as u32,
        (h >> 16) as u16,
        (h & 0xFFFF) as u16,
        (h2 >> 48) as u16,
        h2 & 0xFFFF_FFFF_FFFF
    )
}

/// Generates a complete FictionBook 2.0 XML document from a `Book`.
fn generate_fb2(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\" ");
    xml.push_str("xmlns:l=\"http://www.w3.org/1999/xlink\">\n");

    // Description / title-info
    xml.push_str("  <description>\n");
    xml.push_str("    <title-info>\n");

    // Genre (FB2 requires at least one)
    if let Some(subject) = book.metadata.subjects.first() {
        xml.push_str("      <genre>");
        push_escape_html(&mut xml, subject);
        xml.push_str("</genre>\n");
    } else {
        xml.push_str("      <genre>other</genre>\n");
    }

    // Authors
    write_fb2_author_elements(&mut xml, &book.metadata.authors, "      ");

    // Title
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");
    xml.push_str("      <book-title>");
    push_escape_html(&mut xml, title);
    xml.push_str("</book-title>\n");

    // Language
    if let Some(ref lang) = book.metadata.language {
        xml.push_str("      <lang>");
        push_escape_html(&mut xml, lang);
        xml.push_str("</lang>\n");
    }

    // Annotation (description)
    if let Some(ref desc) = book.metadata.description {
        xml.push_str("      <annotation>\n");
        for line in desc.lines() {
            xml.push_str("        <p>");
            push_escape_html(&mut xml, line);
            xml.push_str("</p>\n");
        }
        xml.push_str("      </annotation>\n");
    }

    // Keywords from subjects (comma-separated, matching Calibre behavior)
    if !book.metadata.subjects.is_empty() {
        xml.push_str("      <keywords>");
        for (i, s) in book.metadata.subjects.iter().enumerate() {
            if i > 0 {
                xml.push_str(", ");
            }
            push_escape_html(&mut xml, s);
        }
        xml.push_str("</keywords>\n");
    }

    // Coverpage – look for a cover image in the manifest.
    // First check metadata.cover_image_id, then fall back to heuristic search.
    let cover_id = book
        .metadata
        .cover_image_id
        .as_ref()
        .and_then(|cid| {
            book.manifest
                .get(cid)
                .filter(|item| item.media_type.starts_with("image/"))
                .map(|item| item.id.as_str())
        })
        .or_else(|| {
            book.manifest
                .iter()
                .find(|item| {
                    contains_ascii_ci(&item.id, "cover") && item.media_type.starts_with("image/")
                })
                .map(|item| item.id.as_str())
        });
    if let Some(cid) = cover_id {
        xml.push_str("      <coverpage><image l:href=\"#");
        push_escape_html(&mut xml, cid);
        xml.push_str("\"/></coverpage>\n");
    }

    xml.push_str("    </title-info>\n");

    // Document-info (metadata about this conversion)
    xml.push_str("    <document-info>\n");
    // Copy book author(s) into document-info (matching Calibre behavior)
    write_fb2_author_elements(&mut xml, &book.metadata.authors, "      ");
    xml.push_str("      <program-used>eruditio</program-used>\n");
    xml.push_str("      <date>");
    xml.push_str(&chrono::Utc::now().format("%Y-%m-%d").to_string());
    xml.push_str("</date>\n");
    xml.push_str("      <id>");
    xml.push_str(&generate_document_id(book));
    xml.push_str("</id>\n");
    xml.push_str("      <version>1.0</version>\n");
    xml.push_str("    </document-info>\n");

    // Publish-info (publisher, isbn, year)
    let has_publisher = book.metadata.publisher.is_some();
    let has_isbn = book.metadata.isbn.is_some();
    let has_pub_date = book.metadata.publication_date.is_some();
    if has_publisher || has_isbn || has_pub_date {
        xml.push_str("    <publish-info>\n");
        if let Some(ref publisher) = book.metadata.publisher {
            xml.push_str("      <publisher>");
            push_escape_html(&mut xml, publisher);
            xml.push_str("</publisher>\n");
        }
        if let Some(ref isbn) = book.metadata.isbn {
            xml.push_str("      <isbn>");
            push_escape_html(&mut xml, isbn);
            xml.push_str("</isbn>\n");
        }
        if let Some(ref pub_date) = book.metadata.publication_date {
            xml.push_str("      <year>");
            xml.push_str(&pub_date.format("%Y").to_string());
            xml.push_str("</year>\n");
        }
        xml.push_str("    </publish-info>\n");
    }

    xml.push_str("  </description>\n");

    // Body
    // Note: the cover image is referenced ONLY in the <coverpage> element
    // inside <title-info>. We deliberately do NOT duplicate it as an <image>
    // in the body — Calibre 9.6 uses a single coverpage reference.
    xml.push_str("  <body>\n");

    // Track whether we've written at least one section, so we can insert
    // <empty-line/> elements between sections for visual spacing (matching
    // Calibre's use of <empty-line/> for section separation).
    let mut section_written = false;

    for chapter in &book.chapter_views() {
        // Convert HTML content to FB2 paragraphs.
        let fb2_content = html_to_fb2_paragraphs(&chapter.content);

        // Skip chapters that produce no visible content after conversion
        // (e.g. the cover page wrapper whose only content is an <img> tag,
        // or chapters whose XHTML only contains navigation/boilerplate).
        if fb2_content.trim().is_empty() && chapter.title.is_none() {
            continue;
        }

        xml.push_str("    <section>\n");
        if let Some(ch_title) = chapter.title {
            xml.push_str("      <title><p>");
            push_escape_html(&mut xml, ch_title);
            xml.push_str("</p></title>\n");
        }
        // Insert an <empty-line/> at the start of each section (after title)
        // for visual spacing between sections, matching Calibre's behavior.
        // <empty-line/> must be inside <section>, not between sections.
        if section_written {
            xml.push_str("      <empty-line/>\n");
        }
        xml.push_str(&fb2_content);
        xml.push_str("    </section>\n");
        section_written = true;
    }
    xml.push_str("  </body>\n");

    // Binary resources (base64-encoded)
    for resource in &book.resources() {
        // Skip CSS resources — FB2 readers don't use CSS
        if resource.media_type == "text/css" {
            continue;
        }
        xml.push_str("  <binary id=\"");
        push_escape_html(&mut xml, resource.id);
        xml.push_str("\" content-type=\"");
        push_escape_html(&mut xml, resource.media_type);
        xml.push_str("\">");
        base64::engine::general_purpose::STANDARD.encode_string(resource.data, &mut xml);
        xml.push_str("</binary>\n");
    }

    xml.push_str("</FictionBook>\n");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn fb2_writer_produces_valid_xml() {
        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.language = Some("en".into());

        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello world</p>".into(),
            id: Some("ch1".into()),
        });

        book.add_resource("img1", "images/test.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<book-title>Test Book</book-title>"));
        assert!(xml.contains("<first-name>Jane</first-name>"));
        assert!(xml.contains("<last-name>Doe</last-name>"));
        assert!(xml.contains("<lang>en</lang>"));
        assert!(xml.contains("<p>Hello world</p>"));
        assert!(xml.contains("content-type=\"image/jpeg\""));
        assert!(xml.contains("id=\"img1\""));
    }

    #[test]
    fn fb2_round_trip_preserves_content() {
        let mut book = Book::new();
        book.metadata.title = Some("Round Trip".into());
        book.metadata.authors.push("Author Name".into());

        book.add_chapter(Chapter {
            title: Some("Section One".into()),
            content: "<p>First paragraph</p><p>Second paragraph</p>".into(),
            id: Some("s1".into()),
        });

        // Write to FB2
        let mut fb2_bytes = Vec::new();
        Fb2Writer::new().write_book(&book, &mut fb2_bytes).unwrap();

        // Read back
        let mut cursor = Cursor::new(fb2_bytes);
        let decoded = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.title.as_deref(), Some("Round Trip"));
        let chapters = decoded.chapters();
        assert!(!chapters.is_empty());
        assert_eq!(chapters[0].title.as_deref(), Some("Section One"));
    }

    #[test]
    fn fb2_writer_generates_publish_info() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.publisher = Some("Test Press".into());
        book.metadata.isbn = Some("978-0-123456-78-9".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 6, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<publish-info>"), "missing <publish-info>");
        assert!(
            xml.contains("<publisher>Test Press</publisher>"),
            "missing publisher"
        );
        assert!(
            xml.contains("<isbn>978-0-123456-78-9</isbn>"),
            "missing isbn"
        );
        assert!(xml.contains("<year>2024</year>"), "missing year");
        assert!(xml.contains("</publish-info>"), "missing </publish-info>");
    }

    #[test]
    fn fb2_writer_omits_publish_info_when_empty() {
        let mut book = Book::new();
        book.metadata.title = Some("No Publish Info".into());

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<publish-info>"),
            "publish-info should not be present when all fields are None"
        );
    }

    #[test]
    fn fb2_writer_partial_publish_info() {
        let mut book = Book::new();
        book.metadata.title = Some("Partial".into());
        book.metadata.publisher = Some("Only Publisher".into());
        // isbn and publication_date are None

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<publish-info>"));
        assert!(xml.contains("<publisher>Only Publisher</publisher>"));
        assert!(!xml.contains("<isbn>"));
        assert!(!xml.contains("<year>"));
    }

    #[test]
    fn fb2_reader_parses_publish_info() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Parsed Book</book-title>
    </title-info>
    <publish-info>
      <publisher>Acme Publishing</publisher>
      <isbn>978-1-234567-89-0</isbn>
      <year>2023</year>
    </publish-info>
  </description>
  <body>
    <section>
      <title><p>Ch1</p></title>
      <p>Content here</p>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(book.metadata.title.as_deref(), Some("Parsed Book"));
        assert_eq!(book.metadata.publisher.as_deref(), Some("Acme Publishing"));
        assert_eq!(book.metadata.isbn.as_deref(), Some("978-1-234567-89-0"));
        assert!(book.metadata.publication_date.is_some());
        assert_eq!(
            book.metadata
                .publication_date
                .unwrap()
                .format("%Y")
                .to_string(),
            "2023"
        );
    }

    #[test]
    fn fb2_publish_info_round_trip() {
        use chrono::NaiveDate;

        let mut book = Book::new();
        book.metadata.title = Some("Round Trip Publish".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.publisher = Some("Test Press".into());
        book.metadata.isbn = Some("978-0-123456-78-9".into());
        book.metadata.publication_date = Some(
            NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );

        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        // Write
        let mut fb2_bytes = Vec::new();
        Fb2Writer::new().write_book(&book, &mut fb2_bytes).unwrap();

        // Read back
        let mut cursor = Cursor::new(fb2_bytes);
        let decoded = Fb2Reader::new().read_book(&mut cursor).unwrap();

        assert_eq!(decoded.metadata.publisher.as_deref(), Some("Test Press"));
        assert_eq!(decoded.metadata.isbn.as_deref(), Some("978-0-123456-78-9"));
        assert!(decoded.metadata.publication_date.is_some());
        assert_eq!(
            decoded
                .metadata
                .publication_date
                .unwrap()
                .format("%Y")
                .to_string(),
            "2024"
        );
    }

    // =========================================================================
    // New tests for the 5 FB2 writer enhancements
    // =========================================================================

    #[test]
    fn fb2_writer_includes_document_info() {
        let mut book = Book::new();
        book.metadata.title = Some("Doc Info Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<document-info>"), "missing <document-info>");
        assert!(
            xml.contains("<program-used>eruditio</program-used>"),
            "missing program-used"
        );
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let expected_date = format!("<date>{}</date>", today);
        assert!(
            xml.contains(&expected_date),
            "missing current date in document-info, expected {}, got:\n{}",
            expected_date,
            xml
        );
        assert!(xml.contains("</document-info>"), "missing </document-info>");

        // Verify ordering: document-info comes after title-info and before publish-info / </description>
        let ti_end = xml.find("</title-info>").expect("no </title-info>");
        let di_start = xml.find("<document-info>").expect("no <document-info>");
        let desc_end = xml.find("</description>").expect("no </description>");
        assert!(
            di_start > ti_end,
            "document-info should come after title-info"
        );
        assert!(
            di_start < desc_end,
            "document-info should come before </description>"
        );
    }

    #[test]
    fn fb2_writer_includes_genre_from_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("Genre Test".into());
        book.metadata.subjects.push("science_fiction".into());
        book.metadata.subjects.push("adventure".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<genre>science_fiction</genre>"),
            "should use first subject as genre, got: {}",
            xml
        );
        // Genre should appear before <author>
        let genre_pos = xml.find("<genre>").unwrap();
        let author_pos = xml.find("<author>").unwrap();
        assert!(genre_pos < author_pos, "genre should appear before author");
    }

    #[test]
    fn fb2_writer_includes_default_genre_when_no_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("Default Genre".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<genre>other</genre>"),
            "should use 'other' as default genre"
        );
    }

    #[test]
    fn fb2_writer_includes_coverpage_when_cover_image_exists() {
        let mut book = Book::new();
        book.metadata.title = Some("Cover Test".into());
        book.add_resource("cover", "images/cover.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<coverpage><image l:href=\"#cover\"/></coverpage>"),
            "missing coverpage element, got:\n{}",
            xml
        );
        // Coverpage should be inside title-info
        let ti_start = xml.find("<title-info>").unwrap();
        let ti_end = xml.find("</title-info>").unwrap();
        let cp_pos = xml.find("<coverpage>").unwrap();
        assert!(
            cp_pos > ti_start && cp_pos < ti_end,
            "coverpage should be inside title-info"
        );
    }

    #[test]
    fn fb2_writer_omits_coverpage_when_no_cover_image() {
        let mut book = Book::new();
        book.metadata.title = Some("No Cover".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<coverpage>"),
            "coverpage should not be present without a cover image"
        );
    }

    #[test]
    fn fb2_writer_preserves_inline_formatting() {
        let mut book = Book::new();
        book.metadata.title = Some("Formatting Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>This is <b>bold</b> and <i>italic</i> text.</p><p>Also <strong>strong</strong> and <em>emphasis</em>.</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<strong>bold</strong>"),
            "HTML <b> should become FB2 <strong>, got:\n{xml}"
        );
        assert!(
            xml.contains("<emphasis>italic</emphasis>"),
            "HTML <i> should become FB2 <emphasis>, got:\n{xml}"
        );
        assert!(
            xml.contains("<strong>strong</strong>"),
            "HTML <strong> should become FB2 <strong>, got:\n{xml}"
        );
        assert!(
            xml.contains("<emphasis>emphasis</emphasis>"),
            "HTML <em> should become FB2 <emphasis>, got:\n{xml}"
        );
    }

    #[test]
    fn fb2_writer_converts_hyperlinks() {
        let mut book = Book::new();
        book.metadata.title = Some("Link Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>Click <a href="http://example.com">here</a> for more.</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains(r#"<a l:href="http://example.com">here</a>"#),
            "hyperlinks should be converted to l:href format, got:\n{}",
            xml
        );
        assert!(
            xml.contains("Click "),
            "text before link should be preserved"
        );
        assert!(
            xml.contains(" for more."),
            "text after link should be preserved"
        );
    }

    #[test]
    fn fb2_writer_closes_anchor_at_paragraph_boundary() {
        let mut book = Book::new();
        book.metadata.title = Some("Anchor Close Test".into());
        // Simulate a link that spans across a paragraph boundary:
        // the </a> comes after the </p>, so the writer must auto-close it.
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p><a href="https://example.org">link text</p><p>next paragraph</p>"#
                .into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The anchor must be closed before the paragraph closes
        assert!(
            xml.contains(r#"<a l:href="https://example.org">link text</a></p>"#),
            "anchor tag should be closed before </p>, got:\n{}",
            xml
        );
        // The output must not contain an unclosed <a> tag
        assert!(
            !xml.contains(r#"<a l:href="https://example.org">link text</p>"#),
            "must not have unclosed <a> tag, got:\n{}",
            xml
        );
        // Validate the XML is well-formed
        assert!(
            xml.contains("next paragraph"),
            "subsequent paragraph content should be preserved"
        );
    }

    #[test]
    fn fb2_writer_no_excessive_empty_lines() {
        let mut book = Book::new();
        book.metadata.title = Some("Empty Line Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>First paragraph</p><p>Second paragraph</p><p>Third paragraph</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let empty_line_count = xml.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 0,
            "consecutive <p> tags should NOT produce empty-lines, but found {}",
            empty_line_count
        );
        // All three paragraphs should be present
        assert!(xml.contains("<p>First paragraph</p>"));
        assert!(xml.contains("<p>Second paragraph</p>"));
        assert!(xml.contains("<p>Third paragraph</p>"));
    }

    #[test]
    fn fb2_writer_emits_empty_line_for_br() {
        let mut book = Book::new();
        book.metadata.title = Some("BR Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Before break</p><br/><p>After break</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let empty_line_count = xml.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 1,
            "a <br/> between paragraphs should produce exactly one empty-line, got {}",
            empty_line_count
        );
    }

    // =========================================================================
    // Tests for nested section handling in FB2 reader
    // =========================================================================

    #[test]
    fn fb2_reader_nested_sections() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Nested Test</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Chapter 1</p></title>
      <section>
        <title><p>Section 1.1</p></title>
        <p>Content of 1.1</p>
      </section>
      <section>
        <title><p>Section 1.2</p></title>
        <p>Content of 1.2</p>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        // The outer section has a title but the content was flushed when the
        // first child section started, producing a chapter for "Chapter 1"
        // (empty body) and one chapter per inner section.
        assert!(
            chapters.len() >= 2,
            "expected at least 2 chapters for nested sections, got {}",
            chapters.len()
        );

        // Find chapters by title
        let titles: Vec<Option<&str>> = chapters.iter().map(|c| c.title.as_deref()).collect();
        assert!(
            titles.contains(&Some("Section 1.1")),
            "missing 'Section 1.1' chapter, found titles: {:?}",
            titles
        );
        assert!(
            titles.contains(&Some("Section 1.2")),
            "missing 'Section 1.2' chapter, found titles: {:?}",
            titles
        );

        // Verify inner section content is not dropped
        let sec11 = chapters
            .iter()
            .find(|c| c.title.as_deref() == Some("Section 1.1"))
            .unwrap();
        assert!(
            sec11.content.contains("Content of 1.1"),
            "Section 1.1 content was dropped: {:?}",
            sec11.content
        );
        let sec12 = chapters
            .iter()
            .find(|c| c.title.as_deref() == Some("Section 1.2"))
            .unwrap();
        assert!(
            sec12.content.contains("Content of 1.2"),
            "Section 1.2 content was dropped: {:?}",
            sec12.content
        );
    }

    #[test]
    fn fb2_reader_deeply_nested_sections() {
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Deep Nesting</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Part I</p></title>
      <section>
        <title><p>Chapter 1</p></title>
        <section>
          <title><p>Section 1.1</p></title>
          <p>Deep content here</p>
        </section>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        let titles: Vec<Option<&str>> = chapters.iter().map(|c| c.title.as_deref()).collect();
        assert!(
            titles.contains(&Some("Section 1.1")),
            "deeply nested section title not found, got: {:?}",
            titles
        );

        let sec = chapters
            .iter()
            .find(|c| c.title.as_deref() == Some("Section 1.1"))
            .unwrap();
        assert!(
            sec.content.contains("Deep content here"),
            "deeply nested section content was dropped: {:?}",
            sec.content
        );
    }

    #[test]
    fn fb2_reader_flat_sections_still_work() {
        // Regression test: flat (non-nested) sections must keep working.
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Flat Sections</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Chapter 1</p></title>
      <p>First chapter content</p>
    </section>
    <section>
      <title><p>Chapter 2</p></title>
      <p>Second chapter content</p>
    </section>
    <section>
      <title><p>Chapter 3</p></title>
      <p>Third chapter content</p>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        assert_eq!(
            chapters.len(),
            3,
            "expected 3 flat chapters, got {}",
            chapters.len()
        );
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[1].title.as_deref(), Some("Chapter 2"));
        assert_eq!(chapters[2].title.as_deref(), Some("Chapter 3"));
        assert!(chapters[0].content.contains("First chapter content"));
        assert!(chapters[1].content.contains("Second chapter content"));
        assert!(chapters[2].content.contains("Third chapter content"));
    }

    #[test]
    fn fb2_reader_nested_section_with_parent_content() {
        // A parent section has content before its nested child sections.
        let fb2_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description>
    <title-info>
      <book-title>Mixed Content</book-title>
    </title-info>
  </description>
  <body>
    <section>
      <title><p>Introduction</p></title>
      <p>Intro paragraph</p>
      <section>
        <title><p>Details</p></title>
        <p>Detail paragraph</p>
      </section>
    </section>
  </body>
</FictionBook>"#;

        let mut cursor = Cursor::new(fb2_xml.as_bytes());
        let book = Fb2Reader::new().read_book(&mut cursor).unwrap();
        let chapters = book.chapters();

        // The parent section's content should be flushed as a chapter before
        // the child section starts.
        assert!(
            chapters.len() >= 2,
            "expected at least 2 chapters, got {}",
            chapters.len()
        );

        let intro = chapters
            .iter()
            .find(|c| c.title.as_deref() == Some("Introduction"))
            .unwrap();
        assert!(
            intro.content.contains("Intro paragraph"),
            "parent section content was lost: {:?}",
            intro.content
        );

        let details = chapters
            .iter()
            .find(|c| c.title.as_deref() == Some("Details"))
            .unwrap();
        assert!(
            details.content.contains("Detail paragraph"),
            "child section content was lost: {:?}",
            details.content
        );
    }

    #[test]
    fn fb2_writer_strips_internal_epub_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Internal Link Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>See <a href="@public@vhost@g@gutenberg@html@files@11@11-h@11-h-0.htm.html#link2HCH0001">Chapter 1</a> for details.</p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The link wrapper should be stripped; text content preserved
        assert!(
            !xml.contains("<a "),
            "internal EPUB links should be stripped, but found <a> tag in:\n{}",
            xml
        );
        assert!(
            xml.contains("Chapter 1"),
            "link text should be preserved as inline content"
        );
        assert!(xml.contains("See "), "text before link should be preserved");
        assert!(
            xml.contains(" for details."),
            "text after link should be preserved"
        );
    }

    #[test]
    fn fb2_writer_strips_fragment_only_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Fragment Link Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r##"<p>Go to <a href="#section1">Section 1</a> now.</p>"##.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<a "),
            "fragment-only links should be stripped, but found <a> tag in:\n{}",
            xml
        );
        assert!(
            xml.contains("Section 1"),
            "link text should be preserved as inline content"
        );
    }

    #[test]
    fn fb2_writer_preserves_external_links() {
        // Verify that http/https/ftp/mailto links are still emitted
        let mut book = Book::new();
        book.metadata.title = Some("External Link Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p><a href="https://example.com">HTTPS</a> <a href="http://example.com">HTTP</a> <a href="ftp://files.example.com">FTP</a> <a href="mailto:test@example.com">Email</a></p>"#.into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains(r#"<a l:href="https://example.com">HTTPS</a>"#),
            "https links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="http://example.com">HTTP</a>"#),
            "http links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="ftp://files.example.com">FTP</a>"#),
            "ftp links should be preserved"
        );
        assert!(
            xml.contains(r#"<a l:href="mailto:test@example.com">Email</a>"#),
            "mailto links should be preserved"
        );
    }

    #[test]
    fn fb2_writer_strips_relative_path_links() {
        let mut book = Book::new();
        book.metadata.title = Some("Relative Link Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>See <a href="chapter1.xhtml#section1">this section</a> please.</p>"#
                .into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<a "),
            "relative path links should be stripped, got:\n{}",
            xml
        );
        assert!(
            xml.contains("this section"),
            "link text should be preserved"
        );
    }

    // =========================================================================
    // Tests for CSS filtering and cover image in body
    // =========================================================================

    #[test]
    fn fb2_writer_excludes_css_from_binary_elements() {
        let mut book = Book::new();
        book.metadata.title = Some("CSS Filter Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        // Add a CSS resource and an image resource
        book.add_resource(
            "style1",
            "styles/main.css",
            b"body { color: red; }".to_vec(),
            "text/css",
        );
        book.add_resource("img1", "images/photo.jpg", vec![0xFF, 0xD8], "image/jpeg");

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // CSS should NOT appear as a binary element
        assert!(
            !xml.contains("id=\"style1\""),
            "CSS resource should be filtered out of binary elements, got:\n{}",
            xml
        );
        assert!(
            !xml.contains("content-type=\"text/css\""),
            "text/css content-type should not appear in binary elements"
        );

        // Image resource should still be present
        assert!(
            xml.contains("id=\"img1\""),
            "image resource should still be included as binary element"
        );
        assert!(
            xml.contains("content-type=\"image/jpeg\""),
            "image content-type should be present"
        );
    }

    #[test]
    fn fb2_writer_cover_image_in_body() {
        // After the fix, the cover image should appear ONLY in the <coverpage>
        // element inside <title-info>, NOT duplicated in the body.
        let mut book = Book::new();
        book.metadata.title = Some("Cover Body Test".into());
        book.add_resource("cover", "images/cover.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The coverpage in <title-info> should be present
        assert!(
            xml.contains("<coverpage><image l:href=\"#cover\"/></coverpage>"),
            "coverpage should be present in title-info, got:\n{}",
            xml
        );

        // Extract the body section
        let body_start = xml.find("<body>").expect("missing <body>");
        let body_end = xml.find("</body>").expect("missing </body>");
        let body_section = &xml[body_start..body_end];

        // The body should NOT contain a duplicate cover image reference
        assert!(
            !body_section.contains("<image l:href=\"#cover\"/>"),
            "body should NOT contain duplicate cover image reference, got:\n{}",
            body_section
        );

        // Count total <image> references in the entire document
        let image_refs: Vec<_> = xml.match_indices("<image l:href=\"#cover\"/>").collect();
        assert_eq!(
            image_refs.len(),
            1,
            "should have exactly 1 cover <image> reference (coverpage only), got {} in:\n{}",
            image_refs.len(),
            xml
        );
    }

    #[test]
    fn fb2_writer_no_cover_image_in_body_without_cover() {
        let mut book = Book::new();
        book.metadata.title = Some("No Cover Body Test".into());
        // Add a non-cover image resource
        book.add_resource("img1", "images/photo.jpg", vec![0xFF, 0xD8], "image/jpeg");
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Should not have a cover image section in the body
        let body_section = &xml[xml.find("<body>").unwrap()..xml.find("</body>").unwrap()];
        assert!(
            !body_section.contains("<image l:href=\"#"),
            "body should not contain cover image section when no cover exists, got:\n{}",
            body_section
        );
    }

    // =========================================================================
    // Tests for the 5 FB2 writer fixes (headings, coverpage, keywords,
    // document-info id/version, and br-within-paragraph)
    // =========================================================================

    // --- Fix 1: HTML headings (h1-h6) → <strong> ---

    #[test]
    fn fb2_writer_wraps_h1_in_strong() {
        let mut book = Book::new();
        book.metadata.title = Some("Heading Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<h1>Chapter One</h1><p>Body text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<p><strong>Chapter One</strong></p>"),
            "h1 content should be wrapped in <strong>, got:\n{}",
            xml
        );
        assert!(
            xml.contains("<p>Body text</p>"),
            "body text should follow heading as normal paragraph"
        );
    }

    #[test]
    fn fb2_writer_wraps_h2_through_h6_in_strong() {
        for level in 2..=6 {
            let mut book = Book::new();
            book.metadata.title = Some(format!("H{} Test", level));
            let content = format!(
                "<h{}>Heading {}</h{}><p>After heading</p>",
                level, level, level
            );
            book.add_chapter(Chapter {
                title: Some("Ch1".into()),
                content,
                id: Some("ch1".into()),
            });

            let mut output = Vec::new();
            Fb2Writer::new().write_book(&book, &mut output).unwrap();
            let xml = String::from_utf8(output).unwrap();

            let expected = format!("<p><strong>Heading {}</strong></p>", level);
            assert!(
                xml.contains(&expected),
                "h{} content should be wrapped in <strong>, expected '{}', got:\n{}",
                level,
                expected,
                xml
            );
        }
    }

    #[test]
    fn fb2_writer_heading_with_inline_formatting() {
        let mut book = Book::new();
        book.metadata.title = Some("Heading Inline Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<h2>Chapter <em>One</em></h2>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<p><strong>Chapter <emphasis>One</emphasis></strong></p>"),
            "heading with inline formatting should preserve emphasis inside strong, got:\n{}",
            xml
        );
    }

    #[test]
    fn fb2_writer_heading_between_paragraphs() {
        let mut book = Book::new();
        book.metadata.title = Some("Heading Between Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Before</p><h3>Middle Heading</h3><p>After</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(xml.contains("<p>Before</p>"), "paragraph before heading");
        assert!(
            xml.contains("<p><strong>Middle Heading</strong></p>"),
            "heading should be strong-wrapped paragraph, got:\n{}",
            xml
        );
        assert!(xml.contains("<p>After</p>"), "paragraph after heading");
    }

    // --- Fix 2: <coverpage> element referencing cover binary ---

    #[test]
    fn fb2_writer_coverpage_from_metadata_cover_image_id() {
        let mut book = Book::new();
        book.metadata.title = Some("Cover ID Test".into());
        book.metadata.cover_image_id = Some("my-cover-img".into());
        book.add_resource(
            "my-cover-img",
            "images/cover.png",
            vec![0x89, 0x50],
            "image/png",
        );
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<coverpage><image l:href=\"#my-cover-img\"/></coverpage>"),
            "coverpage should reference cover from metadata.cover_image_id, got:\n{}",
            xml
        );
    }

    // --- Fix 3: <keywords> from book subjects ---

    #[test]
    fn fb2_writer_includes_keywords_from_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("Keywords Test".into());
        book.metadata.subjects.push("Fiction".into());
        book.metadata.subjects.push("Adventure".into());
        book.metadata.subjects.push("Classic".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<keywords>Fiction, Adventure, Classic</keywords>"),
            "keywords should be comma-separated subjects, got:\n{}",
            xml
        );
        // Keywords should be inside title-info
        let ti_start = xml.find("<title-info>").unwrap();
        let ti_end = xml.find("</title-info>").unwrap();
        let kw_pos = xml.find("<keywords>").unwrap();
        assert!(
            kw_pos > ti_start && kw_pos < ti_end,
            "keywords should be inside title-info"
        );
    }

    #[test]
    fn fb2_writer_omits_keywords_when_no_subjects() {
        let mut book = Book::new();
        book.metadata.title = Some("No Keywords Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            !xml.contains("<keywords>"),
            "keywords should not be present when subjects are empty"
        );
    }

    #[test]
    fn fb2_writer_keywords_html_escaping() {
        let mut book = Book::new();
        book.metadata.title = Some("Keywords Escape Test".into());
        book.metadata.subjects.push("Science & Fiction".into());
        book.metadata.subjects.push("Children's Books".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("Science &amp; Fiction"),
            "ampersand should be escaped in keywords, got:\n{}",
            xml
        );
    }

    // --- Fix 4: document-info/id and version ---

    #[test]
    fn fb2_writer_document_info_has_id_and_version() {
        let mut book = Book::new();
        book.metadata.title = Some("Doc ID Test".into());
        book.metadata.authors.push("Test Author".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Must have <id> element
        assert!(
            xml.contains("<id>"),
            "document-info should contain <id> element"
        );
        assert!(
            xml.contains("</id>"),
            "document-info should contain </id> element"
        );

        // Must have <version>1.0</version>
        assert!(
            xml.contains("<version>1.0</version>"),
            "document-info should contain <version>1.0</version>, got:\n{}",
            xml
        );

        // id and version should be inside document-info
        let di_start = xml.find("<document-info>").unwrap();
        let di_end = xml.find("</document-info>").unwrap();
        let id_pos = xml.find("<id>").unwrap();
        let ver_pos = xml.find("<version>").unwrap();
        assert!(
            id_pos > di_start && id_pos < di_end,
            "id should be inside document-info"
        );
        assert!(
            ver_pos > di_start && ver_pos < di_end,
            "version should be inside document-info"
        );
    }

    #[test]
    fn fb2_writer_document_id_is_deterministic() {
        let make_book = || {
            let mut book = Book::new();
            book.metadata.title = Some("Deterministic ID".into());
            book.metadata.authors.push("Author One".into());
            book.add_chapter(Chapter {
                title: Some("Ch1".into()),
                content: "<p>Text</p>".into(),
                id: Some("ch1".into()),
            });
            book
        };

        let mut out1 = Vec::new();
        Fb2Writer::new()
            .write_book(&make_book(), &mut out1)
            .unwrap();
        let xml1 = String::from_utf8(out1).unwrap();

        let mut out2 = Vec::new();
        Fb2Writer::new()
            .write_book(&make_book(), &mut out2)
            .unwrap();
        let xml2 = String::from_utf8(out2).unwrap();

        // Extract the <id> values
        let extract_id = |xml: &str| -> String {
            let start = xml.find("<id>").unwrap() + 4;
            let end = xml.find("</id>").unwrap();
            xml[start..end].to_string()
        };

        assert_eq!(
            extract_id(&xml1),
            extract_id(&xml2),
            "document ID should be deterministic for same metadata"
        );
    }

    #[test]
    fn fb2_writer_document_id_has_uuid_format() {
        let mut book = Book::new();
        book.metadata.title = Some("UUID Format Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let start = xml.find("<id>").unwrap() + 4;
        let end = xml.find("</id>").unwrap();
        let id = &xml[start..end];

        // UUID format: 8-4-4-4-12 hex characters
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.len(),
            5,
            "UUID should have 5 parts separated by hyphens, got: {}",
            id
        );
        assert_eq!(parts[0].len(), 8, "first part should be 8 chars");
        assert_eq!(parts[1].len(), 4, "second part should be 4 chars");
        assert_eq!(parts[2].len(), 4, "third part should be 4 chars");
        assert_eq!(parts[3].len(), 4, "fourth part should be 4 chars");
        assert_eq!(parts[4].len(), 12, "fifth part should be 12 chars");
        // All parts should be hex
        for part in &parts {
            assert!(
                part.chars().all(|c| c.is_ascii_hexdigit()),
                "UUID parts should be hex, got: {}",
                id
            );
        }
    }

    // --- Fix 5: br within paragraph treated as soft break ---

    #[test]
    fn fb2_writer_br_inside_paragraph_is_soft_break() {
        let mut book = Book::new();
        book.metadata.title = Some("BR Soft Break Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Line one<br/>Line two</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // br inside paragraph should NOT split into two paragraphs
        assert!(
            xml.contains("<p>Line one Line two</p>"),
            "br inside paragraph should become a space, keeping content in single <p>, got:\n{}",
            xml
        );
        assert!(
            !xml.contains("<empty-line/>"),
            "br inside paragraph should not produce empty-line, got:\n{}",
            xml
        );
    }

    #[test]
    fn fb2_writer_br_outside_paragraph_produces_empty_line() {
        let mut book = Book::new();
        book.metadata.title = Some("BR Outside Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Para 1</p><br/><p>Para 2</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        assert!(
            xml.contains("<empty-line/>"),
            "br between paragraphs should produce empty-line, got:\n{}",
            xml
        );
        assert!(xml.contains("<p>Para 1</p>"));
        assert!(xml.contains("<p>Para 2</p>"));
    }

    #[test]
    fn fb2_writer_br_reduces_paragraph_count() {
        // This test verifies the fix for the +19.4% paragraph inflation issue.
        // Multiple <br> tags within a single <p> should NOT produce extra paragraphs.
        let mut book = Book::new();
        book.metadata.title = Some("BR Inflation Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Line A<br/>Line B<br/>Line C</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Should be ONE content paragraph, not three
        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];
        // Count <p> tags in body (excluding the title <p>)
        let p_tags: Vec<_> = body.match_indices("<p>").collect();
        // One for the section title, one for content = 2 total
        assert_eq!(
            p_tags.len(),
            2,
            "should have 2 <p> tags in body (1 title + 1 content), not {}: body is:\n{}",
            p_tags.len(),
            body
        );
    }

    // =========================================================================
    // Tests for code-quality review findings
    // =========================================================================

    #[test]
    fn fb2_writer_head_content_suppressed() {
        // <head> content (e.g. <title>) should NOT leak into FB2 output.
        let mut book = Book::new();
        book.metadata.title = Some("Head Suppression Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content:
                "<html><head><title>Page Title</title></head><body><p>Content</p></body></html>"
                    .into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // The body content should be present
        assert!(
            xml.contains("<p>Content</p>"),
            "body content should be present, got:\n{}",
            xml
        );
        // The <title> text from <head> must NOT appear in any paragraph
        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];
        assert!(
            !body.contains("Page Title"),
            "<head><title> text should be suppressed from FB2 output, got:\n{}",
            body
        );
    }

    #[test]
    fn fb2_writer_pg_boilerplate_filtered() {
        // Project Gutenberg header/footer boilerplate divs should be suppressed.
        let mut book = Book::new();
        book.metadata.title = Some("PG Boilerplate Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content:
                r#"<div id="pg-header"><p>Project Gutenberg header</p></div><p>Real content</p>"#
                    .into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];

        assert!(
            !body.contains("Project Gutenberg header"),
            "PG boilerplate text should be suppressed, got:\n{}",
            body
        );
        assert!(
            body.contains("<p>Real content</p>"),
            "non-boilerplate content should be preserved, got:\n{}",
            body
        );
    }

    #[test]
    fn fb2_writer_div_block_content_accumulation() {
        // Multi-line text inside a single <div> should produce a single
        // paragraph, not one paragraph per line.
        let mut book = Book::new();
        book.metadata.title = Some("Div Block Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<div>Line one\nLine two\nLine three</div>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];

        // All three lines should appear in a single <p> element
        assert!(
            body.contains("Line one Line two Line three"),
            "div content should be accumulated into a single paragraph, got:\n{}",
            body
        );
        // Count content <p> tags (excluding section title)
        let content_p_count =
            body.matches("<p>").count() - if body.contains("<title><p>") { 1 } else { 0 };
        assert_eq!(
            content_p_count, 1,
            "should produce exactly 1 content paragraph from a single <div>, got {} in:\n{}",
            content_p_count, body
        );
    }

    #[test]
    fn fb2_writer_empty_chapter_skipped() {
        // A chapter whose content is only an <img> tag (no text) and has no
        // title should be skipped entirely.
        let mut book = Book::new();
        book.metadata.title = Some("Empty Chapter Test".into());
        // Chapter with only an image tag and no title
        book.add_chapter(Chapter {
            title: None,
            content: r#"<img src="cover.jpg"/>"#.into(),
            id: Some("cover-page".into()),
        });
        // A real chapter that should be present
        book.add_chapter(Chapter {
            title: Some("Real Chapter".into()),
            content: "<p>Real content here</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];

        // The real chapter must be present
        assert!(
            body.contains("Real content here"),
            "real chapter content should be present, got:\n{}",
            body
        );

        // Count <section> elements — should be exactly 1 (the real chapter)
        // (no cover image section since no cover resource was added)
        let section_count = body.matches("<section>").count();
        assert_eq!(
            section_count, 1,
            "empty/image-only chapter without title should be skipped, but got {} sections in:\n{}",
            section_count, body
        );
    }

    #[test]
    fn fb2_writer_document_info_author_population() {
        // Verify that <document-info> contains the same author elements as
        // <title-info>.
        let mut book = Book::new();
        book.metadata.title = Some("Doc Info Author Test".into());
        book.metadata.authors.push("Jane Doe".into());
        book.metadata.authors.push("Bob".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Extract the document-info block
        let di_start = xml
            .find("<document-info>")
            .expect("missing <document-info>");
        let di_end = xml
            .find("</document-info>")
            .expect("missing </document-info>");
        let di_block = &xml[di_start..di_end];

        // Jane Doe should be split into first-name/last-name
        assert!(
            di_block.contains("<first-name>Jane</first-name>"),
            "document-info should contain first-name Jane, got:\n{}",
            di_block
        );
        assert!(
            di_block.contains("<last-name>Doe</last-name>"),
            "document-info should contain last-name Doe, got:\n{}",
            di_block
        );
        // Single-name author "Bob" should appear as first-name only
        assert!(
            di_block.contains("<first-name>Bob</first-name>"),
            "document-info should contain first-name Bob, got:\n{}",
            di_block
        );
        // Should have 2 <author> elements in document-info
        let author_count = di_block.matches("<author>").count();
        assert_eq!(
            author_count, 2,
            "document-info should have 2 author elements, got {} in:\n{}",
            author_count, di_block
        );
    }

    // =========================================================================
    // Tests for Task 10: Hyperlink preservation, empty-line spacing, cover fix
    // =========================================================================

    #[test]
    fn fb2_writer_empty_line_between_multiple_sections() {
        // Multiple chapters should produce <empty-line/> between sections
        // for visual spacing (matching Calibre behavior).
        let mut book = Book::new();
        book.metadata.title = Some("Multi Section Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>First chapter content</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter 2".into()),
            content: "<p>Second chapter content</p>".into(),
            id: Some("ch2".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter 3".into()),
            content: "<p>Third chapter content</p>".into(),
            id: Some("ch3".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];

        // There should be empty-line elements between sections (2 for 3 sections)
        let empty_line_count = body.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 2,
            "expected 2 <empty-line/> elements between 3 sections, got {} in:\n{}",
            empty_line_count, body
        );

        // Each <empty-line/> should be INSIDE a <section> (valid FB2 XML)
        // Verify by checking they appear after a <section> opening
        for (idx, _) in body.match_indices("<empty-line/>") {
            let before = &body[..idx];
            let last_section_open = before.rfind("<section>").unwrap_or(0);
            let last_section_close = before.rfind("</section>").unwrap_or(0);
            assert!(
                last_section_open > last_section_close,
                "<empty-line/> at position {} should be inside a <section>, not between sections",
                idx
            );
        }
    }

    #[test]
    fn fb2_writer_no_empty_line_in_single_section() {
        // A single chapter should NOT produce any inter-section empty-lines.
        let mut book = Book::new();
        book.metadata.title = Some("Single Section Test".into());
        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Only chapter</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        let body_start = xml.find("<body>").unwrap();
        let body_end = xml.find("</body>").unwrap();
        let body = &xml[body_start..body_end];

        let empty_line_count = body.matches("<empty-line/>").count();
        assert_eq!(
            empty_line_count, 0,
            "single section should not have inter-section <empty-line/>, got {} in:\n{}",
            empty_line_count, body
        );
    }

    #[test]
    fn fb2_writer_single_cover_reference_with_cover_resource() {
        // Verify that when a cover image exists, there is exactly ONE <image>
        // reference to it (in <coverpage>), and NOT a duplicate in the body.
        let mut book = Book::new();
        book.metadata.title = Some("Single Cover Ref Test".into());
        book.metadata.cover_image_id = Some("cover-img".into());
        book.add_resource(
            "cover-img",
            "images/cover.jpg",
            vec![0xFF, 0xD8],
            "image/jpeg",
        );
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // Count ALL <image> references to the cover
        let cover_refs: Vec<_> = xml
            .match_indices("<image l:href=\"#cover-img\"/>")
            .collect();
        assert_eq!(
            cover_refs.len(),
            1,
            "should have exactly 1 cover <image> reference (in coverpage only), got {} in:\n{}",
            cover_refs.len(),
            xml
        );

        // The single reference should be inside <coverpage>
        assert!(
            xml.contains("<coverpage><image l:href=\"#cover-img\"/></coverpage>"),
            "cover image should be in <coverpage> element"
        );
    }

    #[test]
    fn fb2_writer_external_links_in_multi_chapter() {
        // Verify external links are preserved across multiple chapters
        let mut book = Book::new();
        book.metadata.title = Some("Multi Chapter Links".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: r#"<p>Visit <a href="https://www.gutenberg.org">Project Gutenberg</a></p>"#
                .into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Ch2".into()),
            content: r##"<p>Also <a href="http://example.com/page">this page</a> and <a href="#internal">internal</a></p>"##.into(),
            id: Some("ch2".into()),
        });

        let mut output = Vec::new();
        Fb2Writer::new().write_book(&book, &mut output).unwrap();
        let xml = String::from_utf8(output).unwrap();

        // External links should be preserved
        assert!(
            xml.contains(r#"<a l:href="https://www.gutenberg.org">Project Gutenberg</a>"#),
            "https link should be preserved, got:\n{}",
            xml
        );
        assert!(
            xml.contains(r#"<a l:href="http://example.com/page">this page</a>"#),
            "http link should be preserved, got:\n{}",
            xml
        );
        // Internal links should be stripped (text preserved)
        assert!(
            !xml.contains(r##"l:href="#internal""##),
            "internal fragment links should be stripped, got:\n{}",
            xml
        );
        assert!(
            xml.contains("internal"),
            "link text from internal links should be preserved"
        );
    }
}
