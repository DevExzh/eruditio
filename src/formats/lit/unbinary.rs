//! LIT binary-to-HTML state machine decoder.
//!
//! Converts LIT binary token streams into HTML/OPF text. Ported from
//! calibre's `ebooks/lit/reader.py` (`UnBinary` class).

use std::borrow::Cow;
use std::collections::HashMap;

use crate::error::{EruditioError, Result};

use super::maps::LitMap;

const FLAG_OPENING: u8 = 0x01;
const FLAG_CLOSING: u8 = 0x02;
const FLAG_ATOM: u8 = 0x10;

/// Atom tables for custom per-document tags and attributes.
#[derive(Default)]
pub(crate) struct AtomTable {
    pub tags: HashMap<u32, String>,
    pub attrs: HashMap<u32, String>,
}

/// Manifest entry used for href resolution in unbinary decoding.
pub(crate) struct ManifestPath {
    pub path: String,
}

/// Read one UTF-8 character from raw bytes at `pos`.
pub(super) fn read_utf8_char(data: &[u8], pos: usize) -> Result<(char, usize)> {
    if pos >= data.len() {
        return Err(EruditioError::Parse("Unexpected end of data".into()));
    }
    let b = data[pos];
    let (len, mask): (usize, u32) = match b.leading_ones() {
        0 => (1, 0x7F),
        2 => (2, 0x1F),
        3 => (3, 0x0F),
        4 => (4, 0x07),
        _ => {
            return Err(EruditioError::Parse(format!(
                "Invalid UTF-8 leading byte: 0x{b:02X}"
            )));
        },
    };
    if pos + len > data.len() {
        return Err(EruditioError::Parse("Truncated UTF-8 sequence".into()));
    }
    let mut code = u32::from(b) & mask;
    for i in 1..len {
        let cb = data[pos + i];
        if cb & 0xC0 != 0x80 {
            return Err(EruditioError::Parse("Invalid UTF-8 continuation".into()));
        }
        code = (code << 6) | u32::from(cb & 0x3F);
    }
    let c = char::from_u32(code)
        .ok_or_else(|| EruditioError::Parse(format!("Invalid codepoint: {code}")))?;
    Ok((c, pos + len))
}

/// Read a sized UTF-8 string: first char's ordinal = length, then that many chars.
pub(super) fn consume_sized_utf8_string(
    data: &[u8],
    pos: usize,
    zpad: bool,
) -> Result<(String, usize)> {
    let (len_char, mut pos) = read_utf8_char(data, pos)?;
    let len = len_char as u32 as usize;
    let mut result = String::with_capacity(len);
    for _ in 0..len {
        let (c, new_pos) = read_utf8_char(data, pos)?;
        result.push(c);
        pos = new_pos;
    }
    if zpad && pos < data.len() && data[pos] == 0 {
        pos += 1;
    }
    Ok((result, pos))
}

// ---------------------------------------------------------------------------
// State machine internals
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum State {
    Text,
    GetFlags,
    GetTag,
    GetAttr,
    GetValueLength,
    GetValue,
    GetCustomLength,
    GetCustom,
    GetAttrLength,
    GetCustomAttr,
    GetHrefLength,
    GetHref,
    CloseTag,
}

struct Frame {
    depth: u32,
    tag_name: Option<String>,
    tag_index: Option<usize>,
    use_atoms: bool,
    is_goingdown: bool,
    in_censorship: bool,
    state: State,
    flags: u8,
    count: i64,
    href_buf: String,
}

impl Frame {
    fn new_root() -> Self {
        Self {
            depth: 0,
            tag_name: None,
            tag_index: None,
            use_atoms: false,
            is_goingdown: false,
            in_censorship: false,
            state: State::Text,
            flags: 0,
            count: 0,
            href_buf: String::new(),
        }
    }
}

fn encode_char(c: char, buf: &mut String) {
    if c.is_ascii() {
        buf.push(c);
    } else {
        use std::fmt::Write;
        let _ = write!(buf, "&#{};", c as u32);
    }
}

fn encode_str(s: &str, buf: &mut String) {
    for c in s.chars() {
        encode_char(c, buf);
    }
}

