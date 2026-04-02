//! LRF text stream parsing and HTML generation.
//!
//! Text objects in LRF contain UTF-16LE text interspersed with inline
//! formatting tags (2-byte `XX F5` markers). This module scans the raw
//! stream, splits it into text segments and tag commands, and produces HTML.

use super::header::read_u16_le;
use super::tags::*;

/// A token produced by scanning a Text object's stream.
#[derive(Debug)]
pub enum TextToken {
    /// Raw text content (already decoded from UTF-16LE).
    Text(String),
    /// An inline formatting tag with its payload.
    Tag(Tag),
}

/// Scans a Text object's decompressed stream into a sequence of tokens.
///
/// The stream is UTF-16LE text interspersed with `0xF5` tag markers.
/// We scan forward looking for `0xF5` bytes, then back up one byte
/// to check if it's a valid tag.
pub fn tokenize_text_stream(stream: &[u8]) -> Vec<TextToken> {
    let mut tokens = Vec::new();
    let mut pos = 0;

    while pos < stream.len() {
        // Find the next 0xF5 marker.
        let marker_pos = stream[pos..].iter().position(|&b| b == 0xF5);

        match marker_pos {
            Some(0) => {
                // 0xF5 at the very start of remaining data — skip it.
                pos += 1;
            }
            Some(rel_offset) => {
                let abs_marker = pos + rel_offset;
                // The tag starts one byte before the 0xF5 marker.
                let tag_start = abs_marker - 1;

                // Everything before the tag is text.
                if tag_start > pos {
                    let text_bytes = &stream[pos..tag_start];
                    let text = decode_utf16le(text_bytes);
                    if !text.is_empty() {
                        tokens.push(TextToken::Text(text));
                    }
                }

                // Try to parse the tag.
                match parse_tag(stream, tag_start) {
                    Ok((tag, new_pos)) => {
                        tokens.push(TextToken::Tag(tag));
                        pos = new_pos;
                    }
                    Err(_) => {
                        // Not a valid tag — treat the 0xF5 as part of text and skip.
                        pos = abs_marker + 1;
                    }
                }
            }
            None => {
                // No more tags — rest is text.
                if pos < stream.len() {
                    let text = decode_utf16le(&stream[pos..]);
                    if !text.is_empty() {
                        tokens.push(TextToken::Text(text));
                    }
                }
                break;
            }
        }
    }

    tokens
}

/// Converts a sequence of text tokens into HTML.
pub fn tokens_to_html(tokens: &[TextToken]) -> String {
    let mut html = String::with_capacity(tokens.len() * 32);
    let mut in_para = false;

    for token in tokens {
        match token {
            TextToken::Text(text) => {
                html.push_str(&html_escape(text));
            }
            TextToken::Tag(tag) => {
                match tag.id {
                    TAG_TEXT_P_START => {
                        if in_para {
                            html.push_str("</p>");
                        }
                        html.push_str("<p>");
                        in_para = true;
                    }
                    TAG_TEXT_P_END => {
                        if in_para {
                            html.push_str("</p>\n");
                            in_para = false;
                        }
                    }
                    TAG_TEXT_CR => {
                        html.push_str("<br />");
                    }
                    TAG_TEXT_ITALIC_START => html.push_str("<i>"),
                    TAG_TEXT_ITALIC_END => html.push_str("</i>"),
                    TAG_TEXT_SUP_START => html.push_str("<sup>"),
                    TAG_TEXT_SUP_END => html.push_str("</sup>"),
                    TAG_TEXT_SUB_START => html.push_str("<sub>"),
                    TAG_TEXT_SUB_END => html.push_str("</sub>"),
                    TAG_TEXT_NOBR_START => html.push_str("<nobr>"),
                    TAG_TEXT_NOBR_END => html.push_str("</nobr>"),
                    TAG_TEXT_EMPLINE_START => html.push_str("<u>"),
                    TAG_TEXT_EMPLINE_END => html.push_str("</u>"),
                    TAG_TEXT_CHAR_BUTTON => {
                        // Link: 4-byte refobj ID in payload.
                        let refobj = tag.as_u32();
                        html.push_str(&format!("<a href=\"#obj_{}\">", refobj));
                    }
                    TAG_TEXT_CHAR_BUTTON_END => html.push_str("</a>"),
                    TAG_TEXT_PLOT => {
                        // Inline image: u16 xsize, u16 ysize, u32 refobj, u32 adjustment.
                        if tag.contents.len() >= 8 {
                            let _xsize = read_u16_le(&tag.contents, 0);
                            let _ysize = read_u16_le(&tag.contents, 2);
                            let refobj = u32::from_le_bytes([
                                tag.contents[4],
                                tag.contents[5],
                                tag.contents[6],
                                tag.contents[7],
                            ]);
                            html.push_str(&format!("<img src=\"#obj_{}\" />", refobj));
                        }
                    }
                    TAG_TEXT_CR_GRAPH => {
                        // Inline text block: u16 length, then UTF-16LE.
                        let text = tag.as_string();
                        html.push_str(&html_escape(&text));
                    }
                    TAG_TEXT_SPACE => {
                        html.push(' ');
                    }
                    // Style span tags — apply inline styles.
                    TAG_FONT_SIZE | TAG_FONT_WEIGHT | TAG_FONT_FACE
                    | TAG_TEXT_COLOR | TAG_TEXT_BG_COLOR | TAG_LINE_SPACE
                    | TAG_PAR_INDENT | TAG_ALIGN => {
                        // Emit a span with the style attribute.
                        if let Some(css) = tag_to_css(tag) {
                            html.push_str(&format!("<span style=\"{}\">", css));
                        }
                    }
                    _ => {
                        // Unknown tag — ignore silently.
                    }
                }
            }
        }
    }

    // Close any unclosed paragraph.
    if in_para {
        html.push_str("</p>\n");
    }

    if html.is_empty() {
        html.push_str("<p></p>");
    }

    html
}

