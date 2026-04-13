//! PML markup parser — converts between PML tags and HTML.
//!
//! PML uses backslash-prefixed tags: `\b` for bold, `\i` for italic,
//! `\x` for chapter headings, `\p` for page breaks, etc.

use crate::domain::Book;
use crate::formats::common::html_utils::strip_tags;
use crate::formats::common::text_utils;
use crate::formats::common::text_utils::push_escape_html;

/// Pre-computed heading open/close tags to avoid `format!` in the parsing loop.
const H_OPEN: [&str; 7] = ["", "<h1>", "<h2>", "<h3>", "<h4>", "<h5>", "<h6>"];
const H_CLOSE: [&str; 7] = [
    "", "</h1>\n", "</h2>\n", "</h3>\n", "</h4>\n", "</h5>\n", "</h6>\n",
];

/// Converts PML markup to HTML.
pub(crate) fn pml_to_html(pml: &str) -> String {
    let mut html = String::with_capacity(pml.len() * 2);
    let mut pos = 0;
    let bytes = pml.as_bytes();
    let len = bytes.len();

    // Toggle states for paired formatting tags.
    let mut bold = false;
    let mut italic = false;
    let mut underline = false;
    let mut strikethrough = false;
    let mut large = false;
    let mut small_caps = false;
    let mut superscript = false;
    let mut subscript = false;
    let mut in_paragraph = false;

    while pos < len {
        if bytes[pos] == b'\\' {
            pos += 1;
            if pos >= len {
                break;
            }

            match bytes[pos] {
                // Page break.
                b'p' => {
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    html.push_str("<!-- pagebreak -->\n");
                    pos += 1;
                },
                // Chapter title (h1 with page break).
                b'x' => {
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    pos += 1;
                    // Skip leading whitespace.
                    while pos < len && bytes[pos] == b' ' {
                        pos += 1;
                    }
                    let title_start = pos;
                    while pos < len && bytes[pos] != b'\n' && bytes[pos] != b'\r' {
                        pos += 1;
                    }
                    let title = &pml[title_start..pos];
                    html.push_str("<h1>");
                    push_escape_html(&mut html, title);
                    html.push_str("</h1>\n");
                },
                // Extended headings: \X0 through \X4 → h2 through h6.
                b'X' => {
                    pos += 1;
                    let level = if pos < len && bytes[pos].is_ascii_digit() {
                        let d = (bytes[pos] - b'0') as usize;
                        pos += 1;
                        (d + 2).min(6)
                    } else {
                        2
                    };
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    // Skip leading whitespace.
                    while pos < len && bytes[pos] == b' ' {
                        pos += 1;
                    }
                    let title_start = pos;
                    while pos < len && bytes[pos] != b'\n' && bytes[pos] != b'\r' {
                        pos += 1;
                    }
                    let title = &pml[title_start..pos];
                    html.push_str(H_OPEN[level]);
                    push_escape_html(&mut html, title);
                    html.push_str(H_CLOSE[level]);
                },
                // Bold toggle.
                b'b' | b'B' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if bold {
                        html.push_str("</b>");
                    } else {
                        html.push_str("<b>");
                    }
                    bold = !bold;
                    pos += 1;
                },
                // Italic toggle.
                b'i' | b'I' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if italic {
                        html.push_str("</i>");
                    } else {
                        html.push_str("<i>");
                    }
                    italic = !italic;
                    pos += 1;
                },
                // Underline toggle.
                b'u' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if underline {
                        html.push_str("</u>");
                    } else {
                        html.push_str("<u>");
                    }
                    underline = !underline;
                    pos += 1;
                },
                // Strikethrough toggle.
                b'o' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if strikethrough {
                        html.push_str("</s>");
                    } else {
                        html.push_str("<s>");
                    }
                    strikethrough = !strikethrough;
                    pos += 1;
                },
                // Large text toggle.
                b'l' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if large {
                        html.push_str("</span>");
                    } else {
                        html.push_str("<span style=\"font-size: 150%;\">");
                    }
                    large = !large;
                    pos += 1;
                },
                // Small caps toggle.
                b'k' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if small_caps {
                        html.push_str("</span>");
                    } else {
                        html.push_str("<span style=\"font-variant: small-caps;\">");
                    }
                    small_caps = !small_caps;
                    pos += 1;
                },
                // Superscript.
                b'S' if pos + 1 < len && bytes[pos + 1] == b'p' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if superscript {
                        html.push_str("</sup>");
                    } else {
                        html.push_str("<sup>");
                    }
                    superscript = !superscript;
                    pos += 2;
                },
                // Subscript.
                b'S' if pos + 1 < len && bytes[pos + 1] == b'b' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    if subscript {
                        html.push_str("</sub>");
                    } else {
                        html.push_str("<sub>");
                    }
                    subscript = !subscript;
                    pos += 2;
                },
                // Center alignment.
                b'c' => {
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    html.push_str("<div style=\"text-align: center;\">");
                    pos += 1;
                    let content_start = pos;
                    // Read until next \c or newline.
                    while pos < len && bytes[pos] != b'\n' {
                        if bytes[pos] == b'\\' && pos + 1 < len && bytes[pos + 1] == b'c' {
                            break;
                        }
                        pos += 1;
                    }
                    push_escape_html(&mut html, &pml[content_start..pos]);
                    html.push_str("</div>\n");
                    // Skip the closing \c if present.
                    if pos < len && bytes[pos] == b'\\' {
                        pos += 2;
                    }
                },
                // Right alignment.
                b'r' => {
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    html.push_str("<div style=\"text-align: right;\">");
                    pos += 1;
                    let content_start = pos;
                    while pos < len && bytes[pos] != b'\n' {
                        if bytes[pos] == b'\\' && pos + 1 < len && bytes[pos + 1] == b'r' {
                            break;
                        }
                        pos += 1;
                    }
                    push_escape_html(&mut html, &pml[content_start..pos]);
                    html.push_str("</div>\n");
                    if pos < len && bytes[pos] == b'\\' {
                        pos += 2;
                    }
                },
                // Indent.
                b't' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("&emsp;");
                    pos += 1;
                },
                // Newline / line break.
                b'n' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<br />");
                    pos += 1;
                },
                // Link: \q="#target"text\q
                b'q' => {
                    pos += 1;
                    let target = read_quoted_value(pml, &mut pos);
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<a href=\"");
                    push_escape_html(&mut html, &target);
                    html.push_str("\">");
                    // Read link text until closing \q.
                    let text_start = pos;
                    while pos < len {
                        if bytes[pos] == b'\\' && pos + 1 < len && bytes[pos + 1] == b'q' {
                            break;
                        }
                        pos += 1;
                    }
                    push_escape_html(&mut html, &pml[text_start..pos]);
                    html.push_str("</a>");
                    if pos < len && bytes[pos] == b'\\' {
                        pos += 2; // skip closing \q
                    }
                },
                // Link target: \Q="id"
                b'Q' => {
                    pos += 1;
                    let target_id = read_quoted_value(pml, &mut pos);
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<a id=\"");
                    push_escape_html(&mut html, &target_id);
                    html.push_str("\"></a>");
                },
                // Image: \m="filename.png"
                b'm' => {
                    pos += 1;
                    let filename = read_quoted_value(pml, &mut pos);
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<img src=\"");
                    push_escape_html(&mut html, &filename);
                    html.push_str("\" alt=\"\" />");
                },
                // Footnote reference: \Fn="id" or sidebar reference: \Sd="id"
                b'F' if pos + 1 < len && bytes[pos + 1] == b'n' => {
                    pos += 2;
                    let fn_id = read_quoted_value(pml, &mut pos);
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<a href=\"#fn-");
                    push_escape_html(&mut html, &fn_id);
                    html.push_str("\">[*]</a>");
                },
                b'S' if pos + 1 < len && bytes[pos + 1] == b'd' => {
                    pos += 2;
                    let sb_id = read_quoted_value(pml, &mut pos);
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push_str("<a href=\"#sb-");
                    push_escape_html(&mut html, &sb_id);
                    html.push_str("\">[*]</a>");
                },
                // Footnote content: \FN="id"...\FN
                b'F' if pos + 1 < len && bytes[pos + 1] == b'N' => {
                    pos += 2;
                    let fn_id = read_quoted_value(pml, &mut pos);
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    html.push_str("<div id=\"fn-");
                    push_escape_html(&mut html, &fn_id);
                    html.push_str("\"><p>");
                    // Content until closing \FN.
                    let content_start = pos;
                    loop {
                        match memchr::memchr(b'\\', &bytes[pos..len]) {
                            Some(offset) => {
                                let abs = pos + offset;
                                if abs + 2 < len && bytes[abs + 1] == b'F' && bytes[abs + 2] == b'N'
                                {
                                    pos = abs;
                                    break;
                                }
                                pos = abs + 1;
                            },
                            None => {
                                pos = len;
                                break;
                            },
                        }
                    }
                    push_escape_html(&mut html, &pml[content_start..pos]);
                    html.push_str("</p></div>\n");
                    if pos < len && bytes[pos] == b'\\' {
                        pos += 3; // skip \FN
                    }
                },
                // Sidebar content: \SB="id"...\SB
                b'S' if pos + 1 < len && bytes[pos + 1] == b'B' => {
                    pos += 2;
                    let sb_id = read_quoted_value(pml, &mut pos);
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    html.push_str("<div id=\"sb-");
                    push_escape_html(&mut html, &sb_id);
                    html.push_str("\"><p>");
                    let content_start = pos;
                    loop {
                        match memchr::memchr(b'\\', &bytes[pos..len]) {
                            Some(offset) => {
                                let abs = pos + offset;
                                if abs + 2 < len && bytes[abs + 1] == b'S' && bytes[abs + 2] == b'B'
                                {
                                    pos = abs;
                                    break;
                                }
                                pos = abs + 1;
                            },
                            None => {
                                pos = len;
                                break;
                            },
                        }
                    }
                    push_escape_html(&mut html, &pml[content_start..pos]);
                    html.push_str("</p></div>\n");
                    if pos < len && bytes[pos] == b'\\' {
                        pos += 3; // skip \SB
                    }
                },
                // Horizontal rule.
                b'w' => {
                    if in_paragraph {
                        html.push_str("</p>\n");
                        in_paragraph = false;
                    }
                    let width = read_quoted_value(pml, &mut pos);
                    html.push_str("<hr style=\"width: ");
                    if width.is_empty() {
                        html.push_str("100%");
                    } else {
                        html.push_str(&width);
                        html.push('%');
                    }
                    html.push_str(";\" />\n");
                    pos += 1;
                },
                // Em dash.
                b'-' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push('\u{2014}');
                    pos += 1;
                },
                // Escaped backslash.
                b'\\' => {
                    ensure_paragraph(&mut html, &mut in_paragraph);
                    html.push('\\');
                    pos += 1;
                },
                // Accented character: \aXXX (3-digit code).
                b'a' => {
                    pos += 1;
                    let code_start = pos;
                    while pos < len && pos < code_start + 3 && bytes[pos].is_ascii_digit() {
                        pos += 1;
                    }
                    let code_str = &pml[code_start..pos];
                    if let Ok(code) = code_str.parse::<u32>()
                        && let Some(ch) = char::from_u32(code)
                    {
                        ensure_paragraph(&mut html, &mut in_paragraph);
                        html.push(ch);
                    }
                },
                // Unicode character: \UXXXX (4-digit hex).
                b'U' => {
                    pos += 1;
                    let code_start = pos;
                    while pos < len && pos < code_start + 4 && bytes[pos].is_ascii_hexdigit() {
                        pos += 1;
                    }
                    let hex_str = &pml[code_start..pos];
                    if let Ok(code) = u32::from_str_radix(hex_str, 16)
                        && let Some(ch) = char::from_u32(code)
                    {
                        ensure_paragraph(&mut html, &mut in_paragraph);
                        html.push(ch);
                    }
                },
                // Unknown escape — skip one full UTF-8 character.
                _ => {
                    let ch_len = pml[pos..].chars().next().map_or(1, |c| c.len_utf8());
                    pos += ch_len;
                },
            }
        } else if bytes[pos] == b'\n' {
            // Newline in PML starts a new paragraph.
            if in_paragraph {
                html.push_str("</p>\n");
                in_paragraph = false;
            }
            pos += 1;
            // Skip \r\n pairs.
            if pos < len && bytes[pos] == b'\r' {
                pos += 1;
            }
        } else if bytes[pos] == b'\r' {
            if in_paragraph {
                html.push_str("</p>\n");
                in_paragraph = false;
            }
            pos += 1;
            if pos < len && bytes[pos] == b'\n' {
                pos += 1;
            }
        } else {
            // Regular text.
            ensure_paragraph(&mut html, &mut in_paragraph);
            let start = pos;
            let remaining = &bytes[pos..len];
            let skip = memchr::memchr3(b'\\', b'\n', b'\r', remaining).unwrap_or(remaining.len());
            pos += skip;
            push_escape_html(&mut html, &pml[start..pos]);
        }
    }

    // Close any open paragraph.
    if in_paragraph {
        html.push_str("</p>\n");
    }

    html
}