fn item_path(id: &str, dir: &str, manifest: &HashMap<String, ManifestPath>) -> String {
    let target = match manifest.get(id) {
        Some(e) => &e.path,
        None => return id.to_string(),
    };
    if dir.is_empty() {
        return target.clone();
    }
    let tp: Vec<&str> = target.split('/').collect();
    let bp: Vec<&str> = dir.split('/').collect();
    let common = bp.iter().zip(tp.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = (0..bp.len() - common).map(|_| "..").collect();
    parts.extend(&tp[common..]);
    parts.join("/")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert LIT binary token data to HTML/OPF text.
pub(crate) fn unbinary_to_html(
    bin: &[u8],
    path: &str,
    manifest: &HashMap<String, ManifestPath>,
    map: &LitMap,
    atoms: &AtomTable,
) -> Result<String> {
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut buf = String::new();
    let mut cpos: usize = 0;
    let mut stack = vec![Frame::new_root()];

    while let Some(mut frame) = stack.pop() {
        if frame.state == State::CloseTag {
            if let Some(ref name) = frame.tag_name {
                buf.push_str("</");
                encode_str(name, &mut buf);
                buf.push('>');
            }
            frame.tag_name = None;
            frame.state = State::Text;
        }

        while cpos < bin.len() {
            let (c, new_pos) = read_utf8_char(bin, cpos)?;
            cpos = new_pos;
            let oc = c as u32;

            match frame.state {
                State::Text => {
                    if oc == 0 {
                        frame.state = State::GetFlags;
                    } else {
                        match c {
                            '\x0B' => buf.push('\n'),
                            '>' => buf.push_str(">>"),
                            '<' => buf.push_str("<<"),
                            _ => encode_char(c, &mut buf),
                        }
                    }
                },

                State::GetFlags => {
                    if oc == 0 {
                        frame.state = State::Text;
                    } else {
                        frame.flags = oc as u8;
                        frame.state = State::GetTag;
                    }
                },

                State::GetTag => {
                    frame.state = if oc == 0 { State::Text } else { State::GetAttr };
                    if frame.flags & FLAG_OPENING != 0 {
                        buf.push('<');
                        frame.is_goingdown = frame.flags & FLAG_CLOSING == 0;
                        if oc == 0x8000 {
                            frame.state = State::GetCustomLength;
                            continue;
                        }
                        if frame.flags & FLAG_ATOM != 0 {
                            match atoms.tags.get(&oc) {
                                Some(name) => {
                                    frame.tag_name = Some(name.clone());
                                    frame.tag_index = None;
                                    frame.use_atoms = true;
                                },
                                None => {
                                    // Missing atom — use placeholder (matches calibre behavior)
                                    frame.tag_name = Some(format!("?atom{oc}?"));
                                    frame.tag_index = None;
                                    frame.use_atoms = true;
                                },
                            }
                        } else if (oc as usize) < map.tags.len() {
                            let name = map.tags[oc as usize].unwrap_or("unknown").to_string();
                            frame.tag_index = Some(oc as usize);
                            frame.tag_name = Some(name);
                            frame.use_atoms = false;
                        } else {
                            frame.tag_name = Some(format!("?{oc}?"));
                            frame.tag_index = None;
                            frame.use_atoms = false;
                        }
                        if let Some(ref name) = frame.tag_name {
                            encode_str(name, &mut buf);
                        }
                    } else if frame.flags & FLAG_CLOSING != 0 {
                        break;
                    }
                },

                State::GetAttr => {
                    frame.in_censorship = false;
                    if oc == 0 {
                        frame.state = State::Text;
                        if !frame.is_goingdown {
                            frame.tag_name = None;
                            buf.push_str(" />");
                        } else {
                            buf.push('>');
                            let close = Frame {
                                depth: frame.depth,
                                tag_name: frame.tag_name.take(),
                                tag_index: frame.tag_index,
                                use_atoms: frame.use_atoms,
                                is_goingdown: false,
                                in_censorship: false,
                                state: State::CloseTag,
                                flags: frame.flags,
                                count: 0,
                                href_buf: String::new(),
                            };
                            let child = Frame {
                                depth: frame.depth + 1,
                                ..Frame::new_root()
                            };
                            stack.push(close);
                            stack.push(child);
                            break;
                        }
                    } else if oc == 0x8000 {
                        frame.state = State::GetAttrLength;
                    } else {
                        let attr = if frame.use_atoms {
                            atoms
                                .attrs
                                .get(&oc)
                                .map(|s| s.as_str())
                                .or_else(|| (map.global_attr)(oc as u16))
                        } else {
                            frame
                                .tag_index
                                .and_then(|idx| (map.tag_attr)(idx, oc as u16))
                                .or_else(|| (map.global_attr)(oc as u16))
                        };
                        let attr = attr.ok_or_else(|| {
                            EruditioError::Parse(format!(
                                "Unknown attr 0x{oc:04X} in {:?}",
                                frame.tag_name
                            ))
                        })?;
                        if attr.starts_with('%') {
                            frame.in_censorship = true;
                            frame.state = State::GetValueLength;
                        } else {
                            buf.push(' ');
                            encode_str(attr, &mut buf);
                            buf.push('=');
                            frame.state = if attr == "href" || attr == "src" {
                                State::GetHrefLength
                            } else {
                                State::GetValueLength
                            };
                        }
                    }
                },

                State::GetValueLength => {
                    if !frame.in_censorship {
                        buf.push('"');
                    }
                    let count = oc as i64 - 1;
                    if count == 0 {
                        if !frame.in_censorship {
                            buf.push('"');
                        }
                        frame.in_censorship = false;
                        frame.state = State::GetAttr;
                    } else if oc == 0xFFFF {
                        frame.count = 0xFFFE;
                        frame.state = State::GetValue;
                    } else {
                        frame.count = count;
                        frame.state = State::GetValue;
                    }
                },

                State::GetValue => {
                    if frame.count == 0xFFFE {
                        // Numeric value mode
                        if !frame.in_censorship {
                            buf.push_str(&(oc - 1).to_string());
                            buf.push('"');
                        }
                        frame.in_censorship = false;
                        frame.state = State::GetAttr;
                    } else {
                        if !frame.in_censorship {
                            match c {
                                '"' => buf.push_str("&quot;"),
                                '<' => buf.push_str("&lt;"),
                                _ => encode_char(c, &mut buf),
                            }
                        }
                        frame.count -= 1;
                        if frame.count <= 0 {
                            if !frame.in_censorship {
                                buf.push('"');
                            }
                            frame.in_censorship = false;
                            frame.state = State::GetAttr;
                        }
                    }
                },

                State::GetCustomLength => {
                    frame.count = oc as i64 - 1;
                    if frame.count <= 0 {
                        return Err(EruditioError::Parse("Invalid custom tag length".into()));
                    }
                    frame.tag_name = Some(String::new());
                    frame.state = State::GetCustom;
                },

                State::GetCustom => {
                    if let Some(ref mut name) = frame.tag_name {
                        name.push(c);
                    }
                    frame.count -= 1;
                    if frame.count == 0 {
                        if let Some(ref name) = frame.tag_name {
                            encode_str(name, &mut buf);
                        }
                        frame.state = State::GetAttr;
                    }
                },

                State::GetAttrLength => {
                    frame.count = oc as i64 - 1;
                    if frame.count <= 0 {
                        return Err(EruditioError::Parse("Invalid custom attr length".into()));
                    }
                    buf.push(' ');
                    frame.state = State::GetCustomAttr;
                },

                State::GetCustomAttr => {
                    encode_char(c, &mut buf);
                    frame.count -= 1;
                    if frame.count == 0 {
                        buf.push('=');
                        frame.state = State::GetValueLength;
                    }
                },

                State::GetHrefLength => {
                    frame.count = oc as i64 - 1;
                    if frame.count <= 0 {
                        return Err(EruditioError::Parse("Invalid href length".into()));
                    }
                    frame.href_buf.clear();
                    frame.state = State::GetHref;
                },

                State::GetHref => {
                    frame.href_buf.push(c);
                    frame.count -= 1;
                    if frame.count == 0 {
                        let href = &frame.href_buf;
                        let body = if href.len() > 1 { &href[1..] } else { "" };
                        let (doc, frag) = body
                            .find('#')
                            .map_or((body, None), |p| (&body[..p], Some(&body[p + 1..])));
                        let mut resolved = item_path(doc, dir, manifest);
                        if let Some(f) = frag {
                            resolved.push('#');
                            resolved.push_str(f);
                        }
                        buf.push('"');
                        encode_str(&resolved, &mut buf);
                        buf.push('"');
                        frame.state = State::GetAttr;
                    }
                },

                State::CloseTag => unreachable!(),
            }
        }
    }

    let result = escape_reserved(&buf);
    Ok(result.trim_start().to_string())
}

// ---------------------------------------------------------------------------
// Post-processing: escape reserved characters
// ---------------------------------------------------------------------------

/// Escape `<<`/`>>` (literal angle brackets in text) and bare `&` in a single pass.
/// Preserves HTML comment delimiters `<!--` and `-->`, and valid entity references.
fn escape_reserved(raw: &str) -> Cow<'_, str> {
    let bytes = raw.as_bytes();
    let len = bytes.len();

    // Fast scan: find first byte that might need escaping
    let first = bytes
        .iter()
        .position(|&b| b == b'&' || b == b'<' || b == b'>');
    let first = match first {
        Some(i) => i,
        None => return Cow::Borrowed(raw),
    };

    let mut result = String::with_capacity(len + 16);
    result.push_str(&raw[..first]);
    let mut i = first;

    while i < len {
        match bytes[i] {
            b'&' => {
                if is_entity_ref(bytes, i) {
                    result.push('&');
                } else {
                    result.push_str("&amp;");
                }
                i += 1;
            },
            b'<' if i + 1 < len && bytes[i + 1] == b'<' => {
                // Preserve comment start: <<! --
                if i + 4 < len
                    && bytes[i + 2] == b'!'
                    && bytes[i + 3] == b'-'
                    && bytes[i + 4] == b'-'
                {
                    result.push('<');
                } else {
                    result.push_str("&lt;");
                }
                i += 2;
            },
            b'>' if i + 1 < len && bytes[i + 1] == b'>' => {
                // Preserve comment end: -->>
                if result.ends_with("--") {
                    result.push('>');
                } else {
                    result.push_str("&gt;");
                }
                i += 2;
            },
            _ => {
                result.push(char::from(bytes[i]));
                i += 1;
            },
        }
    }

    Cow::Owned(result)
}

/// Check if `&` at `pos` starts a valid entity reference (`&name;`, `&#N;`, `&#xH;`).
fn is_entity_ref(bytes: &[u8], pos: usize) -> bool {
    let rest = &bytes[pos + 1..];
    if rest.starts_with(b"#x") || rest.starts_with(b"#X") {
        rest[2..]
            .iter()
            .take_while(|&&b| b != b';')
            .all(|&b| b.is_ascii_hexdigit())
            && rest[2..].contains(&b';')
    } else if rest.starts_with(b"#") {
        rest[1..]
            .iter()
            .take_while(|&&b| b != b';')
            .all(|&b| b.is_ascii_digit())
            && rest[1..].contains(&b';')
    } else {
        let first = rest.first().copied().unwrap_or(0);
        (first.is_ascii_alphabetic() || first == b'_' || first == b':')
            && rest
                .iter()
                .take_while(|&&b| b != b';')
                .all(|&b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
            && rest.contains(&b';')
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::maps::HTML_MAP;

    #[test]
    fn read_utf8_char_ascii() {
        let (c, pos) = read_utf8_char(b"ABC", 0).unwrap();
        assert_eq!(c, 'A');
        assert_eq!(pos, 1);
    }

    #[test]
    fn read_utf8_char_multibyte() {
        let data = "\u{00E9}".as_bytes(); // e-acute: 2 bytes
        let (c, pos) = read_utf8_char(data, 0).unwrap();
        assert_eq!(c, '\u{00E9}');
        assert_eq!(pos, 2);
    }

    #[test]
    fn read_utf8_char_null() {
        let (c, pos) = read_utf8_char(&[0x00], 0).unwrap();
        assert_eq!(c, '\0');
        assert_eq!(pos, 1);
    }

    #[test]
    fn read_utf8_char_three_byte() {
        let data = "\u{8000}".as_bytes(); // 3-byte CJK char
        let (c, pos) = read_utf8_char(data, 0).unwrap();
        assert_eq!(c as u32, 0x8000);
        assert_eq!(pos, 3);
    }

    #[test]
    fn consume_sized_string_basic() {
        let data = b"\x03abc";
        let (s, pos) = consume_sized_utf8_string(data, 0, false).unwrap();
        assert_eq!(s, "abc");
        assert_eq!(pos, 4);
    }

    #[test]
    fn consume_sized_string_with_zpad() {
        let data = b"\x02hi\x00rest";
        let (s, pos) = consume_sized_utf8_string(data, 0, true).unwrap();
        assert_eq!(s, "hi");
        assert_eq!(pos, 4);
    }

    #[test]
    fn unbinary_plain_text() {
        let bin = b"Hello";
        let result = unbinary_to_html(
            bin,
            "test.html",
            &HashMap::new(),
            &HTML_MAP,
            &AtomTable::default(),
        )
        .unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn escape_reserved_bare_ampersand() {
        assert_eq!(escape_reserved("a & b"), "a &amp; b");
        assert_eq!(escape_reserved("&amp;"), "&amp;");
        assert_eq!(escape_reserved("&#123;"), "&#123;");
    }

    #[test]
    fn escape_reserved_double_angles() {
        assert_eq!(escape_reserved("a << b"), "a &lt; b");
        assert_eq!(escape_reserved("a >> b"), "a &gt; b");
    }

    #[test]
    fn escape_reserved_preserves_comments() {
        assert_eq!(
            escape_reserved("<<!--- comment --->>"),
            "<!--- comment --->"
        );
    }

    #[test]
    fn item_path_resolves_relative() {
        let mut manifest = HashMap::new();
        manifest.insert(
            "ch1".to_string(),
            ManifestPath {
                path: "content/chapter1.html".to_string(),
            },
        );
        assert_eq!(item_path("ch1", "content", &manifest), "chapter1.html");
    }

    #[test]
    fn item_path_unknown_id() {
        let manifest = HashMap::new();
        assert_eq!(item_path("unknown", "", &manifest), "unknown");
    }
}