/// Converts a style tag to a CSS property string.
fn tag_to_css(tag: &Tag) -> Option<String> {
    match tag.id {
        TAG_FONT_SIZE => {
            let size = tag.as_i16();
            // LRF font sizes are in 1/10 pt.
            let pt = size as f32 / 10.0;
            Some(format!("font-size: {:.1}pt", pt))
        }
        TAG_FONT_WEIGHT => {
            let weight = tag.as_u16();
            if weight >= 700 {
                Some("font-weight: bold".into())
            } else {
                Some("font-weight: normal".into())
            }
        }
        TAG_FONT_FACE => {
            let face = tag.as_string();
            Some(format!("font-family: '{}'", face))
        }
        TAG_TEXT_COLOR => {
            let (r, g, b) = decode_lrf_color(tag.as_u32());
            Some(format!("color: rgb({},{},{})", r, g, b))
        }
        TAG_TEXT_BG_COLOR => {
            let (r, g, b) = decode_lrf_color(tag.as_u32());
            Some(format!("background-color: rgb({},{},{})", r, g, b))
        }
        TAG_ALIGN => {
            let align = tag.as_u16();
            let name = match align {
                1 => "left",
                4 => "center",
                8 => "right",
                _ => "left",
            };
            Some(format!("text-align: {}", name))
        }
        TAG_PAR_INDENT => {
            let indent = tag.as_i16();
            let pt = indent as f32 / 10.0;
            Some(format!("text-indent: {:.1}pt", pt))
        }
        _ => None,
    }
}

/// Decodes an LRF ARGB color value to (R, G, B).
/// LRF stores colors as: a=byte0, r=byte1, g=byte2, b=byte3.
fn decode_lrf_color(val: u32) -> (u8, u8, u8) {
    let r = ((val >> 8) & 0xFF) as u8;
    let g = ((val >> 16) & 0xFF) as u8;
    let b = ((val >> 24) & 0xFF) as u8;
    (r, g, b)
}

/// Escapes HTML special characters.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_plain_text() {
        // "Hi" in UTF-16LE
        let stream = [0x48, 0x00, 0x69, 0x00];
        let tokens = tokenize_text_stream(&stream);
        assert_eq!(tokens.len(), 1);
        if let TextToken::Text(ref t) = tokens[0] {
            assert_eq!(t, "Hi");
        } else {
            panic!("Expected text token");
        }
    }

    #[test]
    fn tokenize_with_inline_tags() {
        let mut stream = Vec::new();
        // Text "A" in UTF-16LE
        stream.extend_from_slice(&[0x41, 0x00]);
        // Italic start tag: 0x81 0xF5
        stream.extend_from_slice(&[0x81, 0xF5]);
        // Text "B"
        stream.extend_from_slice(&[0x42, 0x00]);
        // Italic end tag: 0x82 0xF5
        stream.extend_from_slice(&[0x82, 0xF5]);

        let tokens = tokenize_text_stream(&stream);
        // Should have: Text("A"), Tag(Italic start), Text("B"), Tag(Italic end)
        assert!(tokens.len() >= 3);
    }

    #[test]
    fn html_escapes_special_chars() {
        assert_eq!(html_escape("A & B < C > D"), "A &amp; B &lt; C &gt; D");
    }

    #[test]
    fn decode_color_rgb() {
        // ARGB: a=0x00, r=0xFF, g=0x80, b=0x40
        // As u32 LE: 0x408000FF → b=0x40 at byte3, g=0x80 at byte2, r=0xFF at byte1, a=0x00 at byte0
        // Actually the encoding is: val = a | (r<<8) | (g<<16) | (b<<24)
        let val: u32 = (0xFF << 8) | (0x80 << 16) | (0x40 << 24);
        let (r, g, b) = decode_lrf_color(val);
        assert_eq!((r, g, b), (0xFF, 0x80, 0x40));
    }

    #[test]
    fn tokens_to_html_paragraph() {
        let tokens = vec![
            TextToken::Tag(Tag { id: TAG_TEXT_P_START, contents: vec![0; 6] }),
            TextToken::Text("Hello world".into()),
            TextToken::Tag(Tag { id: TAG_TEXT_P_END, contents: vec![] }),
        ];
        let html = tokens_to_html(&tokens);
        assert!(html.contains("<p>Hello world</p>"));
    }

    #[test]
    fn tokens_to_html_italic() {
        let tokens = vec![
            TextToken::Tag(Tag { id: TAG_TEXT_P_START, contents: vec![0; 6] }),
            TextToken::Text("Normal ".into()),
            TextToken::Tag(Tag { id: TAG_TEXT_ITALIC_START, contents: vec![] }),
            TextToken::Text("italic".into()),
            TextToken::Tag(Tag { id: TAG_TEXT_ITALIC_END, contents: vec![] }),
            TextToken::Tag(Tag { id: TAG_TEXT_P_END, contents: vec![] }),
        ];
        let html = tokens_to_html(&tokens);
        assert!(html.contains("Normal <i>italic</i>"));
    }
}