/// Splits HTML (converted from PML) into chapters at h1/pagebreak boundaries.
///
/// Uses byte-level scanning with `memchr` instead of character-by-character
/// iteration to avoid O(n) per-char push overhead.
pub(crate) fn split_pml_chapters(html: &str) -> Vec<(Option<String>, String)> {
    let mut chapters = Vec::new();

    let bytes = html.as_bytes();
    let pb_needle = b"<!-- pagebreak -->";

    // Pre-lowercase the input once for case-insensitive tag matching.
    let lowered = text_utils::ascii_lowercase_copy(bytes);

    // Find all split points: h1 opens and pagebreak markers.
    #[derive(Debug)]
    enum SplitKind {
        H1 { title_end: usize },
        Pagebreak,
    }

    let mut splits: Vec<(usize, SplitKind)> = Vec::new();

    // Find all <h1 tags.
    let mut search_from = 0;
    while let Some(rel) = memchr::memmem::find(&lowered[search_from..], b"<h1") {
        let abs = search_from + rel;
        // Verify it's a tag (next byte is > or space).
        if abs + 3 < bytes.len() && (bytes[abs + 3] == b'>' || bytes[abs + 3] == b' ') {
            // Find closing </h1>.
            let tag_end = memchr::memchr(b'>', &bytes[abs..])
                .map(|e| abs + e + 1)
                .unwrap_or(abs);
            let close_pos =
                memchr::memmem::find(&lowered[tag_end..], b"</h1>").map(|e| tag_end + e);
            let title_end = close_pos.map(|c| c + 5).unwrap_or(tag_end);
            // Extract title text.
            if let Some(close) = close_pos {
                splits.push((abs, SplitKind::H1 { title_end }));
                // Stash title info — we'll extract it during chapter assembly.
                let _ = close; // used via title_end
            } else {
                splits.push((abs, SplitKind::H1 { title_end }));
            }
        }
        search_from = abs + 3;
    }

    // Find all pagebreak markers.
    search_from = 0;
    while let Some(rel) = memchr::memmem::find(&bytes[search_from..], pb_needle) {
        let abs = search_from + rel;
        splits.push((abs, SplitKind::Pagebreak));
        search_from = abs + pb_needle.len();
    }

    if splits.is_empty() {
        return Vec::new();
    }

    // Sort by position.
    splits.sort_by_key(|(pos, _)| *pos);

    // Assemble chapters.
    let mut current_title: Option<String> = None;
    let mut last_end = 0;

    for (pos, kind) in &splits {
        let content_before = html[last_end..*pos].trim();
        if !content_before.is_empty() {
            chapters.push((current_title.take(), content_before.to_string()));
        }

        match kind {
            SplitKind::H1 { title_end } => {
                // Extract title from <h1>...</h1>.
                let tag_end = memchr::memchr(b'>', &bytes[*pos..])
                    .map(|e| *pos + e + 1)
                    .unwrap_or(*pos);
                let close = title_end.saturating_sub(5);
                if close > tag_end {
                    let raw_title = &html[tag_end..close];
                    current_title = Some(strip_tags(raw_title).trim().to_string());
                }
                last_end = *title_end;
            },
            SplitKind::Pagebreak => {
                last_end = pos + pb_needle.len();
            },
        }
    }

    // Remaining content after last split.
    let trailing = html[last_end..].trim();
    if !trailing.is_empty() {
        chapters.push((current_title.take(), trailing.to_string()));
    }

    chapters
}

