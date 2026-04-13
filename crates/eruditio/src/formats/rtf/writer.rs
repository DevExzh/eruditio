//! RTF writer — generates RTF documents from `Book`.

use ahash::AHashMap as HashMap;

use crate::domain::Book;
use crate::formats::common::html_utils::{strip_leading_heading, strip_tags};

/// Converts a `Book` to an RTF document string.
pub fn book_to_rtf(book: &Book) -> String {
    let mut rtf = String::with_capacity(8192);

    // RTF header.
    rtf.push_str("{\\rtf1\\ansi\\widowctrl\\deff0\n");

    // Font table.
    rtf.push_str("{\\fonttbl{\\f0\\froman Times New Roman;}}\n");

    // Color table.
    rtf.push_str("{\\colortbl;\\red0\\green0\\blue0;}\n");

    // Stylesheet.
    rtf.push_str("{\\stylesheet\n");
    rtf.push_str("{\\s0\\ql\\sa120\\fi360\\f0\\fs24 Normal;}\n");
    rtf.push_str("{\\s1\\qc\\sb360\\sa120\\keepn\\outlinelevel0\\f0\\fs48\\b Heading 1;}\n");
    rtf.push_str("{\\s2\\qc\\sb300\\sa120\\keepn\\outlinelevel1\\f0\\fs36\\b Heading 2;}\n");
    rtf.push_str("{\\s3\\qc\\sb240\\sa120\\keepn\\outlinelevel2\\f0\\fs32\\b Heading 3;}\n");
    rtf.push_str("{\\s4\\qc\\sb200\\sa120\\keepn\\outlinelevel3\\f0\\fs28\\b Heading 4;}\n");
    rtf.push_str("{\\s5\\qc\\sb160\\sa120\\keepn\\outlinelevel4\\f0\\fs24\\b\\i Heading 5;}\n");
    rtf.push_str("{\\s6\\qc\\sb120\\sa120\\keepn\\outlinelevel5\\f0\\fs24\\i Heading 6;}\n");
    rtf.push_str("}\n");

    // Info group (metadata).
    write_info_group(book, &mut rtf);

    // Cover image (embedded as RTF picture group before chapter content).
    if let Some(cover_item) = find_cover_image(book)
        && let Some(image_data) = cover_item.data.as_bytes()
    {
        write_cover_image(&mut rtf, image_data, &cover_item.media_type);
    }

    // Default font and size.
    rtf.push_str("\\f0\\fs24\n");

    // Build a class-to-alignment map from CSS resources in the book.
    let align_map = build_class_alignment_map(book);

    // Content.
    let chapters = book.chapter_views();
    for (i, chapter) in chapters.iter().enumerate() {
        if i > 0 {
            // Page break between chapters.
            rtf.push_str("\\page\n");
        }

        // Chapter title as Heading 1 style (centered, with spacing).
        if let Some(title) = chapter.title {
            rtf.push_str("{\\pard\\s1\\qc\\sb360\\sa120\\keepn\\f0\\fs48\\b ");
            write_rtf_text(&mut rtf, title);
            rtf.push_str("\\par}\n");
        }

        // Strip duplicate heading from content before converting.
        let content = match chapter.title {
            Some(title) => strip_leading_heading(chapter.content, title),
            None => chapter.content,
        };

        // Convert HTML content to RTF.
        // First paragraph after a chapter title (or page break) suppresses indent.
        html_to_rtf(content, &mut rtf, true, &align_map);
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

/// Finds the cover image manifest item from the book.
///
/// Searches in order of priority:
/// 1. Item whose ID matches `book.metadata.cover_image_id`
/// 2. Item with the EPUB3 `cover-image` property
/// 3. Item with "cover" in its ID or href and an image media type
fn find_cover_image(book: &Book) -> Option<&crate::domain::manifest::ManifestItem> {
    // 1. Explicit cover image ID from metadata.
    if let Some(ref id) = book.metadata.cover_image_id
        && let Some(item) = book.manifest.get(id)
        && item.media_type.starts_with("image/")
    {
        return Some(item);
    }

    // 2. EPUB3 cover-image property.
    let by_property = book
        .manifest
        .iter()
        .find(|item| item.has_property("cover-image") && item.media_type.starts_with("image/"));
    if by_property.is_some() {
        return by_property;
    }

    // 3. Heuristic: "cover" in ID or href (case-insensitive).
    book.manifest.iter().find(|item| {
        item.media_type.starts_with("image/")
            && (item.id.to_ascii_lowercase().contains("cover")
                || item.href.to_ascii_lowercase().contains("cover"))
    })
}

/// Writes an RTF `\pict` group for the cover image followed by a page break.
fn write_cover_image(rtf: &mut String, image_data: &[u8], media_type: &str) {
    let blip_tag = match media_type {
        "image/png" => "\\pngblip",
        "image/jpeg" | "image/jpg" => "\\jpegblip",
        _ => return, // Unsupported image format; skip silently.
    };

    let (width_px, height_px) = if media_type == "image/png" {
        parse_png_dimensions(image_data)
    } else {
        parse_jpeg_dimensions(image_data)
    }
    .unwrap_or((600, 800));

    // \picwgoal / \pichgoal: desired display size in twips (pixels * 1440 / 96).
    let width_twips = (width_px as u32) * 1440 / 96;
    let height_twips = (height_px as u32) * 1440 / 96;

    use std::fmt::Write;
    let _ = writeln!(
        rtf,
        "{{\\pict{blip_tag}\\picwgoal{width_twips}\\pichgoal{height_twips}"
    );

    // Hex-encode image data with line breaks every 80 hex characters (40 bytes).
    // Use a lookup table for performance instead of per-byte write!().
    const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";
    rtf.reserve(image_data.len() * 2 + image_data.len() / 40 + 64);
    for (i, &byte) in image_data.iter().enumerate() {
        rtf.push(HEX_CHARS[(byte >> 4) as usize] as char);
        rtf.push(HEX_CHARS[(byte & 0x0F) as usize] as char);
        if (i + 1) % 40 == 0 {
            rtf.push('\n');
        }
    }

    rtf.push_str("}\n\\par\\page\n");
}

/// Parses JPEG dimensions from the SOF0/SOF2 marker.
///
/// Walks marker-to-marker (skipping segment payloads) to avoid false-positive
/// matches inside APP segment data (e.g. EXIF, ICC profiles).
fn parse_jpeg_dimensions(data: &[u8]) -> Option<(u16, u16)> {
    let len = data.len();
    if len < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return None; // Not a JPEG (missing SOI marker).
    }
    let mut i = 2;
    while i + 1 < len {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        // Skip fill bytes (consecutive 0xFF).
        while i + 1 < len && data[i + 1] == 0xFF {
            i += 1;
        }
        if i + 1 >= len {
            break;
        }
        let marker = data[i + 1];
        i += 2;
        // SOF0 (baseline) or SOF2 (progressive).
        if marker == 0xC0 || marker == 0xC2 {
            if i + 7 <= len {
                let height = u16::from_be_bytes([data[i + 3], data[i + 4]]);
                let width = u16::from_be_bytes([data[i + 5], data[i + 6]]);
                if width > 0 && height > 0 {
                    return Some((width, height));
                }
            }
            return None;
        }
        // Markers without payloads: RST0-RST7 (0xD0-0xD7), SOI (0xD8), EOI (0xD9), TEM (0x01).
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            continue;
        }
        // All other markers have a 2-byte length field; skip the segment payload.
        if i + 1 < len {
            let seg_len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += seg_len; // Length includes its own 2 bytes.
        } else {
            break;
        }
    }
    None
}

/// Parses PNG dimensions from the IHDR chunk.
///
/// PNG files start with an 8-byte signature, then the first chunk is IHDR
/// which contains width (4 bytes BE) and height (4 bytes BE) at offsets 16 and 20.
fn parse_png_dimensions(data: &[u8]) -> Option<(u16, u16)> {
    // PNG signature (8 bytes) + IHDR chunk length (4 bytes) + "IHDR" (4 bytes)
    // + width (4) + height (4) = 24 bytes minimum
    if data.len() < 24 {
        return None;
    }
    // Verify PNG signature.
    if &data[0..4] != b"\x89PNG" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    if width == 0 || height == 0 {
        return None;
    }
    // Clamp to u16; images larger than 65535px are unlikely for ebook covers.
    Some((width.min(65535) as u16, height.min(65535) as u16))
}

/// Builds a map from CSS class name to `text-align` value by parsing CSS
/// stylesheets found in the book's manifest.
///
/// Only recognises simple selectors of the form `p.classname`, `.classname`,
/// or `element.classname` blocks that contain `text-align: left|center|right`.
fn build_class_alignment_map(book: &Book) -> HashMap<String, &'static str> {
    let mut map = HashMap::new();

    for resource in book.resources() {
        if resource.media_type != "text/css" {
            continue;
        }
        let Ok(css) = std::str::from_utf8(resource.data) else {
            continue;
        };
        parse_css_alignment_rules(css, &mut map);
    }

    map
}