/// Converts a `Book` to PML markup.
pub(crate) fn book_to_pml(book: &Book) -> String {
    let mut pml = String::with_capacity(4096);

    for (i, chapter) in book.chapter_views().iter().enumerate() {
        if i > 0 {
            pml.push_str("\\p\n");
        }

        if let Some(title) = chapter.title {
            pml.push_str("\\x ");
            pml.push_str(title);
            pml.push('\n');
        }

        html_to_pml(chapter.content, &mut pml);
        pml.push('\n');
    }

    pml
}

/// Converts HTML content to PML markup.
fn html_to_pml(html: &str, pml: &mut String) {
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    while pos < len {
        if bytes[pos] == b'<' {
            let tag_end = match html[pos..].find('>') {
                Some(e) => pos + e + 1,
                None => break,
            };
            let tag = &html[pos..tag_end];
            let tag_eq = |s: &str| tag.eq_ignore_ascii_case(s);
            let tag_starts = |prefix: &str| {
                tag.as_bytes()
                    .get(..prefix.len())
                    .is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
            };

            if tag_eq("<p>") || tag_starts("<p ") {
                // Paragraph — start a new line.
            } else if tag_eq("</p>") {
                pml.push('\n');
            } else if tag_eq("<br>") || tag_eq("<br/>") || tag_eq("<br />") {
                pml.push_str("\\n");
            } else if tag_eq("<b>") || tag_eq("</b>") || tag_eq("<strong>") || tag_eq("</strong>") {
                pml.push_str("\\b");
            } else if tag_eq("<i>") || tag_eq("</i>") || tag_eq("<em>") || tag_eq("</em>") {
                pml.push_str("\\i");
            } else if tag_eq("<u>") || tag_eq("</u>") {
                pml.push_str("\\u");
            } else if tag_eq("<s>")
                || tag_eq("</s>")
                || tag_eq("<strike>")
                || tag_eq("</strike>")
                || tag_eq("<del>")
                || tag_eq("</del>")
            {
                pml.push_str("\\o");
            } else if tag_eq("<sup>") || tag_eq("</sup>") {
                pml.push_str("\\Sp");
            } else if tag_eq("<sub>") || tag_eq("</sub>") {
                pml.push_str("\\Sb");
            } else if tag_starts("<h1") {
                pml.push_str("\\x ");
            } else if tag_eq("</h1>") {
                pml.push('\n');
            } else if tag_starts("<h2") {
                pml.push_str("\\X0 ");
            } else if tag_eq("</h2>") {
                pml.push('\n');
            } else if tag_starts("<h3") {
                pml.push_str("\\X1 ");
            } else if tag_eq("</h3>") {
                pml.push('\n');
            } else if tag_starts("<h4") {
                pml.push_str("\\X2 ");
            } else if tag_eq("</h4>") {
                pml.push('\n');
            } else if tag_starts("<h5") {
                pml.push_str("\\X3 ");
            } else if tag_eq("</h5>") {
                pml.push('\n');
            } else if tag_starts("<h6") {
                pml.push_str("\\X4 ");
            } else if tag_eq("</h6>") {
                pml.push('\n');
            } else if tag_eq("<hr>") || tag_eq("<hr/>") || tag_eq("<hr />") {
                pml.push_str("\\w\n");
            }
            // Other tags silently skipped.

            pos = tag_end;
        } else if bytes[pos] == b'&' {
            // Decode HTML entity.
            let (ch, consumed) = decode_entity(html, pos);
            pml_escape_char(pml, ch);
            pos += consumed;
        } else if let Some(ch) = html[pos..].chars().next() {
            pml_escape_char(pml, ch);
            pos += ch.len_utf8();
        } else {
            break;
        }
    }
}