/// Parses CSS text to extract class-name to text-align mappings.
///
/// Handles selectors like `p.right`, `.center`, `div.myclass` where the
/// declaration block contains `text-align: left|center|right`.
fn parse_css_alignment_rules(css: &str, map: &mut HashMap<String, &'static str>) {
    let len = css.len();
    let mut pos = 0;

    while pos < len {
        // Find the next '{'.
        let brace_start = match css[pos..].find('{') {
            Some(offset) => pos + offset,
            None => break,
        };

        // Find the matching '}'.
        let brace_end = match css[brace_start..].find('}') {
            Some(offset) => brace_start + offset,
            None => break,
        };

        // Extract selector and declaration block (from the original, un-lowered CSS).
        let selector = css[pos..brace_start].trim();
        let declarations = &css[brace_start + 1..brace_end];

        // Check if declarations contain text-align (case-insensitive).
        if let Some(ta_pos) = find_ascii_case_insensitive(declarations, "text-align") {
            let after_ta = &declarations[ta_pos + 10..];
            let after_colon = after_ta
                .trim_start()
                .strip_prefix(':')
                .unwrap_or(after_ta)
                .trim_start();

            let align_value = if after_colon.len() >= 5
                && after_colon[..5].eq_ignore_ascii_case("right")
            {
                Some("right")
            } else if after_colon.len() >= 6 && after_colon[..6].eq_ignore_ascii_case("center") {
                Some("center")
            } else if after_colon.len() >= 4 && after_colon[..4].eq_ignore_ascii_case("left") {
                Some("left")
            } else {
                None
            };

            if let Some(align) = align_value {
                // Extract class name from selector. Support:
                //   .classname
                //   p.classname
                //   element.classname
                // Split selector on commas for grouped selectors.
                for sel in selector.split(',') {
                    let sel = sel.trim();
                    if let Some(dot_pos) = sel.find('.') {
                        // Extract class name after the dot (stop at space, colon, etc.)
                        let class_part = &sel[dot_pos + 1..];
                        let class_name: String = class_part
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                            .map(|c| c.to_ascii_lowercase())
                            .collect();
                        if !class_name.is_empty() {
                            map.insert(class_name, align);
                        }
                    }
                }
            }
        }

        pos = brace_end + 1;
    }
}

/// Finds a needle (assumed all-ASCII-lowercase) in a haystack using
/// case-insensitive ASCII comparison, returning the byte offset of the first
/// match.  Avoids allocating a lowered copy of the haystack.
fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    let limit = h.len() - n.len();
    for i in 0..=limit {
        if h[i..i + n.len()].eq_ignore_ascii_case(n) {
            return Some(i);
        }
    }
    None
}

/// Converts simple HTML content to RTF control words.
///
/// Handles: `<p>`, `<br>`, `<b>`, `<i>`, `<em>`, `<strong>`, `<h1>`-`<h6>`.
/// Strips all other tags and converts entities.
///
/// `after_break` suppresses first-line indent on the first paragraph (used after
/// chapter titles and page breaks).
///
/// `align_map` maps CSS class names to their `text-align` values (e.g.
/// `"right"` -> `"right"`) so that `<p class="right">` emits `\qr`.
fn html_to_rtf(
    html: &str,
    rtf: &mut String,
    after_break: bool,
    align_map: &HashMap<String, &'static str>,
) {
    let mut pos = 0;
    let bytes = html.as_bytes();
    let len = bytes.len();

    // Track whether the next paragraph should suppress first-line indent.
    // True after a heading close or when entering after a chapter title / page break.
    let mut suppress_indent = after_break;

    // When true, skip leading whitespace (especially newlines) at the start of
    // paragraph content.  Without this, a newline between `<p ...>` and the
    // actual text is converted to `\par`, which closes the aligned paragraph
    // before any visible text appears (breaking `\qr` / `\qc` alignment).
    let mut skip_leading_ws = false;

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
                // Start of paragraph — emit paragraph formatting.
                // Check for text-align in the tag's style attribute or class.
                let tag_str = &html[pos..tag_end];
                let align = alignment_from_tag(tag_str, align_map);
                if suppress_indent {
                    rtf.push_str("\\pard\\s0");
                    rtf.push_str(align);
                    rtf.push_str("\\sb0\\sa120\\f0\\fs24 ");
                    suppress_indent = false;
                } else {
                    rtf.push_str("\\pard\\s0");
                    rtf.push_str(align);
                    rtf.push_str("\\sa120\\fi360\\f0\\fs24 ");
                }
                skip_leading_ws = true;
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
                && matches!(tag_bytes[2], b'1'..=b'6')
            {
                // Heading — centered, with level-scaled spacing.
                // Wrap in a group `{...}` to scope bold/italic character formatting.
                let level = tag_bytes[2] - b'0';
                let style_ref = match level {
                    1 => "{\\pard\\s1\\qc\\sb360\\sa120\\keepn\\f0\\fs48\\b ",
                    2 => "{\\pard\\s2\\qc\\sb300\\sa120\\keepn\\f0\\fs36\\b ",
                    3 => "{\\pard\\s3\\qc\\sb240\\sa120\\keepn\\f0\\fs32\\b ",
                    4 => "{\\pard\\s4\\qc\\sb200\\sa120\\keepn\\f0\\fs28\\b ",
                    5 => "{\\pard\\s5\\qc\\sb160\\sa120\\keepn\\f0\\fs24\\b\\i ",
                    6 => "{\\pard\\s6\\qc\\sb120\\sa120\\keepn\\f0\\fs24\\i ",
                    _ => "{\\pard\\s1\\qc\\sb360\\sa120\\keepn\\f0\\fs48\\b ",
                };
                rtf.push_str(style_ref);
            } else if tag_bytes.len() >= 5
                && tag_bytes[0] == b'<'
                && tag_bytes[1] == b'/'
                && tag_bytes[2].eq_ignore_ascii_case(&b'h')
                && matches!(tag_bytes.get(3), Some(b'1'..=b'6'))
            {
                // End heading paragraph, close group, and suppress indent on next paragraph.
                rtf.push_str("\\par}\n");
                suppress_indent = true;
            }
            // Other tags are silently skipped.

            pos = tag_end;
        } else if bytes[pos] == b'&' {
            // HTML entity — never whitespace, so stop skipping.
            skip_leading_ws = false;
            // HTML entity.
            let (ch, consumed) = decode_html_entity(html, pos);
            write_rtf_char(rtf, ch);
            pos += consumed;
        } else {
            // Regular text — batch-copy runs of safe ASCII to avoid
            // per-character write_rtf_char() overhead.  For English text,
            // the vast majority of bytes are safe ASCII (a-z, A-Z, 0-9,
            // space, punctuation) that need no RTF escaping.
            if skip_leading_ws {
                while pos < len {
                    let b = bytes[pos];
                    if b == b'<' || b == b'&' || !b.is_ascii_whitespace() {
                        break;
                    }
                    pos += 1;
                }
                if pos >= len {
                    break;
                }
                if bytes[pos] == b'<' || bytes[pos] == b'&' {
                    continue;
                }
                skip_leading_ws = false;
            }

            // Scan for the end of a safe ASCII run.  "Safe" means: not a
            // tag start (<), entity start (&), RTF special (\, {, }),
            // newline, or non-ASCII byte (>= 128).
            let start = pos;
            while pos < len {
                let b = bytes[pos];
                if b == b'<'
                    || b == b'&'
                    || b == b'\\'
                    || b == b'{'
                    || b == b'}'
                    || b == b'\n'
                    || b >= 128
                {
                    break;
                }
                pos += 1;
            }

            if pos > start {
                // Bulk copy the safe ASCII run — no per-character dispatch.
                rtf.push_str(&html[start..pos]);
            }

            // If we stopped at a character that is NOT a tag/entity opener,
            // handle the single special character before the next loop iter.
            if pos < len {
                let b = bytes[pos];
                if b == b'<' || b == b'&' {
                    // Let the next loop iteration handle tags/entities.
                } else if b == b'\\' {
                    rtf.push_str("\\\\");
                    pos += 1;
                } else if b == b'{' {
                    rtf.push_str("\\{");
                    pos += 1;
                } else if b == b'}' {
                    rtf.push_str("\\}");
                    pos += 1;
                } else if b == b'\n' {
                    rtf.push_str("\\par\n");
                    pos += 1;
                } else {
                    // Non-ASCII UTF-8 character — decode and emit \uNNNN?.
                    let ch = html[pos..].chars().next().unwrap();
                    rtf.push_str("\\u");
                    push_i32(rtf, ch as i32);
                    rtf.push('?');
                    pos += ch.len_utf8();
                }
            }
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

/// Extracts the `text-align` value from an HTML tag's `style` attribute or
/// `class` attribute (resolved via the CSS alignment map).
///
/// Given the full tag string (e.g., `<p style="text-align: right">` or
/// `<p class="right">`), returns the RTF alignment command (`\qr`, `\qc`,
/// or `\ql`).
fn alignment_from_tag(tag_str: &str, align_map: &HashMap<String, &'static str>) -> &'static str {
    // Stack-based lowercasing avoids heap allocation for most tags.
    let tag_bytes = tag_str.as_bytes();
    let mut lower_buf = [0u8; 512];
    let tag_lower: std::borrow::Cow<'_, str> = if tag_bytes.len() <= 512 {
        for i in 0..tag_bytes.len() {
            lower_buf[i] = tag_bytes[i].to_ascii_lowercase();
        }
        // SAFETY: ASCII lowercasing preserves UTF-8 validity.
        std::borrow::Cow::Borrowed(unsafe {
            std::str::from_utf8_unchecked(&lower_buf[..tag_bytes.len()])
        })
    } else {
        std::borrow::Cow::Owned(tag_str.to_ascii_lowercase())
    };

    // 1. Check inline style="..." for text-align.
    if let Some(style_start) = tag_lower.find("style=") {
        let rest = &tag_lower[style_start + 6..];
        // The style value is in quotes (single or double).
        let quote = rest.as_bytes().first().copied();
        if matches!(quote, Some(b'"') | Some(b'\'')) {
            let inner = &rest[1..];
            if let Some(end) = inner.find(quote.unwrap() as char) {
                let style_value = &inner[..end];
                if let Some(ta_pos) = style_value.find("text-align") {
                    let after_ta = &style_value[ta_pos + 10..]; // skip "text-align"
                    let after_colon = after_ta
                        .trim_start()
                        .strip_prefix(':')
                        .unwrap_or(after_ta)
                        .trim_start();
                    if after_colon.starts_with("right") {
                        return "\\qr";
                    } else if after_colon.starts_with("center") {
                        return "\\qc";
                    }
                }
            }
        }
    }

    // 2. Check class="..." and resolve against the CSS alignment map.
    #[allow(clippy::collapsible_if)]
    if !align_map.is_empty() {
        if let Some(class_start) = tag_lower.find("class=") {
            let rest = &tag_lower[class_start + 6..];
            let quote = rest.as_bytes().first().copied();
            if matches!(quote, Some(b'"') | Some(b'\'')) {
                let inner = &rest[1..];
                if let Some(end) = inner.find(quote.unwrap() as char) {
                    let class_value = &inner[..end];
                    // A class attribute can contain multiple space-separated classes.
                    for class_name in class_value.split_whitespace() {
                        if let Some(&align) = align_map.get(class_name) {
                            match align {
                                "right" => return "\\qr",
                                "center" => return "\\qc",
                                _ => {},
                            }
                        }
                    }
                }
            }
        }
    }

    "\\ql"
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
            // Use a stack buffer to avoid format!() machinery overhead.
            rtf.push_str("\\u");
            push_i32(rtf, c as i32);
            rtf.push('?');
        },
        c => rtf.push(c),
    }
}