/// Escapes a character for PML output.
fn pml_escape_char(pml: &mut String, ch: char) {
    match ch {
        '\\' => pml.push_str("\\\\"),
        c if (c as u32) > 127 => {
            // Write directly into the String, avoiding a format!() allocation.
            let _ = std::fmt::Write::write_fmt(pml, format_args!("\\U{:04X}", c as u32));
        },
        c => pml.push(c),
    }
}

/// Decodes an HTML entity at position `pos`. Returns (char, bytes_consumed).
fn decode_entity(html: &str, pos: usize) -> (char, usize) {
    let rest = &html[pos..];

    let entities = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&nbsp;", '\u{00A0}'),
        ("&mdash;", '\u{2014}'),
        ("&ndash;", '\u{2013}'),
    ];

    for (entity, ch) in &entities {
        if rest.starts_with(entity) {
            return (*ch, entity.len());
        }
    }

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

    ('&', 1)
}

/// Reads a quoted value like `="something"` at the current position.
/// Advances `pos` past the closing quote.
fn read_quoted_value(pml: &str, pos: &mut usize) -> String {
    let bytes = pml.as_bytes();
    let len = bytes.len();

    // Skip = and opening quote.
    if *pos < len && bytes[*pos] == b'=' {
        *pos += 1;
    }
    if *pos < len && bytes[*pos] == b'"' {
        *pos += 1;
    }

    let start = *pos;
    while *pos < len && bytes[*pos] != b'"' {
        *pos += 1;
    }
    let value = pml[start..*pos].to_string();

    // Skip closing quote.
    if *pos < len && bytes[*pos] == b'"' {
        *pos += 1;
    }

    value
}

/// Ensures we're inside a `<p>` element.
fn ensure_paragraph(html: &mut String, in_paragraph: &mut bool) {
    if !*in_paragraph {
        html.push_str("<p>");
        *in_paragraph = true;
    }
}

/// Escapes text for HTML output.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_toggle() {
        let result = pml_to_html("\\bBold text\\b");
        assert!(result.contains("<b>Bold text</b>"));
    }

    #[test]
    fn italic_toggle() {
        let result = pml_to_html("\\iItalic\\i");
        assert!(result.contains("<i>Italic</i>"));
    }

    #[test]
    fn heading_conversion() {
        let result = pml_to_html("\\x Chapter One");
        assert!(result.contains("<h1>Chapter One</h1>"));
    }

    #[test]
    fn extended_heading() {
        let result = pml_to_html("\\X0 Sub Heading");
        assert!(result.contains("<h2>Sub Heading</h2>"));
    }

    #[test]
    fn page_break() {
        let result = pml_to_html("Before\\pAfter");
        assert!(result.contains("<!-- pagebreak -->"));
    }

    #[test]
    fn escaped_backslash() {
        let result = pml_to_html("path\\\\to\\\\file");
        assert!(result.contains("path\\to\\file"));
    }

    #[test]
    fn em_dash() {
        let result = pml_to_html("word\\-word");
        assert!(result.contains('\u{2014}'));
    }

    #[test]
    fn unicode_escape() {
        let result = pml_to_html("\\U2014");
        assert!(result.contains('\u{2014}'));
    }

    #[test]
    fn link_markup() {
        let result = pml_to_html("\\q=\"#ch1\"Click here\\q");
        assert!(result.contains("<a href=\"#ch1\">Click here</a>"));
    }

    #[test]
    fn image_markup() {
        let result = pml_to_html("\\m=\"cover.png\"");
        assert!(result.contains("<img src=\"cover.png\""));
    }

    #[test]
    fn html_to_pml_basic() {
        let mut pml = String::new();
        html_to_pml("<p><b>Bold</b> and <i>italic</i></p>", &mut pml);
        assert!(pml.contains("\\b"));
        assert!(pml.contains("\\i"));
        assert!(pml.contains("Bold"));
    }

    #[test]
    fn html_to_pml_heading() {
        let mut pml = String::new();
        html_to_pml("<h1>Title</h1>", &mut pml);
        assert!(pml.contains("\\x "));
        assert!(pml.contains("Title"));
    }

    #[test]
    fn split_chapters_on_h1() {
        let html = "<h1>Chapter 1</h1><p>Content 1</p><h1>Chapter 2</h1><p>Content 2</p>";
        let chapters = split_pml_chapters(html);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].0.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[1].0.as_deref(), Some("Chapter 2"));
    }

    #[test]
    fn split_chapters_case_insensitive() {
        let html = "<H1>Chapter 1</H1><p>Content 1</p><H1>Chapter 2</H1><p>Content 2</p>";
        let chapters = split_pml_chapters(html);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].0.as_deref(), Some("Chapter 1"));
        assert_eq!(chapters[1].0.as_deref(), Some("Chapter 2"));
    }

    #[test]
    fn pml_escape_char_backslash() {
        let mut out = String::new();
        pml_escape_char(&mut out, '\\');
        assert_eq!(out, "\\\\");
    }

    #[test]
    fn pml_escape_char_unicode() {
        let mut out = String::new();
        pml_escape_char(&mut out, '\u{2014}');
        assert_eq!(out, "\\U2014");
    }
}