/// Pushes the decimal representation of an `i32` into a string using a stack
/// buffer, avoiding the overhead of `write!`/`format!` machinery.
fn push_i32(s: &mut String, value: i32) {
    if value == 0 {
        s.push('0');
        return;
    }
    // An i32 has at most 10 digits + optional '-' sign = 11 bytes.
    let mut buf = [0u8; 11];
    let mut pos = buf.len();
    let negative = value < 0;
    // Work with the absolute value as u32 to avoid overflow on i32::MIN.
    let mut n = (value as i64).unsigned_abs() as u32;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if negative {
        pos -= 1;
        buf[pos] = b'-';
    }
    // SAFETY: buf[pos..] contains only ASCII digits and possibly '-'.
    let digits = unsafe { std::str::from_utf8_unchecked(&buf[pos..]) };
    s.push_str(digits);
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
        book.add_chapter(Chapter {
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
        let empty_map = HashMap::new();
        html_to_rtf("<p>Hello</p><p>World</p>", &mut rtf, false, &empty_map);
        assert!(rtf.contains("Hello"));
        assert!(rtf.contains("\\par"));
        assert!(rtf.contains("World"));
    }

    #[test]
    fn html_to_rtf_handles_bold_italic() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<b>Bold</b> and <i>Italic</i>", &mut rtf, false, &empty_map);
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

    #[test]
    fn rtf_writer_no_duplicate_heading() {
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<h1>Ch 1</h1><p>Body text</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        // The title "Ch 1" should appear exactly once as a styled heading.
        let count = rtf.matches("Ch 1").count();
        assert_eq!(
            count, 1,
            "Expected 'Ch 1' once, found {count} times in: {rtf}"
        );
        assert!(rtf.contains("Body text"));
    }

    #[test]
    fn stylesheet_group_present() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Title".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("{\\stylesheet"),
            "RTF should contain a stylesheet group"
        );
        assert!(
            rtf.contains("\\s0\\ql\\sa120\\fi360\\f0\\fs24 Normal;"),
            "Stylesheet should contain Normal style with alignment and spacing"
        );
        assert!(
            rtf.contains("\\s1\\qc\\sb360\\sa120\\keepn\\outlinelevel0\\f0\\fs48\\b Heading 1;"),
            "Stylesheet should contain Heading 1 style with outline level"
        );
        assert!(
            rtf.contains("\\s6\\qc\\sb120\\sa120\\keepn\\outlinelevel5\\f0\\fs24\\i Heading 6;"),
            "Stylesheet should contain Heading 6 style with outline level"
        );
    }

    #[test]
    fn heading_levels_produce_different_font_sizes() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>",
            &mut rtf,
            false,
            &empty_map,
        );

        assert!(
            rtf.contains("\\pard\\s1\\qc\\sb360\\sa120\\keepn\\f0\\fs48\\b H1"),
            "H1 should use fs48 centered with spacing, got: {rtf}"
        );
        assert!(
            rtf.contains("\\pard\\s2\\qc\\sb300\\sa120\\keepn\\f0\\fs36\\b H2"),
            "H2 should use fs36 centered with spacing, got: {rtf}"
        );
        assert!(
            rtf.contains("\\pard\\s3\\qc\\sb240\\sa120\\keepn\\f0\\fs32\\b H3"),
            "H3 should use fs32 centered with spacing, got: {rtf}"
        );
        assert!(
            rtf.contains("\\pard\\s4\\qc\\sb200\\sa120\\keepn\\f0\\fs28\\b H4"),
            "H4 should use fs28 centered with spacing, got: {rtf}"
        );
        assert!(
            rtf.contains("\\pard\\s5\\qc\\sb160\\sa120\\keepn\\f0\\fs24\\b\\i H5"),
            "H5 should use fs24 bold italic centered with spacing, got: {rtf}"
        );
        assert!(
            rtf.contains("\\pard\\s6\\qc\\sb120\\sa120\\keepn\\f0\\fs24\\i H6"),
            "H6 should use fs24 italic centered with spacing, got: {rtf}"
        );
    }

    #[test]
    fn normal_style_restored_after_heading() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<h2>Title</h2><p>Body</p>", &mut rtf, false, &empty_map);

        // After the heading, the next <p> restores Normal style with \pard\s0\ql.
        assert!(
            rtf.contains("\\pard\\s0\\ql\\sb0\\sa120\\f0\\fs24 Body"),
            "Normal style should be restored on paragraph after heading, got: {rtf}"
        );
    }

    #[test]
    fn chapter_title_uses_heading1_style() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("My Chapter".into()),
            content: "<p>Body</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("\\pard\\s1\\qc\\sb360\\sa120\\keepn\\f0\\fs48\\b My Chapter"),
            "Chapter title should use Heading 1 style centered with spacing, got: {rtf}"
        );
    }

    /// Builds a minimal valid JPEG with the given dimensions.
    /// Contains SOI, SOF0 with dimensions, and EOI markers.
    fn make_fake_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut data = vec![
            0xFF, 0xD8, // SOI
            0xFF, 0xC0, // SOF0 marker
            0x00, 0x0B, // segment length (11 bytes)
            0x08, // precision (8-bit)
        ];
        data.extend_from_slice(&height.to_be_bytes());
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]); // 1 component
        data.extend_from_slice(&[0xFF, 0xD9]); // EOI
        data
    }

    /// Builds a minimal valid PNG with the given dimensions.
    fn make_fake_png(width: u32, height: u32) -> Vec<u8> {
        let mut data = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR chunk length (13)
            b'I', b'H', b'D', b'R', // chunk type
        ];
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());
        data.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]); // bit depth, color type, etc.
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // CRC placeholder
        data
    }

    #[test]
    fn cover_image_jpeg_embedded_via_metadata_id() {
        let jpeg_data = make_fake_jpeg(800, 600);
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.metadata.cover_image_id = Some("cover-img".into());
        book.add_resource("cover-img", "images/cover.jpg", jpeg_data, "image/jpeg");
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("\\pict\\jpegblip"),
            "RTF should contain a JPEG pict group, got: {rtf}"
        );
        // 800px at 96dpi = 800*1440/96 = 12000 twips
        assert!(
            rtf.contains("\\picwgoal12000"),
            "Picture width should be 12000 twips, got: {rtf}"
        );
        // 600px at 96dpi = 600*1440/96 = 9000 twips
        assert!(
            rtf.contains("\\pichgoal9000"),
            "Picture height should be 9000 twips, got: {rtf}"
        );
        // Cover should appear before chapter content.
        let pict_pos = rtf.find("\\pict").unwrap();
        let chapter_pos = rtf.find("Ch 1").unwrap();
        assert!(
            pict_pos < chapter_pos,
            "Cover image should appear before chapter content"
        );
    }

    #[test]
    fn cover_image_png_embedded() {
        let png_data = make_fake_png(640, 480);
        let mut book = Book::new();
        book.metadata.cover_image_id = Some("cover-png".into());
        book.add_resource("cover-png", "images/cover.png", png_data, "image/png");
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("\\pict\\pngblip"),
            "RTF should contain a PNG pict group, got: {rtf}"
        );
        // 640px at 96dpi = 640*1440/96 = 9600 twips
        assert!(
            rtf.contains("\\picwgoal9600"),
            "Picture width should be 9600 twips, got: {rtf}"
        );
    }

    #[test]
    fn cover_image_found_by_heuristic() {
        let jpeg_data = make_fake_jpeg(400, 500);
        let mut book = Book::new();
        // No cover_image_id set — fallback to heuristic matching.
        book.add_resource("my-cover", "images/cover.jpg", jpeg_data, "image/jpeg");
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("\\pict\\jpegblip"),
            "RTF should find cover image by heuristic, got: {rtf}"
        );
    }

    #[test]
    fn no_pict_when_no_cover() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Ch 1".into()),
            content: "<p>Hello</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            !rtf.contains("\\pict"),
            "RTF should not contain \\pict when there is no cover image"
        );
    }

    #[test]
    fn cover_image_hex_encoding() {
        // Use a small known JPEG to verify hex encoding.
        let jpeg_data = make_fake_jpeg(100, 200);
        let mut book = Book::new();
        book.metadata.cover_image_id = Some("cover".into());
        book.add_resource("cover", "cover.jpg", jpeg_data.clone(), "image/jpeg");

        let rtf = book_to_rtf(&book);
        // The hex data should start with FFD8FFC0 (SOI + SOF0).
        assert!(
            rtf.contains("FFD8FFC0"),
            "Hex-encoded JPEG should start with FFD8FFC0, got: {rtf}"
        );
    }

    #[test]
    fn parse_jpeg_dimensions_basic() {
        let data = make_fake_jpeg(1024, 768);
        let (w, h) = parse_jpeg_dimensions(&data).unwrap();
        assert_eq!(w, 1024);
        assert_eq!(h, 768);
    }

    #[test]
    fn parse_png_dimensions_basic() {
        let data = make_fake_png(1920, 1080);
        let (w, h) = parse_png_dimensions(&data).unwrap();
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
    }

    #[test]
    fn parse_jpeg_dimensions_returns_none_for_invalid() {
        assert!(parse_jpeg_dimensions(&[0x00, 0x01, 0x02]).is_none());
    }

    #[test]
    fn parse_png_dimensions_returns_none_for_invalid() {
        assert!(parse_png_dimensions(&[0x00, 0x01, 0x02]).is_none());
    }

    // --- Tests for alignment, spacing, and first-line indent ---

    #[test]
    fn headings_have_center_alignment() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<h1>Centered</h1>", &mut rtf, false, &empty_map);
        assert!(
            rtf.contains("\\qc"),
            "Headings should contain \\qc for center alignment, got: {rtf}"
        );
    }

    #[test]
    fn headings_have_space_before_and_after() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<h1>H1</h1><h3>H3</h3><h6>H6</h6>",
            &mut rtf,
            false,
            &empty_map,
        );

        // H1: sb360, sa120
        assert!(
            rtf.contains("\\sb360\\sa120"),
            "H1 should have \\sb360\\sa120, got: {rtf}"
        );
        // H3: sb240, sa120
        assert!(
            rtf.contains("\\sb240\\sa120"),
            "H3 should have \\sb240\\sa120, got: {rtf}"
        );
        // H6: sb120, sa120
        assert!(
            rtf.contains("\\sb120\\sa120"),
            "H6 should have \\sb120\\sa120, got: {rtf}"
        );
    }

    #[test]
    fn heading_spacing_scales_by_level() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>",
            &mut rtf,
            false,
            &empty_map,
        );

        // Verify each level has its own sb value (descending).
        assert!(rtf.contains("\\sb360"), "H1 sb360, got: {rtf}");
        assert!(rtf.contains("\\sb300"), "H2 sb300, got: {rtf}");
        assert!(rtf.contains("\\sb240"), "H3 sb240, got: {rtf}");
        assert!(rtf.contains("\\sb200"), "H4 sb200, got: {rtf}");
        assert!(rtf.contains("\\sb160"), "H5 sb160, got: {rtf}");
        assert!(rtf.contains("\\sb120"), "H6 sb120, got: {rtf}");
    }

    #[test]
    fn body_paragraphs_have_space_after() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<p>First</p><p>Second</p>", &mut rtf, false, &empty_map);
        assert!(
            rtf.contains("\\sa120"),
            "Body paragraphs should have \\sa120 space after, got: {rtf}"
        );
    }

    #[test]
    fn body_paragraphs_have_first_line_indent() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<p>First</p><p>Second</p>", &mut rtf, false, &empty_map);

        // The first paragraph gets \fi360 (no after_break suppression).
        assert!(
            rtf.contains("\\fi360"),
            "Body paragraphs should have \\fi360 first-line indent, got: {rtf}"
        );
    }

    #[test]
    fn headings_do_not_have_first_line_indent() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<h2>Title</h2>", &mut rtf, false, &empty_map);

        // The heading paragraph should NOT contain \fi.
        let heading_part = &rtf[..rtf.find("\\par").unwrap_or(rtf.len())];
        assert!(
            !heading_part.contains("\\fi"),
            "Headings should not have \\fi first-line indent, got: {rtf}"
        );
    }

    #[test]
    fn first_paragraph_after_heading_suppresses_indent() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<h2>Title</h2><p>First</p><p>Second</p>",
            &mut rtf,
            false,
            &empty_map,
        );

        // The first <p> after heading should use \sb0 and no \fi (suppress_indent).
        assert!(
            rtf.contains("\\pard\\s0\\ql\\sb0\\sa120\\f0\\fs24 First"),
            "First paragraph after heading should suppress indent with \\sb0, got: {rtf}"
        );

        // The second <p> should have normal \fi360 indent.
        assert!(
            rtf.contains("\\pard\\s0\\ql\\sa120\\fi360\\f0\\fs24 Second"),
            "Second paragraph should have \\fi360 indent, got: {rtf}"
        );
    }

    #[test]
    fn first_paragraph_after_break_suppresses_indent() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        // Simulate content called with after_break=true (as book_to_rtf does).
        html_to_rtf("<p>First</p><p>Second</p>", &mut rtf, true, &empty_map);

        // First paragraph: suppressed indent (\\sb0, no \\fi).
        assert!(
            rtf.contains("\\pard\\s0\\ql\\sb0\\sa120\\f0\\fs24 First"),
            "First paragraph after break should suppress indent, got: {rtf}"
        );

        // Second paragraph: normal indent.
        assert!(
            rtf.contains("\\pard\\s0\\ql\\sa120\\fi360\\f0\\fs24 Second"),
            "Second paragraph should have normal indent, got: {rtf}"
        );
    }

    #[test]
    fn paragraphs_use_left_alignment() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<p>Text</p>", &mut rtf, false, &empty_map);
        assert!(
            rtf.contains("\\ql"),
            "Body paragraphs should use \\ql left alignment, got: {rtf}"
        );
    }

    #[test]
    fn chapter_title_centered_with_spacing_in_book() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("My Title".into()),
            content: "<p>Body</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        // Chapter title should be centered.
        assert!(
            rtf.contains("\\qc"),
            "Chapter title should be centered, got: {rtf}"
        );
        // Chapter title should have spacing.
        assert!(
            rtf.contains("\\sb360\\sa120"),
            "Chapter title should have spacing, got: {rtf}"
        );
    }

    // --- Tests for right-align and class-based alignment ---

    #[test]
    fn inline_style_right_align_emits_qr() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<p style=\"text-align: right\">Right aligned</p>",
            &mut rtf,
            false,
            &empty_map,
        );
        assert!(
            rtf.contains("\\qr"),
            "Paragraph with text-align: right should emit \\qr, got: {rtf}"
        );
        assert!(
            rtf.contains("Right aligned"),
            "Content should be preserved, got: {rtf}"
        );
    }

    #[test]
    fn inline_style_center_align_emits_qc() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf(
            "<p style=\"text-align: center\">Centered</p>",
            &mut rtf,
            false,
            &empty_map,
        );
        assert!(
            rtf.contains("\\qc"),
            "Paragraph with text-align: center should emit \\qc, got: {rtf}"
        );
    }

    #[test]
    fn class_based_right_align_emits_qr() {
        let mut rtf = String::new();
        let mut align_map = HashMap::new();
        align_map.insert("right".to_string(), "right");
        html_to_rtf(
            "<p class=\"right\">Right aligned</p>",
            &mut rtf,
            false,
            &align_map,
        );
        assert!(
            rtf.contains("\\qr"),
            "Paragraph with class='right' should emit \\qr, got: {rtf}"
        );
    }

    #[test]
    fn class_based_center_align_emits_qc() {
        let mut rtf = String::new();
        let mut align_map = HashMap::new();
        align_map.insert("center".to_string(), "center");
        html_to_rtf(
            "<p class=\"center\">Centered</p>",
            &mut rtf,
            false,
            &align_map,
        );
        assert!(
            rtf.contains("\\qc"),
            "Paragraph with class='center' should emit \\qc, got: {rtf}"
        );
    }

    #[test]
    fn parse_css_alignment_rules_extracts_class() {
        let mut map = HashMap::new();
        parse_css_alignment_rules("p.right { text-align: right; }", &mut map);
        assert_eq!(map.get("right"), Some(&"right"));
    }

    #[test]
    fn parse_css_alignment_rules_dot_only_selector() {
        let mut map = HashMap::new();
        parse_css_alignment_rules(".centered { text-align: center; }", &mut map);
        assert_eq!(map.get("centered"), Some(&"center"));
    }

    #[test]
    fn parse_css_alignment_rules_multiple_rules() {
        let mut map = HashMap::new();
        parse_css_alignment_rules(
            "p.right { text-align: right; }\n.center { text-align: center; }",
            &mut map,
        );
        assert_eq!(map.get("right"), Some(&"right"));
        assert_eq!(map.get("center"), Some(&"center"));
    }

    #[test]
    fn stylesheet_has_outline_levels() {
        let mut book = Book::new();
        book.add_chapter(Chapter {
            title: Some("Title".into()),
            content: "<p>Text</p>".into(),
            id: Some("ch1".into()),
        });

        let rtf = book_to_rtf(&book);
        assert!(
            rtf.contains("\\outlinelevel0"),
            "H1 style should have \\outlinelevel0, got: {rtf}"
        );
        assert!(
            rtf.contains("\\outlinelevel1"),
            "H2 style should have \\outlinelevel1, got: {rtf}"
        );
        assert!(
            rtf.contains("\\outlinelevel2"),
            "H3 style should have \\outlinelevel2, got: {rtf}"
        );
        assert!(
            rtf.contains("\\outlinelevel3"),
            "H4 style should have \\outlinelevel3, got: {rtf}"
        );
        assert!(
            rtf.contains("\\outlinelevel4"),
            "H5 style should have \\outlinelevel4, got: {rtf}"
        );
        assert!(
            rtf.contains("\\outlinelevel5"),
            "H6 style should have \\outlinelevel5, got: {rtf}"
        );
    }

    #[test]
    fn leading_newline_in_paragraph_is_trimmed() {
        let mut rtf = String::new();
        let mut align_map = HashMap::new();
        align_map.insert("right".to_string(), "right");
        // Simulate HTML with a newline between <p> tag and text content,
        // as commonly found in real-world EPUBs.
        html_to_rtf(
            "<p class=\"right\">\nSt. Petersburgh, Dec. 11th</p>",
            &mut rtf,
            false,
            &align_map,
        );
        // The \qr paragraph should contain the text directly, without a
        // spurious \par from the leading newline.
        assert!(rtf.contains("\\qr"), "Should contain \\qr, got: {rtf}");
        assert!(
            !rtf.contains("\\qr\\sa120\\fi360\\f0\\fs24 \\par"),
            "Leading newline should not produce \\par before text, got: {rtf}"
        );
        assert!(
            rtf.contains("St. Petersburgh"),
            "Text content should be present, got: {rtf}"
        );
    }

    #[test]
    fn leading_whitespace_trimmed_for_left_aligned_paragraph() {
        let mut rtf = String::new();
        let empty_map = HashMap::new();
        html_to_rtf("<p>\n  Hello world</p>", &mut rtf, false, &empty_map);
        // Should not start with \par from the leading newline.
        assert!(
            !rtf.contains("\\fs24 \\par"),
            "Leading whitespace should not produce \\par, got: {rtf}"
        );
        assert!(
            rtf.contains("Hello world"),
            "Text content should be present, got: {rtf}"
        );
    }
}
