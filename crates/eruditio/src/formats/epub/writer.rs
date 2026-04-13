use crate::domain::{Book, TocItem};
use crate::error::{EruditioError, Result};
use crate::formats::common::text_utils::push_escape_xml;
use crate::formats::common::zip_utils::ZIP_DEFLATE_LEVEL;
#[cfg(feature = "parallel")]
use flate2::{Compress, Compression, FlushCompress};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use std::borrow::Cow;
use std::fmt::Write as FmtWrite;
#[cfg(feature = "parallel")]
use std::io::Cursor;
use std::io::{Seek, Write};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::FileOptions;

/// Returns `true` for already-compressed binary media types that should use
/// `Stored` (no compression) in the ZIP archive.  Text-based entries (XHTML,
/// CSS, NCX, OPF, XML) compress very well with Deflate and should use it.
fn is_already_compressed(media_type: &str) -> bool {
    media_type.starts_with("image/")
        || media_type.starts_with("audio/")
        || media_type.starts_with("video/")
        || media_type.starts_with("font/")
        || media_type == "application/x-font-truetype"
        || media_type == "application/x-font-opentype"
        || media_type == "application/font-woff"
        || media_type == "application/font-woff2"
        || media_type == "application/vnd.ms-opentype"
}

/// Minimum uncompressed entry size to justify Deflate compression.
///
/// Each deflate init/reset zeroes ~256 KB of internal hash-chain state.
/// Callgrind showed this memset consumed 53% of all instructions for a
/// 434 KB HTML→EPUB conversion (35 entries × 256 KB = 9 MB of zeroing).
/// Entries below this threshold use `Stored` — the typical compression
/// savings (< 1 KB for a 2 KB file) don't justify the 256 KB memset cost.
const MIN_DEFLATE_SIZE: usize = 4096;

/// Returns `true` if the XHTML content is already a proper XML document
/// (starts with `<?xml`). Such content can be used as-is without sanitization.
fn is_valid_xhtml_document(text: &str) -> bool {
    text.trim_start().as_bytes().starts_with(b"<?xml")
}

/// Extracts the inner content of the `<body>` element from an HTML document.
///
/// If no `<body>` is found, returns the entire input (it's a bare fragment).
/// Used to strip the MOBI reader's invalid `<html><head>...</head><body>` wrapper.
fn extract_body_content(html: &str) -> &str {
    let bytes = html.as_bytes();
    let len = bytes.len();

    // Find <body (case-insensitive) then the closing '>'.
    let body_start = {
        let mut found = None;
        for i in 0..len.saturating_sub(4) {
            if bytes[i] == b'<' && bytes[i + 1..i + 5].eq_ignore_ascii_case(b"body") {
                // Find the closing '>' of the <body> tag.
                if let Some(gt) = html[i..].find('>') {
                    found = Some(i + gt + 1);
                }
                break;
            }
        }
        found
    };

    let Some(start) = body_start else {
        return html;
    };

    // Find </body> (case-insensitive) from the end.
    let body_end = {
        let mut found = None;
        for i in (0..len.saturating_sub(6)).rev() {
            if bytes[i] == b'<'
                && bytes[i + 1] == b'/'
                && bytes[i + 2..i + 6].eq_ignore_ascii_case(b"body")
            {
                found = Some(i);
                break;
            }
        }
        found
    };

    let end = body_end.unwrap_or(len);
    &html[start..end]
}

/// Sanitizes HTML content to be valid inside an XHTML document.
///
/// Handles the following issues found in MOBI reader output:
/// - Strips MOBI namespace tags (`<mbp:…>`, `</mbp:…>`)
/// - Strips closing tags for void elements (`</br>`, `</hr>`, etc.)
/// - Converts void elements to self-closing form (`<br>` → `<br/>`)
/// - Quotes unquoted attribute values (`filepos=123` → `filepos="123"`)
/// - Escapes bare `&` not part of valid entities
///
/// **Limitation:** Tag boundary detection uses a simple `>` scan and does not
/// handle `>` characters inside quoted attribute values (e.g., `title="a > b"`).
/// This is acceptable for MOBI output which does not produce such patterns.
fn sanitize_html_for_xhtml(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 256);
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        if bytes[pos] == b'<' {
            // Find the end of this tag.
            let tag_end = match memchr::memchr(b'>', &bytes[pos..]) {
                Some(offset) => pos + offset + 1,
                None => {
                    // Malformed — no closing '>'. Output as escaped text.
                    out.push_str("&lt;");
                    pos += 1;
                    continue;
                },
            };

            let tag = &html[pos..tag_end];

            // Strip MOBI namespace tags (<mbp:…> and </mbp:…>).
            if is_mobi_ns_tag(tag) {
                pos = tag_end;
                continue;
            }

            // Strip structural HTML tags — our XHTML wrapper provides these.
            if is_structural_html_tag(tag) {
                pos = tag_end;
                continue;
            }

            // Handle HTML void elements (br, hr, img, etc.):
            // - Strip closing tags (</br>, </hr>, etc.)
            // - Convert opening tags to self-closing XHTML form (<br/>)
            if is_void_element_tag(tag) {
                if bytes[pos + 1] == b'/' {
                    // Closing tag for void element — strip it.
                    pos = tag_end;
                    continue;
                }
                // Opening void element — sanitize attrs and ensure self-closing.
                sanitize_void_element(&mut out, tag);
                pos = tag_end;
                continue;
            }

            // Fix unquoted attributes inside opening/self-closing tags.
            if tag.len() > 2
                && bytes[pos + 1] != b'/'
                && bytes[pos + 1] != b'!'
                && bytes[pos + 1] != b'?'
            {
                sanitize_tag_attrs(&mut out, tag);
            } else {
                out.push_str(tag);
            }
            pos = tag_end;
        } else if bytes[pos] == b'&' {
            // Escape bare '&' that is not a valid entity/character reference.
            if is_valid_entity_ref(html, pos) {
                let rest = &html[pos..];
                if let Some(semi) = rest.find(';') {
                    let inner = &rest[1..semi];
                    if inner.starts_with('#') || is_xml_builtin_entity(inner) {
                        // Numeric reference or XML built-in — pass through.
                        out.push_str(&rest[..semi + 1]);
                    } else if let Some(cp) = html_entity_to_codepoint(inner) {
                        // Known HTML named entity — convert to numeric ref.
                        write!(out, "&#{cp};").unwrap();
                    } else {
                        // Unknown named entity — escape the ampersand.
                        out.push_str("&amp;");
                        out.push_str(&rest[1..semi + 1]);
                    }
                    pos += semi + 1;
                } else {
                    out.push_str("&amp;");
                    pos += 1;
                }
            } else {
                out.push_str("&amp;");
                pos += 1;
            }
        } else {
            // Bulk-copy plain text until the next '<' or '&'.
            // This avoids per-character decode/re-encode overhead, which is
            // significant for CJK content (3-4 bytes per character).
            match memchr::memchr2(b'<', b'&', &bytes[pos..]) {
                Some(offset) if offset > 0 => {
                    out.push_str(&html[pos..pos + offset]);
                    pos += offset;
                },
                Some(_) => {
                    // offset == 0 shouldn't happen (would be caught above),
                    // but advance one char to avoid infinite loop.
                    let ch = html[pos..].chars().next().unwrap();
                    out.push(ch);
                    pos += ch.len_utf8();
                },
                None => {
                    out.push_str(&html[pos..]);
                    break;
                },
            }
        }
    }

    out
}

/// Returns `true` if the tag is a MOBI namespace tag (e.g., `<mbp:pagebreak>`).
fn is_mobi_ns_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    if bytes.len() < 5 {
        return false;
    }
    let start = if bytes[1] == b'/' { 2 } else { 1 };
    bytes
        .get(start..start + 4)
        .is_some_and(|b| b[..3].eq_ignore_ascii_case(b"mbp") && b[3] == b':')
}

/// Returns `true` if the tag is a structural HTML element that the XHTML
/// wrapper already provides (`html`, `head`, `body`, `guide`).
/// These are stripped during sanitization to avoid duplication.
fn is_structural_html_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let start = if bytes[1] == b'/' { 2 } else { 1 };
    let rest = &bytes[start..];

    // Match tag names: html, head, body, guide (case-insensitive).
    let name_end = rest
        .iter()
        .position(|&b| b == b' ' || b == b'>' || b == b'/')
        .unwrap_or(rest.len());
    let name = &rest[..name_end];

    if name.len() < 4 || name.len() > 5 {
        return false;
    }
    let mut lower = [0u8; 5];
    for (i, &b) in name.iter().enumerate() {
        lower[i] = b.to_ascii_lowercase();
    }
    let lower = &lower[..name.len()];
    lower == b"html" || lower == b"head" || lower == b"body" || lower == b"guide"
}

/// Returns `true` if the tag is for an HTML void element (e.g., `<br>`, `</hr>`,
/// `<img src="…">`). In XHTML, void elements must be self-closing and must not
/// have closing tags.
fn is_void_element_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let start = if bytes[1] == b'/' { 2 } else { 1 };
    let rest = &bytes[start..];
    let name_end = rest
        .iter()
        .position(|&b| b == b' ' || b == b'>' || b == b'/')
        .unwrap_or(rest.len());
    let name = &rest[..name_end];

    match name.len() {
        2 => {
            let a = name[0].to_ascii_lowercase();
            let b = name[1].to_ascii_lowercase();
            (a == b'b' || a == b'h') && b == b'r'
        },
        3 => {
            let mut l = [0u8; 3];
            for i in 0..3 {
                l[i] = name[i].to_ascii_lowercase();
            }
            l == *b"img" || l == *b"col" || l == *b"wbr"
        },
        4 => {
            let mut l = [0u8; 4];
            for i in 0..4 {
                l[i] = name[i].to_ascii_lowercase();
            }
            l == *b"area" || l == *b"base" || l == *b"meta" || l == *b"link"
        },
        5 => {
            let mut l = [0u8; 5];
            for i in 0..5 {
                l[i] = name[i].to_ascii_lowercase();
            }
            l == *b"input" || l == *b"embed" || l == *b"track"
        },
        6 => {
            let mut l = [0u8; 6];
            for i in 0..6 {
                l[i] = name[i].to_ascii_lowercase();
            }
            l == *b"source"
        },
        _ => false,
    }
}

/// Returns `true` if position `pos` in `html` starts a valid entity or
/// character reference (`&amp;`, `&lt;`, `&#123;`, `&#xAB;`, etc.).
fn is_valid_entity_ref(html: &str, pos: usize) -> bool {
    let rest = &html[pos..];
    if !rest.starts_with('&') || rest.len() < 3 {
        return false;
    }
    // Must have a ';' within a reasonable distance.
    let semi = match rest[1..].find(';') {
        Some(s) if s < 12 => s + 1, // offset from pos
        _ => return false,
    };
    let inner = &rest[1..semi];
    if let Some(digits) = inner.strip_prefix('#') {
        // Numeric: &#123; or &#xAB;
        if let Some(hex) = digits
            .strip_prefix('x')
            .or_else(|| digits.strip_prefix('X'))
        {
            !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
    } else {
        // Named: accept XML built-ins and known HTML entities so the caller
        // can decide how to handle them (convert or pass through).
        !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric())
    }
}

/// Returns `true` if `name` is one of the five XML predefined entities.
fn is_xml_builtin_entity(name: &str) -> bool {
    matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
}

/// Maps an HTML named entity to its Unicode code point.
///
/// Covers all HTML 4 character entity references plus common HTML5 additions
/// (248 entities). Returns `None` for unknown names.
fn html_entity_to_codepoint(name: &str) -> Option<u32> {
    Some(match name {
        // Latin-1 supplement (ISO 8859-1 characters)
        "nbsp" => 160,
        "iexcl" => 161,
        "cent" => 162,
        "pound" => 163,
        "curren" => 164,
        "yen" => 165,
        "brvbar" => 166,
        "sect" => 167,
        "uml" => 168,
        "copy" => 169,
        "ordf" => 170,
        "laquo" => 171,
        "not" => 172,
        "shy" => 173,
        "reg" => 174,
        "macr" => 175,
        "deg" => 176,
        "plusmn" => 177,
        "sup2" => 178,
        "sup3" => 179,
        "acute" => 180,
        "micro" => 181,
        "para" => 182,
        "middot" => 183,
        "cedil" => 184,
        "sup1" => 185,
        "ordm" => 186,
        "raquo" => 187,
        "frac14" => 188,
        "frac12" => 189,
        "frac34" => 190,
        "iquest" => 191,
        // Latin capital letters with diacritics
        "Agrave" => 192,
        "Aacute" => 193,
        "Acirc" => 194,
        "Atilde" => 195,
        "Auml" => 196,
        "Aring" => 197,
        "AElig" => 198,
        "Ccedil" => 199,
        "Egrave" => 200,
        "Eacute" => 201,
        "Ecirc" => 202,
        "Euml" => 203,
        "Igrave" => 204,
        "Iacute" => 205,
        "Icirc" => 206,
        "Iuml" => 207,
        "ETH" => 208,
        "Ntilde" => 209,
        "Ograve" => 210,
        "Oacute" => 211,
        "Ocirc" => 212,
        "Otilde" => 213,
        "Ouml" => 214,
        "times" => 215,
        "Oslash" => 216,
        "Ugrave" => 217,
        "Uacute" => 218,
        "Ucirc" => 219,
        "Uuml" => 220,
        "Yacute" => 221,
        "THORN" => 222,
        "szlig" => 223,
        // Latin small letters with diacritics
        "agrave" => 224,
        "aacute" => 225,
        "acirc" => 226,
        "atilde" => 227,
        "auml" => 228,
        "aring" => 229,
        "aelig" => 230,
        "ccedil" => 231,
        "egrave" => 232,
        "eacute" => 233,
        "ecirc" => 234,
        "euml" => 235,
        "igrave" => 236,
        "iacute" => 237,
        "icirc" => 238,
        "iuml" => 239,
        "eth" => 240,
        "ntilde" => 241,
        "ograve" => 242,
        "oacute" => 243,
        "ocirc" => 244,
        "otilde" => 245,
        "ouml" => 246,
        "divide" => 247,
        "oslash" => 248,
        "ugrave" => 249,
        "uacute" => 250,
        "ucirc" => 251,
        "uuml" => 252,
        "yacute" => 253,
        "thorn" => 254,
        "yuml" => 255,
        // Latin extended / special
        "OElig" => 338,
        "oelig" => 339,
        "Scaron" => 352,
        "scaron" => 353,
        "Yuml" => 376,
        "fnof" => 402,
        // Spacing modifier letters
        "circ" => 710,
        "tilde" => 732,
        // Greek capital letters
        "Alpha" => 913,
        "Beta" => 914,
        "Gamma" => 915,
        "Delta" => 916,
        "Epsilon" => 917,
        "Zeta" => 918,
        "Eta" => 919,
        "Theta" => 920,
        "Iota" => 921,
        "Kappa" => 922,
        "Lambda" => 923,
        "Mu" => 924,
        "Nu" => 925,
        "Xi" => 926,
        "Omicron" => 927,
        "Pi" => 928,
        "Rho" => 929,
        "Sigma" => 931,
        "Tau" => 932,
        "Upsilon" => 933,
        "Phi" => 934,
        "Chi" => 935,
        "Psi" => 936,
        "Omega" => 937,
        // Greek small letters
        "alpha" => 945,
        "beta" => 946,
        "gamma" => 947,
        "delta" => 948,
        "epsilon" => 949,
        "zeta" => 950,
        "eta" => 951,
        "theta" => 952,
        "iota" => 953,
        "kappa" => 954,
        "lambda" => 955,
        "mu" => 956,
        "nu" => 957,
        "xi" => 958,
        "omicron" => 959,
        "pi" => 960,
        "rho" => 961,
        "sigmaf" => 962,
        "sigma" => 963,
        "tau" => 964,
        "upsilon" => 965,
        "phi" => 966,
        "chi" => 967,
        "psi" => 968,
        "omega" => 969,
        "thetasym" => 977,
        "upsih" => 978,
        "piv" => 982,
        // General punctuation
        "ensp" => 8194,
        "emsp" => 8195,
        "thinsp" => 8201,
        "zwnj" => 8204,
        "zwj" => 8205,
        "lrm" => 8206,
        "rlm" => 8207,
        "ndash" => 8211,
        "mdash" => 8212,
        "lsquo" => 8216,
        "rsquo" => 8217,
        "sbquo" => 8218,
        "ldquo" => 8220,
        "rdquo" => 8221,
        "bdquo" => 8222,
        "dagger" => 8224,
        "Dagger" => 8225,
        "bull" => 8226,
        "hellip" => 8230,
        "permil" => 8240,
        "prime" => 8242,
        "Prime" => 8243,
        "lsaquo" => 8249,
        "rsaquo" => 8250,
        "oline" => 8254,
        "frasl" => 8260,
        // Currency
        "euro" => 8364,
        // Letter-like symbols
        "image" => 8465,
        "weierp" => 8472,
        "real" => 8476,
        "trade" => 8482,
        "alefsym" => 8501,
        // Arrows
        "larr" => 8592,
        "uarr" => 8593,
        "rarr" => 8594,
        "darr" => 8595,
        "harr" => 8596,
        "crarr" => 8629,
        "lArr" => 8656,
        "uArr" => 8657,
        "rArr" => 8658,
        "dArr" => 8659,
        "hArr" => 8660,
        // Mathematical operators
        "forall" => 8704,
        "part" => 8706,
        "exist" => 8707,
        "empty" => 8709,
        "nabla" => 8711,
        "isin" => 8712,
        "notin" => 8713,
        "ni" => 8715,
        "prod" => 8719,
        "sum" => 8721,
        "minus" => 8722,
        "lowast" => 8727,
        "radic" => 8730,
        "prop" => 8733,
        "infin" => 8734,
        "ang" => 8736,
        "and" => 8743,
        "or" => 8744,
        "cap" => 8745,
        "cup" => 8746,
        "int" => 8747,
        "there4" => 8756,
        "sim" => 8764,
        "cong" => 8773,
        "asymp" => 8776,
        "ne" => 8800,
        "equiv" => 8801,
        "le" => 8804,
        "ge" => 8805,
        "sub" => 8834,
        "sup" => 8835,
        "nsub" => 8836,
        "sube" => 8838,
        "supe" => 8839,
        "oplus" => 8853,
        "otimes" => 8855,
        "perp" => 8869,
        "sdot" => 8901,
        // Miscellaneous technical
        "lceil" => 8968,
        "rceil" => 8969,
        "lfloor" => 8970,
        "rfloor" => 8971,
        // HTML 4 values (U+2329/U+232A). HTML5 remaps these to U+27E8/U+27E9
        // but MOBI sources use HTML 4, and the code points are canonical equivalents.
        "lang" => 9001,
        "rang" => 9002,
        // Geometric shapes
        "loz" => 9674,
        // Miscellaneous symbols
        "spades" => 9824,
        "clubs" => 9827,
        "hearts" => 9829,
        "diams" => 9830,
        _ => return None,
    })
}

/// Writes a sanitized copy of an opening/self-closing HTML tag, quoting any
/// unquoted attribute values.
///
/// Example: `<reference type="toc" filepos=0002371959 />`
/// becomes: `<reference type="toc" filepos="0002371959" />`
fn sanitize_tag_attrs(out: &mut String, tag: &str) {
    let bytes = tag.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'=' && i + 1 < len {
            out.push('=');
            i += 1;
            // The next non-space character after '=' should be a quote.
            while i < len && bytes[i] == b' ' {
                out.push(' ');
                i += 1;
            }
            if i >= len {
                break;
            }
            if bytes[i] == b'"' || bytes[i] == b'\'' {
                // Already quoted — copy through to the matching close quote.
                let quote = bytes[i];
                let start = i;
                i += 1;
                while i < len && bytes[i] != quote {
                    i += 1;
                }
                if i < len {
                    i += 1; // include closing quote
                }
                out.push_str(&tag[start..i]);
            } else {
                // Unquoted value — collect until space, '>', or '/'.
                out.push('"');
                let start = i;
                while i < len && bytes[i] != b' ' && bytes[i] != b'>' && bytes[i] != b'/' {
                    i += 1;
                }
                out.push_str(&tag[start..i]);
                out.push('"');
            }
        } else {
            // Copy byte-by-byte for ASCII tag syntax (tag names, spaces, etc.)
            // Attribute values are handled above via push_str to preserve UTF-8.
            out.push(bytes[i] as char);
            i += 1;
        }
    }
}

/// Sanitizes an opening void element tag: quotes unquoted attributes and
/// ensures the tag is self-closing (e.g., `<br>` → `<br/>`,
/// `<img src=foo>` → `<img src="foo"/>`).
fn sanitize_void_element(out: &mut String, tag: &str) {
    let mark = out.len();
    sanitize_tag_attrs(out, tag);
    // Ensure the tag ends with `/>`.
    let written = &out[mark..];
    if written.ends_with("/>") {
        // Already self-closing — nothing to do.
    } else if written.ends_with('>') {
        let len = out.len();
        out.truncate(len - 1);
        out.push_str("/>");
    }
}

/// Wraps a bare HTML fragment in a full XHTML document envelope.
///
/// Produces valid XHTML matching the structure calibre emits:
/// ```xml
/// <?xml version="1.0" encoding="UTF-8"?>
/// <html xmlns="http://www.w3.org/1999/xhtml">
/// <head><title></title></head>
/// <body>
/// {content}
/// </body>
/// </html>
/// ```
fn wrap_xhtml(content: &str, lang: Option<&str>) -> String {
    let mut doc = String::with_capacity(content.len() + 256);
    doc.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"",
    );
    if let Some(l) = lang {
        doc.push_str(" xml:lang=\"");
        push_escape_xml(&mut doc, l);
        doc.push('"');
    }
    doc.push_str(">\n<head>\n<title></title>\n</head>\n<body>\n");
    doc.push_str(content);
    doc.push_str("\n</body>\n</html>\n");
    doc
}

/// Returns the XHTML bytes for a manifest item, sanitizing HTML content and
/// wrapping in a proper XHTML document when needed.
///
/// - Content starting with `<?xml` is assumed to be valid XHTML → zero-copy.
/// - Content with an `<html>` wrapper (e.g., from MOBI) → body extracted,
///   sanitized, and wrapped in proper XHTML.
/// - Bare HTML fragments → sanitized and wrapped.
fn xhtml_bytes<'a>(text: &'a str, lang: Option<&str>) -> Cow<'a, [u8]> {
    if is_valid_xhtml_document(text) {
        return Cow::Borrowed(text.as_bytes());
    }

    // Extract body content (strips MOBI's <html><head>…<body> wrapper).
    let body = extract_body_content(text);

    // Sanitize HTML for XHTML validity (quote attrs, strip MOBI tags, escape &).
    let sanitized = sanitize_html_for_xhtml(body);

    Cow::Owned(wrap_xhtml(&sanitized, lang).into_bytes())
}

/// Writes a `Book` as a valid EPUB archive to the given writer.
///
/// When there are enough deflatable entries, they are pre-compressed in
/// parallel using rayon (via per-entry mini-ZIP archives).  The raw
/// pre-compressed data is then copied into the final ZIP sequentially.
/// For small workloads the original sequential deflation path is used to
/// avoid rayon/mini-ZIP overhead.
pub(crate) fn write_epub<W: Write + Seek>(book: &Book, writer: W) -> Result<()> {
    let stored: FileOptions<'_, ()> =
        FileOptions::default().compression_method(CompressionMethod::Stored);
    #[cfg(feature = "parallel")]
    let deflated: FileOptions<'_, ()> = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(ZIP_DEFLATE_LEVEL);

    // Structural data generated once.
    let opf_xml = generate_opf(book);
    let ncx_xml = generate_ncx(book);

    // -----------------------------------------------------------------------
    // Decide whether to use the parallel path.
    // -----------------------------------------------------------------------
    #[cfg(feature = "parallel")]
    {
        // Count deflatable manifest entries and their total uncompressed size.
        const STRUCTURAL_HREFS: &[&str] = &["toc.ncx", "content.opf"];
        let mut deflate_count: usize = 0;
        let mut deflate_bytes: usize = 0;

        // Only count entries >= MIN_DEFLATE_SIZE — smaller entries will use Stored
        // to avoid the ~256 KB deflate state initialization cost.
        let container_len = generate_container_xml().len();
        if container_len >= MIN_DEFLATE_SIZE {
            deflate_count += 1;
            deflate_bytes += container_len;
        }
        if opf_xml.len() >= MIN_DEFLATE_SIZE {
            deflate_count += 1;
            deflate_bytes += opf_xml.len();
        }
        if ncx_xml.len() >= MIN_DEFLATE_SIZE {
            deflate_count += 1;
            deflate_bytes += ncx_xml.len();
        }

        for item in book.manifest.iter() {
            if STRUCTURAL_HREFS.contains(&item.href.as_str()) {
                continue;
            }
            if !is_already_compressed(&item.media_type) {
                let entry_size = match &item.data {
                    crate::domain::ManifestData::Text(t) => t.len(),
                    crate::domain::ManifestData::Inline(b) => b.len(),
                    crate::domain::ManifestData::Empty => 0,
                };
                if entry_size >= MIN_DEFLATE_SIZE {
                    deflate_count += 1;
                    deflate_bytes += entry_size;
                }
            }
        }

        // Use parallel path when there are enough entries (>= 8) and enough data
        // (>= 64 KiB) for rayon overhead to be worthwhile.  The per-entry mini-ZIP
        // approach adds ~50 us per entry overhead, plus ~100-200 us rayon thread-pool
        // cost, so we need substantial compression work to recoup that.
        let use_parallel = deflate_count >= 8 && deflate_bytes >= 65_536;
        if use_parallel {
            return write_epub_parallel(book, writer, stored, deflated, &opf_xml, &ncx_xml);
        }
    }

    let deflated: FileOptions<'_, ()> = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(ZIP_DEFLATE_LEVEL);

    write_epub_sequential(book, writer, stored, deflated, &opf_xml, &ncx_xml)
}

/// Sequential path: uses ZipWriter's built-in deflate for simplicity.
/// The per-entry ~256 KB deflate state init is acceptable here because the
/// sequential path only handles small workloads (< 8 deflatable entries).
fn write_epub_sequential<W: Write + Seek>(
    book: &Book,
    writer: W,
    stored: FileOptions<'_, ()>,
    deflated: FileOptions<'_, ()>,
    opf_xml: &str,
    ncx_xml: &str,
) -> Result<()> {
    let mut zip = ZipWriter::new(writer);

    // 1. mimetype
    zip.start_file("mimetype", stored)
        .map_err(|e| EruditioError::Format(format!("Failed to write mimetype: {}", e)))?;
    zip.write_all(b"application/epub+zip")?;

    // 2. container.xml — skip deflate for small entries.
    let container_xml = generate_container_xml();
    let container_opts = if container_xml.len() < MIN_DEFLATE_SIZE {
        stored
    } else {
        deflated
    };
    zip.start_file("META-INF/container.xml", container_opts)
        .map_err(|e| EruditioError::Format(format!("Failed to write container.xml: {}", e)))?;
    zip.write_all(container_xml.as_bytes())?;

    // 3. OPF
    let opf_opts = if opf_xml.len() < MIN_DEFLATE_SIZE {
        stored
    } else {
        deflated
    };
    zip.start_file("OEBPS/content.opf", opf_opts)
        .map_err(|e| EruditioError::Format(format!("Failed to write OPF: {}", e)))?;
    zip.write_all(opf_xml.as_bytes())?;

    // 4. NCX
    let ncx_opts = if ncx_xml.len() < MIN_DEFLATE_SIZE {
        stored
    } else {
        deflated
    };
    zip.start_file("OEBPS/toc.ncx", ncx_opts)
        .map_err(|e| EruditioError::Format(format!("Failed to write NCX: {}", e)))?;
    zip.write_all(ncx_xml.as_bytes())?;

    // 5. Manifest items
    const STRUCTURAL_HREFS: &[&str] = &["toc.ncx", "content.opf"];
    let lang = book.metadata.language.as_deref();
    let mut zip_path = String::with_capacity(64);
    for item in book.manifest.iter() {
        if STRUCTURAL_HREFS.contains(&item.href.as_str()) {
            continue;
        }
        zip_path.clear();
        zip_path.push_str("OEBPS/");
        zip_path.push_str(&item.href);

        // For XHTML items, ensure content is a full document (not a bare fragment).
        let is_xhtml = item.media_type == "application/xhtml+xml";
        let wrapped: Cow<'_, [u8]> = match &item.data {
            crate::domain::ManifestData::Text(t) => {
                if is_xhtml {
                    xhtml_bytes(t, lang)
                } else {
                    Cow::Borrowed(t.as_bytes())
                }
            },
            crate::domain::ManifestData::Inline(b) => Cow::Borrowed(b.as_ref()),
            crate::domain::ManifestData::Empty => Cow::Borrowed(&[]),
        };
        let entry_size = wrapped.len();
        let opts = if is_already_compressed(&item.media_type) || entry_size < MIN_DEFLATE_SIZE {
            stored
        } else {
            deflated
        };
        zip.start_file(&zip_path, opts)
            .map_err(|e| EruditioError::Format(format!("Failed to write {}: {}", zip_path, e)))?;
        zip.write_all(&wrapped)?;
    }

    zip.finish()
        .map_err(|e| EruditioError::Format(format!("Failed to finalize EPUB: {}", e)))?;
    Ok(())
}

/// Parallel path: pre-compress deflatable entries via rayon, then write raw
/// pre-compressed data into the final ZIP using `raw_copy_file_rename`.
#[cfg(feature = "parallel")]
fn write_epub_parallel<W: Write + Seek>(
    book: &Book,
    writer: W,
    stored: FileOptions<'_, ()>,
    _deflated: FileOptions<'_, ()>,
    opf_xml: &str,
    ncx_xml: &str,
) -> Result<()> {
    let mut zip = ZipWriter::new(writer);

    // 1. mimetype — must be first, uncompressed.
    zip.start_file("mimetype", stored)
        .map_err(|e| EruditioError::Format(format!("Failed to write mimetype: {}", e)))?;
    zip.write_all(b"application/epub+zip")?;

    // -----------------------------------------------------------------------
    // Collect entries for parallel compression.
    // -----------------------------------------------------------------------
    struct DeflateEntry<'a> {
        zip_path: String,
        data: Cow<'a, [u8]>,
    }

    struct StoredEntry<'a> {
        zip_path: String,
        data: Cow<'a, [u8]>,
    }

    let mut deflate_entries: Vec<DeflateEntry<'_>> = Vec::new();
    let mut stored_entries: Vec<StoredEntry<'_>> = Vec::new();

    // Structural entries — use Stored for small entries to skip deflate init.
    let container_xml = generate_container_xml();
    if container_xml.len() >= MIN_DEFLATE_SIZE {
        deflate_entries.push(DeflateEntry {
            zip_path: "META-INF/container.xml".to_string(),
            data: Cow::Borrowed(container_xml.as_bytes()),
        });
    } else {
        stored_entries.push(StoredEntry {
            zip_path: "META-INF/container.xml".to_string(),
            data: Cow::Borrowed(container_xml.as_bytes()),
        });
    }
    if opf_xml.len() >= MIN_DEFLATE_SIZE {
        deflate_entries.push(DeflateEntry {
            zip_path: "OEBPS/content.opf".to_string(),
            data: Cow::Borrowed(opf_xml.as_bytes()),
        });
    } else {
        stored_entries.push(StoredEntry {
            zip_path: "OEBPS/content.opf".to_string(),
            data: Cow::Borrowed(opf_xml.as_bytes()),
        });
    }
    if ncx_xml.len() >= MIN_DEFLATE_SIZE {
        deflate_entries.push(DeflateEntry {
            zip_path: "OEBPS/toc.ncx".to_string(),
            data: Cow::Borrowed(ncx_xml.as_bytes()),
        });
    } else {
        stored_entries.push(StoredEntry {
            zip_path: "OEBPS/toc.ncx".to_string(),
            data: Cow::Borrowed(ncx_xml.as_bytes()),
        });
    }

    // Manifest entries
    const STRUCTURAL_HREFS: &[&str] = &["toc.ncx", "content.opf"];
    let lang = book.metadata.language.as_deref();
    for item in book.manifest.iter() {
        if STRUCTURAL_HREFS.contains(&item.href.as_str()) {
            continue;
        }
        let mut zip_path = String::with_capacity(6 + item.href.len());
        zip_path.push_str("OEBPS/");
        zip_path.push_str(&item.href);
        let is_xhtml = item.media_type == "application/xhtml+xml";

        if is_already_compressed(&item.media_type) {
            match &item.data {
                crate::domain::ManifestData::Inline(bytes) => {
                    stored_entries.push(StoredEntry {
                        zip_path,
                        data: Cow::Borrowed(bytes),
                    });
                },
                crate::domain::ManifestData::Text(text) => {
                    stored_entries.push(StoredEntry {
                        zip_path,
                        data: Cow::Borrowed(text.as_bytes()),
                    });
                },
                crate::domain::ManifestData::Empty => {
                    stored_entries.push(StoredEntry {
                        zip_path,
                        data: Cow::Borrowed(&[]),
                    });
                },
            }
        } else {
            match &item.data {
                crate::domain::ManifestData::Text(text) => {
                    let effective = if is_xhtml {
                        xhtml_bytes(text, lang)
                    } else {
                        Cow::Borrowed(text.as_bytes())
                    };
                    if effective.len() < MIN_DEFLATE_SIZE {
                        stored_entries.push(StoredEntry {
                            zip_path,
                            data: effective,
                        });
                    } else {
                        deflate_entries.push(DeflateEntry {
                            zip_path,
                            data: effective,
                        });
                    }
                },
                crate::domain::ManifestData::Inline(bytes) => {
                    if bytes.len() < MIN_DEFLATE_SIZE {
                        stored_entries.push(StoredEntry {
                            zip_path,
                            data: Cow::Borrowed(bytes),
                        });
                    } else {
                        deflate_entries.push(DeflateEntry {
                            zip_path,
                            data: Cow::Borrowed(bytes),
                        });
                    }
                },
                crate::domain::ManifestData::Empty => {
                    stored_entries.push(StoredEntry {
                        zip_path,
                        data: Cow::Borrowed(&[]),
                    });
                },
            }
        }
    }

    // -----------------------------------------------------------------------
    // Parallel compression via direct flate2 + reusable compressor.
    //
    // Instead of creating a per-entry mini-ZIP (which allocates a new
    // deflate compressor and inflate decompressor each time), we:
    //   1. Pre-compress with a per-thread `flate2::Compress` (reused via reset)
    //   2. Build minimal ZIP bytes containing the pre-compressed data
    //   3. Open with ZipArchive and raw_copy_file_rename into the final ZIP
    // This eliminates ~66% of EPUB writer allocations (all per-entry
    // deflate::init and inflate::init calls).
    // -----------------------------------------------------------------------
    let level = ZIP_DEFLATE_LEVEL.unwrap_or(1) as u32;
    let mini_zips: Vec<std::result::Result<(String, Vec<u8>), EruditioError>> = deflate_entries
        .into_par_iter()
        .map_init(
            || {
                (
                    Compress::new(Compression::new(level), false),
                    Vec::with_capacity(8192),
                )
            },
            |(compressor, compress_buf), entry| {
                let crc = crc32fast::hash(&entry.data);
                let uncompressed_size = entry.data.len();

                // Compress the entry data, reusing the compressor state.
                compressor.reset();
                compress_buf.clear();
                let max_out =
                    uncompressed_size + (uncompressed_size >> 12) + (uncompressed_size >> 14) + 128;
                compress_buf.resize(max_out, 0);
                let status = compressor
                    .compress(&entry.data, compress_buf, FlushCompress::Finish)
                    .map_err(|e| EruditioError::Format(format!("deflate compress: {}", e)))?;
                if status != flate2::Status::StreamEnd {
                    return Err(EruditioError::Format(
                        "deflate did not complete in one pass".into(),
                    ));
                }
                let compressed_size = compressor.total_out() as usize;
                compress_buf.truncate(compressed_size);

                let compressed_u32 = u32::try_from(compressed_size).map_err(|_| {
                    EruditioError::Format("compressed entry exceeds ZIP32 4 GB limit".into())
                })?;
                let uncompressed_u32 = u32::try_from(uncompressed_size)
                    .map_err(|_| EruditioError::Format("entry exceeds ZIP32 4 GB limit".into()))?;
                let mini =
                    build_deflate_mini_zip(compress_buf, crc, compressed_u32, uncompressed_u32);

                Ok((entry.zip_path, mini))
            },
        )
        .collect();

    // Write pre-compressed entries via raw_copy_file_rename.
    for result in mini_zips {
        let (zip_path, mini_bytes) = result?;
        let cursor = Cursor::new(mini_bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| EruditioError::Format(format!("mini zip read: {}", e)))?;
        // Use by_index_raw to avoid allocating an inflate decompressor.
        let file = archive
            .by_index_raw(0)
            .map_err(|e| EruditioError::Format(format!("mini zip entry: {}", e)))?;
        zip.raw_copy_file_rename(file, &zip_path)
            .map_err(|e| EruditioError::Format(format!("Failed to write {}: {}", zip_path, e)))?;
    }

    // Write stored entries (binary data, no compression needed).
    for entry in &stored_entries {
        zip.start_file(&entry.zip_path, stored).map_err(|e| {
            EruditioError::Format(format!("Failed to write {}: {}", entry.zip_path, e))
        })?;
        zip.write_all(&entry.data)?;
    }

    zip.finish()
        .map_err(|e| EruditioError::Format(format!("Failed to finalize EPUB: {}", e)))?;
    Ok(())
}

/// Builds a minimal valid ZIP archive containing a single deflated entry named "e".
///
/// This avoids the overhead of creating a `ZipWriter` (which allocates a new
/// deflate compressor) and a `ZipArchive` reader (inflate state). The caller
/// pre-compresses with a reusable `flate2::Compress`.
#[cfg(feature = "parallel")]
fn build_deflate_mini_zip(
    compressed: &[u8],
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
) -> Vec<u8> {
    const FNAME: &[u8] = b"e"; // minimal filename
    const FNAME_LEN: u16 = 1;
    const LOCAL_HEADER_SIZE: usize = 30 + FNAME_LEN as usize; // 31
    const CENTRAL_HEADER_SIZE: usize = 46 + FNAME_LEN as usize; // 47
    const EOCD_SIZE: usize = 22;

    let total = LOCAL_HEADER_SIZE + compressed.len() + CENTRAL_HEADER_SIZE + EOCD_SIZE;
    let mut buf = Vec::with_capacity(total);

    // --- Local File Header ---
    buf.extend_from_slice(&0x04034b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
    buf.extend_from_slice(&0u16.to_le_bytes()); // GP flag
    buf.extend_from_slice(&8u16.to_le_bytes()); // compression = Deflated
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
    buf.extend_from_slice(&crc32.to_le_bytes());
    buf.extend_from_slice(&compressed_size.to_le_bytes());
    buf.extend_from_slice(&uncompressed_size.to_le_bytes());
    buf.extend_from_slice(&FNAME_LEN.to_le_bytes()); // filename length
    buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    buf.extend_from_slice(FNAME);

    // --- Compressed Data ---
    buf.extend_from_slice(compressed);

    // --- Central Directory Header ---
    let cd_offset = buf.len();
    buf.extend_from_slice(&0x02014b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
    buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
    buf.extend_from_slice(&0u16.to_le_bytes()); // GP flag
    buf.extend_from_slice(&8u16.to_le_bytes()); // compression = Deflated
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
    buf.extend_from_slice(&crc32.to_le_bytes());
    buf.extend_from_slice(&compressed_size.to_le_bytes());
    buf.extend_from_slice(&uncompressed_size.to_le_bytes());
    buf.extend_from_slice(&FNAME_LEN.to_le_bytes()); // filename length
    buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    buf.extend_from_slice(&0u16.to_le_bytes()); // comment length
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
    buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    buf.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    buf.extend_from_slice(&0u32.to_le_bytes()); // local header offset
    buf.extend_from_slice(FNAME);

    // --- End of Central Directory ---
    let cd_size = (buf.len() - cd_offset) as u32;
    buf.extend_from_slice(&0x06054b50u32.to_le_bytes()); // signature
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
    buf.extend_from_slice(&0u16.to_le_bytes()); // central dir start disk
    buf.extend_from_slice(&1u16.to_le_bytes()); // entries on this disk
    buf.extend_from_slice(&1u16.to_le_bytes()); // total entries
    buf.extend_from_slice(&cd_size.to_le_bytes());
    buf.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // comment length

    buf
}

fn generate_container_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#
}

/// Generates the OPF package document XML from a `Book`.
fn generate_opf(book: &Book) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');

    // Use the preserved OPF version from source, defaulting to "3.0".
    let opf_version = book
        .metadata
        .extended
        .get("opf:version")
        .map(|s| s.as_str())
        .unwrap_or("3.0");
    xml.push_str(r#"<package xmlns="http://www.idpf.org/2007/opf" version=""#);
    xml.push_str(opf_version);
    xml.push_str(r#"" unique-identifier="uid">"#);
    xml.push('\n');

    // Metadata
    generate_opf_metadata(book, &mut xml);

    // Manifest
    generate_opf_manifest(book, &mut xml);

    // Spine
    generate_opf_spine(book, &mut xml);

    // Guide
    generate_opf_guide(book, &mut xml);

    xml.push_str("</package>\n");
    xml
}

fn generate_opf_metadata(book: &Book, xml: &mut String) {
    let m = &book.metadata;
    xml.push_str(r#"  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">"#);
    xml.push('\n');

    if let Some(ref title) = m.title {
        xml.push_str("    <dc:title>");
        push_escape_xml(xml, title);
        xml.push_str("</dc:title>\n");
    }
    for (i, author) in m.authors.iter().enumerate() {
        if i == 0 {
            if let Some(ref sort) = m.author_sort {
                xml.push_str("    <dc:creator opf:file-as=\"");
                push_escape_xml(xml, sort);
                xml.push_str("\">");
            } else {
                xml.push_str("    <dc:creator>");
            }
        } else {
            xml.push_str("    <dc:creator>");
        }
        push_escape_xml(xml, author);
        xml.push_str("</dc:creator>\n");
    }
    if let Some(ref lang) = m.language {
        xml.push_str("    <dc:language>");
        push_escape_xml(xml, lang);
        xml.push_str("</dc:language>\n");
    } else {
        xml.push_str("    <dc:language>en</dc:language>\n");
    }
    if let Some(ref publisher) = m.publisher {
        xml.push_str("    <dc:publisher>");
        push_escape_xml(xml, publisher);
        xml.push_str("</dc:publisher>\n");
    }
    if let Some(ref identifier) = m.identifier {
        if let Some(ref scheme) = m.identifier_scheme {
            xml.push_str("    <dc:identifier id=\"uid\" opf:scheme=\"");
            push_escape_xml(xml, scheme);
            xml.push_str("\">");
        } else {
            xml.push_str("    <dc:identifier id=\"uid\">");
        }
        push_escape_xml(xml, identifier);
        xml.push_str("</dc:identifier>\n");
    } else {
        xml.push_str("    <dc:identifier id=\"uid\">urn:uuid:00000000-0000-0000-0000-000000000000</dc:identifier>\n");
    }
    if let Some(ref isbn) = m.isbn {
        xml.push_str("    <dc:identifier opf:scheme=\"ISBN\">");
        push_escape_xml(xml, isbn);
        xml.push_str("</dc:identifier>\n");
    }
    if let Some(ref desc) = m.description {
        xml.push_str("    <dc:description>");
        push_escape_xml(xml, desc);
        xml.push_str("</dc:description>\n");
    }
    for subject in &m.subjects {
        xml.push_str("    <dc:subject>");
        push_escape_xml(xml, subject);
        xml.push_str("</dc:subject>\n");
    }
    if let Some(ref rights) = m.rights {
        xml.push_str("    <dc:rights>");
        push_escape_xml(xml, rights);
        xml.push_str("</dc:rights>\n");
    }
    // Write dc:date elements: prefer roundtrip-preserved entries, fall back
    // to the parsed publication_date.
    if !m.additional_dates.is_empty() {
        for (event, value) in &m.additional_dates {
            if let Some(ev) = event {
                xml.push_str("    <dc:date opf:event=\"");
                push_escape_xml(xml, ev);
                xml.push_str("\">");
            } else {
                xml.push_str("    <dc:date>");
            }
            push_escape_xml(xml, value);
            xml.push_str("</dc:date>\n");
        }
    } else if let Some(ref date) = m.publication_date {
        xml.push_str("    <dc:date>");
        let _ = write!(xml, "{}", date.format("%Y-%m-%d"));
        xml.push_str("</dc:date>\n");
    }
    if let Some(ref cover_id) = m.cover_image_id {
        xml.push_str("    <meta name=\"cover\" content=\"");
        push_escape_xml(xml, cover_id);
        xml.push_str("\"/>\n");
    }
    if let Some(ref series) = m.series {
        xml.push_str("    <meta name=\"calibre:series\" content=\"");
        push_escape_xml(xml, series);
        xml.push_str("\"/>\n");
    }
    if let Some(idx) = m.series_index {
        xml.push_str("    <meta name=\"calibre:series_index\" content=\"");
        let _ = write!(xml, "{}", idx);
        xml.push_str("\"/>\n");
    }

    xml.push_str("  </metadata>\n");
}

fn generate_opf_manifest(book: &Book, xml: &mut String) {
    xml.push_str("  <manifest>\n");

    // NCX entry (always included for EPUB2 compat).
    xml.push_str(
        "    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
    );

    // All manifest items (skip NCX — already emitted above).
    for item in book.manifest.iter() {
        if item.href == "toc.ncx" || item.id == "ncx" {
            continue;
        }
        xml.push_str("    <item id=\"");
        push_escape_xml(xml, &item.id);
        xml.push_str("\" href=\"");
        push_escape_xml(xml, &item.href);
        xml.push_str("\" media-type=\"");
        push_escape_xml(xml, &item.media_type);
        xml.push('"');
        if !item.properties.is_empty() {
            xml.push_str(" properties=\"");
            for (i, prop) in item.properties.iter().enumerate() {
                if i > 0 {
                    xml.push(' ');
                }
                push_escape_xml(xml, prop);
            }
            xml.push('"');
        }
        xml.push_str("/>\n");
    }

    xml.push_str("  </manifest>\n");
}

fn generate_opf_spine(book: &Book, xml: &mut String) {
    xml.push_str("  <spine toc=\"ncx\"");
    if let Some(ppd) = &book.spine.page_progression_direction {
        let dir = match ppd {
            crate::domain::PageProgression::Ltr => "ltr",
            crate::domain::PageProgression::Rtl => "rtl",
        };
        xml.push_str(" page-progression-direction=\"");
        xml.push_str(dir);
        xml.push('"');
    }
    xml.push_str(">\n");

    for spine_item in book.spine.iter() {
        xml.push_str("    <itemref idref=\"");
        push_escape_xml(xml, &spine_item.manifest_id);
        xml.push('"');
        if !spine_item.linear {
            xml.push_str(" linear=\"no\"");
        }
        xml.push_str("/>\n");
    }

    xml.push_str("  </spine>\n");
}

fn generate_opf_guide(book: &Book, xml: &mut String) {
    if book.guide.is_empty() {
        return;
    }
    xml.push_str("  <guide>\n");
    for r in &book.guide.references {
        xml.push_str("    <reference type=\"");
        push_escape_xml(xml, r.ref_type.as_str());
        xml.push_str("\" title=\"");
        push_escape_xml(xml, &r.title);
        xml.push_str("\" href=\"");
        push_escape_xml(xml, &r.href);
        xml.push_str("\"/>\n");
    }
    xml.push_str("  </guide>\n");
}

/// Generates an NCX document from the book's TOC.
fn generate_ncx(book: &Book) -> String {
    let uid = book
        .metadata
        .identifier
        .as_deref()
        .unwrap_or("urn:uuid:00000000-0000-0000-0000-000000000000");
    let title = book.metadata.title.as_deref().unwrap_or("Untitled");

    let mut xml = String::with_capacity(2048);
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">"#);
    xml.push('\n');
    xml.push_str("  <head>\n");
    xml.push_str("    <meta name=\"dtb:uid\" content=\"");
    push_escape_xml(&mut xml, uid);
    xml.push_str("\"/>\n");
    xml.push_str("    <meta name=\"dtb:depth\" content=\"1\"/>\n");
    xml.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    xml.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    xml.push_str("  </head>\n");
    xml.push_str("  <docTitle><text>");
    push_escape_xml(&mut xml, title);
    xml.push_str("</text></docTitle>\n");
    xml.push_str("  <navMap>\n");

    let mut play_order = 1u32;
    for item in &book.toc {
        write_ncx_navpoint(item, &mut xml, &mut play_order, 2);
    }

    xml.push_str("  </navMap>\n");
    xml.push_str("</ncx>\n");
    xml
}

fn write_ncx_navpoint(item: &TocItem, xml: &mut String, play_order: &mut u32, indent: usize) {
    write_ncx_navpoint_depth(item, xml, play_order, indent, 0);
}

/// Maximum nesting depth for NCX nav-points, matching `MAX_TOC_DEPTH` in `domain::toc`.
const MAX_NCX_DEPTH: usize = 64;

fn write_ncx_navpoint_depth(
    item: &TocItem,
    xml: &mut String,
    play_order: &mut u32,
    indent: usize,
    depth: usize,
) {
    if depth >= MAX_NCX_DEPTH {
        return;
    }
    // Use a fixed indentation buffer to avoid "  ".repeat() allocation per call.
    const INDENT_BUF: &str = "                                ";
    let pad_len = (indent * 2).min(INDENT_BUF.len());
    let pad = &INDENT_BUF[..pad_len];

    let id: std::borrow::Cow<'_, str> = item
        .id
        .as_deref()
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| std::borrow::Cow::Owned(format!("navpoint-{}", *play_order)));

    xml.push_str(pad);
    xml.push_str("<navPoint id=\"");
    push_escape_xml(xml, &id);
    xml.push_str("\" playOrder=\"");
    let _ = write!(xml, "{}", *play_order);
    xml.push_str("\">\n");
    *play_order += 1;

    xml.push_str(pad);
    xml.push_str("  <navLabel><text>");
    push_escape_xml(xml, &item.title);
    xml.push_str("</text></navLabel>\n");

    xml.push_str(pad);
    xml.push_str("  <content src=\"");
    push_escape_xml(xml, &item.href);
    xml.push_str("\"/>\n");

    for child in &item.children {
        write_ncx_navpoint_depth(child, xml, play_order, indent + 1, depth + 1);
    }

    xml.push_str(pad);
    xml.push_str("</navPoint>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Book, Chapter, GuideReference, GuideType};
    use std::io::Cursor;

    fn sample_book() -> Book {
        let mut book = Book::new();
        book.metadata.title = Some("Test Book".into());
        book.metadata.authors.push("Test Author".into());
        book.metadata.language = Some("en".into());
        book.metadata.identifier = Some("urn:test:12345".into());

        book.add_chapter(Chapter {
            title: Some("Chapter 1".into()),
            content: "<p>Hello World</p>".into(),
            id: Some("ch1".into()),
        });
        book.add_chapter(Chapter {
            title: Some("Chapter 2".into()),
            content: "<p>Goodbye World</p>".into(),
            id: Some("ch2".into()),
        });

        book.add_resource(
            "cover",
            "images/cover.jpg",
            vec![0xFF, 0xD8, 0xFF],
            "image/jpeg",
        );

        book.guide.push(GuideReference {
            ref_type: GuideType::Cover,
            title: "Cover".into(),
            href: "ch1.xhtml".into(),
        });

        book
    }

    #[test]
    fn generates_valid_container_xml() {
        let xml = generate_container_xml();
        assert!(xml.contains("OEBPS/content.opf"));
        assert!(xml.contains("application/oebps-package+xml"));
    }

    #[test]
    fn generates_opf_with_metadata() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("<dc:title>Test Book</dc:title>"));
        assert!(opf.contains("<dc:creator>Test Author</dc:creator>"));
        assert!(opf.contains("<dc:language>en</dc:language>"));
    }

    #[test]
    fn generates_opf_with_isbn_identifier() {
        let mut book = sample_book();
        book.metadata.isbn = Some("978-3-16-148410-0".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:identifier opf:scheme="ISBN">978-3-16-148410-0</dc:identifier>"#)
        );
        // The primary identifier should still be present
        assert!(opf.contains(r#"<dc:identifier id="uid">urn:test:12345</dc:identifier>"#));
    }

    #[test]
    fn generates_opf_without_isbn_when_absent() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(!opf.contains("opf:scheme=\"ISBN\""));
    }

    #[test]
    fn generates_opf_manifest_and_spine() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("id=\"ch1\""));
        assert!(opf.contains("id=\"ch2\""));
        assert!(opf.contains("id=\"cover\""));
        assert!(opf.contains("idref=\"ch1\""));
        assert!(opf.contains("idref=\"ch2\""));
        assert!(opf.contains("toc=\"ncx\""));
    }

    #[test]
    fn generates_opf_guide() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(opf.contains("type=\"cover\""));
        assert!(opf.contains("title=\"Cover\""));
    }

    #[test]
    fn generates_ncx_with_toc() {
        let book = sample_book();
        let ncx = generate_ncx(&book);
        assert!(ncx.contains("Chapter 1"));
        assert!(ncx.contains("Chapter 2"));
        assert!(ncx.contains("playOrder=\"1\""));
        assert!(ncx.contains("playOrder=\"2\""));
        assert!(ncx.contains("urn:test:12345"));
    }

    #[test]
    fn write_epub_produces_valid_zip() {
        let book = sample_book();
        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        // Verify the ZIP is valid and contains expected files.
        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        assert!(archive.by_name("mimetype").is_ok());
        assert!(archive.by_name("META-INF/container.xml").is_ok());
        assert!(archive.by_name("OEBPS/content.opf").is_ok());
        assert!(archive.by_name("OEBPS/toc.ncx").is_ok());
        assert!(archive.by_name("OEBPS/ch1.xhtml").is_ok());
        assert!(archive.by_name("OEBPS/ch2.xhtml").is_ok());
        assert!(archive.by_name("OEBPS/images/cover.jpg").is_ok());
    }

    #[test]
    fn mimetype_is_uncompressed() {
        let book = sample_book();
        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();
        let mimetype = archive.by_name("mimetype").unwrap();
        assert_eq!(mimetype.compression(), CompressionMethod::Stored);
    }

    #[test]
    fn css_resources_included_in_epub_zip() {
        use crate::domain::ManifestItem;
        use std::io::Read as _;

        let mut book = sample_book();

        // Add CSS as binary data (via add_resource, the public API).
        book.add_resource(
            "stylesheet",
            "styles/stylesheet.css",
            b"body { margin: 0; }".to_vec(),
            "text/css",
        );

        // Also add CSS as ManifestData::Text (the way the EPUB reader loads it).
        let css_item = ManifestItem::new("extra-css", "styles/extra.css", "text/css")
            .with_text("h1 { color: red; }");
        book.manifest.insert(css_item);

        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        // Verify both CSS files exist in the ZIP at the correct paths.
        {
            let mut css_file = archive
                .by_name("OEBPS/styles/stylesheet.css")
                .expect("CSS file (Inline) missing from EPUB ZIP");
            let mut contents = Vec::new();
            css_file.read_to_end(&mut contents).unwrap();
            assert_eq!(contents, b"body { margin: 0; }");
        }
        {
            let mut css_file = archive
                .by_name("OEBPS/styles/extra.css")
                .expect("CSS file (Text) missing from EPUB ZIP");
            let mut contents = String::new();
            css_file.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, "h1 { color: red; }");
        }
    }

    #[test]
    fn css_items_appear_in_opf_manifest() {
        use crate::domain::ManifestItem;

        let mut book = sample_book();

        book.add_resource(
            "stylesheet",
            "styles/stylesheet.css",
            b"body { margin: 0; }".to_vec(),
            "text/css",
        );

        let css_item = ManifestItem::new("extra-css", "styles/extra.css", "text/css")
            .with_text("h1 { color: red; }");
        book.manifest.insert(css_item);

        let opf = generate_opf(&book);

        // Both CSS items must be listed in the OPF <manifest> with correct media-type.
        assert!(
            opf.contains(r#"id="stylesheet" href="styles/stylesheet.css" media-type="text/css""#),
            "OPF manifest missing stylesheet item"
        );
        assert!(
            opf.contains(r#"id="extra-css" href="styles/extra.css" media-type="text/css""#),
            "OPF manifest missing extra-css item"
        );
    }

    #[test]
    fn ncx_respects_depth_limit() {
        // Build a deeply nested TOC (deeper than MAX_NCX_DEPTH) and ensure
        // the writer terminates without panicking or infinite recursion.
        let mut item = TocItem {
            title: "Leaf".into(),
            href: "leaf.xhtml".into(),
            id: Some("leaf".into()),
            children: Vec::new(),
            play_order: None,
        };
        // Nest 100 levels deep (well beyond MAX_NCX_DEPTH = 64).
        for i in (0..100).rev() {
            item = TocItem {
                title: format!("Level {}", i),
                href: format!("ch{}.xhtml", i),
                id: Some(format!("nav-{}", i)),
                children: vec![item],
                play_order: None,
            };
        }

        let mut xml = String::new();
        let mut play_order = 1u32;
        write_ncx_navpoint(&item, &mut xml, &mut play_order, 0);

        // Should contain the top-level navPoint but stop well before level 100.
        assert!(xml.contains("Level 0"));
        // play_order should not reach 100, meaning recursion was cut off.
        assert!(
            (play_order as usize) <= MAX_NCX_DEPTH + 1,
            "play_order {} exceeds depth limit",
            play_order
        );
    }

    #[test]
    fn generates_opf_with_author_sort_file_as() {
        let mut book = sample_book();
        book.metadata.author_sort = Some("Author, Test".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:creator opf:file-as="Author, Test">Test Author</dc:creator>"#),
            "First author should have opf:file-as attribute. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_without_file_as_when_author_sort_absent() {
        let book = sample_book();
        assert!(book.metadata.author_sort.is_none());
        let opf = generate_opf(&book);
        assert!(
            opf.contains("<dc:creator>Test Author</dc:creator>"),
            "Creator should have no opf:file-as when author_sort is None. Got:\n{}",
            opf
        );
        assert!(
            !opf.contains("opf:file-as"),
            "opf:file-as should not appear when author_sort is None. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_file_as_only_on_first_author() {
        let mut book = sample_book();
        book.metadata.authors.push("Second Author".into());
        book.metadata.author_sort = Some("Author, Test".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:creator opf:file-as="Author, Test">Test Author</dc:creator>"#),
            "First author should have opf:file-as. Got:\n{}",
            opf
        );
        assert!(
            opf.contains("<dc:creator>Second Author</dc:creator>"),
            "Second author should not have opf:file-as. Got:\n{}",
            opf
        );
    }

    #[test]
    fn author_sort_round_trips_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata.author_sort = Some("Author, Test".into());

        // Generate OPF XML from the book
        let opf_xml = generate_opf(&book);

        // Parse the generated OPF XML back
        let data = parse_opf_xml(&opf_xml).unwrap();

        assert_eq!(data.metadata.authors, vec!["Test Author"]);
        assert_eq!(
            data.metadata.author_sort.as_deref(),
            Some("Author, Test"),
            "author_sort should survive OPF round-trip"
        );
    }

    #[test]
    fn opf_version_defaults_to_3() {
        let book = sample_book();
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"version="3.0""#),
            "Default OPF version should be 3.0 when no source version is set. Got:\n{}",
            opf
        );
    }

    #[test]
    fn opf_version_preserves_2_0() {
        let mut book = sample_book();
        book.metadata
            .extended
            .insert("opf:version".into(), "2.0".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"version="2.0""#),
            "OPF version should be preserved as 2.0 from source. Got:\n{}",
            opf
        );
        assert!(
            !opf.contains(r#"version="3.0""#),
            "OPF version should NOT be 3.0 when source was 2.0. Got:\n{}",
            opf
        );
    }

    #[test]
    fn opf_version_round_trips_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata
            .extended
            .insert("opf:version".into(), "2.0".into());

        let opf_xml = generate_opf(&book);
        let data = parse_opf_xml(&opf_xml).unwrap();

        assert_eq!(
            data.metadata
                .extended
                .get("opf:version")
                .map(|s| s.as_str()),
            Some("2.0"),
            "OPF version 2.0 should survive round-trip"
        );
    }

    #[test]
    fn multiple_dates_round_trip_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata.additional_dates = vec![
            (Some("publication".into()), "2008-06-27".into()),
            (
                Some("conversion".into()),
                "2026-03-01T08:32:03.786809+00:00".into(),
            ),
        ];

        let opf_xml = generate_opf(&book);
        assert!(
            opf_xml.contains(r#"opf:event="publication">2008-06-27</dc:date>"#),
            "Publication date should appear in output. Got:\n{}",
            opf_xml
        );
        assert!(
            opf_xml
                .contains(r#"opf:event="conversion">2026-03-01T08:32:03.786809+00:00</dc:date>"#),
            "Conversion date should appear in output. Got:\n{}",
            opf_xml
        );

        // Parse back and verify both dates survived.
        let data = parse_opf_xml(&opf_xml).unwrap();
        assert_eq!(
            data.metadata.additional_dates.len(),
            2,
            "Both dates should survive round-trip"
        );
        assert_eq!(data.metadata.additional_dates[0].1, "2008-06-27");
        assert_eq!(
            data.metadata.additional_dates[1].1,
            "2026-03-01T08:32:03.786809+00:00"
        );
    }

    #[test]
    fn single_date_without_additional_dates_still_emitted() {
        let mut book = sample_book();
        book.metadata.publication_date = Some(
            chrono::NaiveDate::from_ymd_opt(2024, 3, 15)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        );
        // additional_dates is empty; the writer should fall back to publication_date
        let opf = generate_opf(&book);
        assert!(
            opf.contains("<dc:date>2024-03-15</dc:date>"),
            "publication_date should be emitted when additional_dates is empty. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_with_identifier_scheme() {
        let mut book = sample_book();
        book.metadata.identifier_scheme = Some("URI".into());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(
                r#"<dc:identifier id="uid" opf:scheme="URI">urn:test:12345</dc:identifier>"#
            ),
            "Primary identifier should have opf:scheme attribute. Got:\n{}",
            opf
        );
    }

    #[test]
    fn generates_opf_without_identifier_scheme_when_absent() {
        let book = sample_book();
        assert!(book.metadata.identifier_scheme.is_none());
        let opf = generate_opf(&book);
        assert!(
            opf.contains(r#"<dc:identifier id="uid">urn:test:12345</dc:identifier>"#),
            "Identifier should have no opf:scheme when identifier_scheme is None. Got:\n{}",
            opf
        );
    }

    #[test]
    fn identifier_scheme_round_trips_through_opf() {
        use crate::formats::epub::opf::parse_opf_xml;

        let mut book = sample_book();
        book.metadata.identifier_scheme = Some("URI".into());

        // Generate OPF XML from the book
        let opf_xml = generate_opf(&book);

        // Parse the generated OPF XML back
        let data = parse_opf_xml(&opf_xml).unwrap();

        assert_eq!(data.metadata.identifier.as_deref(), Some("urn:test:12345"),);
        assert_eq!(
            data.metadata.identifier_scheme.as_deref(),
            Some("URI"),
            "identifier_scheme should survive OPF round-trip"
        );
    }

    #[test]
    fn xhtml_wrapping_for_bare_fragments() {
        // Chapters created via add_chapter() store bare HTML fragments.
        // The EPUB writer must wrap them in full XHTML documents.
        let book = sample_book(); // chapters have content like "<p>Hello World</p>"
        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        for name in ["OEBPS/ch1.xhtml", "OEBPS/ch2.xhtml"] {
            let mut content = String::new();
            {
                use std::io::Read as _;
                archive
                    .by_name(name)
                    .unwrap()
                    .read_to_string(&mut content)
                    .unwrap();
            }

            let trimmed = content.trim_start();
            assert!(
                trimmed.starts_with("<?xml"),
                "{} must start with XML declaration, got: {:?}",
                name,
                &trimmed[..trimmed.len().min(60)]
            );
            assert!(
                content.contains("<html"),
                "{} must contain <html> element",
                name
            );
            assert!(
                content.contains("<body>"),
                "{} must contain <body> element",
                name
            );
            assert!(content.contains("</body>"), "{} must contain </body>", name);
            assert!(content.contains("</html>"), "{} must contain </html>", name);
        }
    }

    #[test]
    fn xhtml_wrapping_preserves_existing_documents() {
        // When content is already a full XHTML document (e.g., EPUB→EPUB),
        // the writer must NOT double-wrap it.
        let full_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Test</title></head>
<body><p>Hello</p></body>
</html>"#;

        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: full_xhtml.to_string(),
            id: Some("ch1".into()),
        });

        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        let mut content = String::new();
        {
            use std::io::Read as _;
            archive
                .by_name("OEBPS/ch1.xhtml")
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        // Should NOT be double-wrapped: only one <?xml declaration.
        let xml_count = content.matches("<?xml").count();
        assert_eq!(
            xml_count,
            1,
            "Should have exactly 1 XML declaration, got {}. Content:\n{}",
            xml_count,
            &content[..content.len().min(300)]
        );
    }

    #[test]
    fn xhtml_wrapping_includes_language() {
        let mut book = Book::new();
        book.metadata.title = Some("Test".into());
        book.metadata.language = Some("ja".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>こんにちは</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();

        let mut content = String::new();
        {
            use std::io::Read as _;
            archive
                .by_name("OEBPS/ch1.xhtml")
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        assert!(
            content.contains("xml:lang=\"ja\""),
            "XHTML wrapper should include xml:lang. Content:\n{}",
            &content[..content.len().min(300)]
        );
        assert!(
            content.contains("こんにちは"),
            "CJK content must be preserved in wrapped XHTML"
        );
    }

    #[test]
    fn valid_xhtml_passes_through() {
        assert!(is_valid_xhtml_document(
            r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><body></body></html>"#
        ));
        assert!(!is_valid_xhtml_document(
            r#"<html><head></head><body></body></html>"#
        ));
        assert!(!is_valid_xhtml_document("<p>fragment</p>"));
        assert!(!is_valid_xhtml_document(""));
    }

    #[test]
    fn extract_body_content_strips_html_wrapper() {
        let html = r#"<html><head><guide></guide></head><body><p>Hello</p></body></html>"#;
        assert_eq!(extract_body_content(html), "<p>Hello</p>");
    }

    #[test]
    fn extract_body_content_returns_fragment_as_is() {
        let frag = "<p>Hello</p>";
        assert_eq!(extract_body_content(frag), frag);
    }

    #[test]
    fn sanitize_quotes_unquoted_attributes() {
        let html = r#"<reference type="toc" filepos=0002371959 />"#;
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains(r#"filepos="0002371959""#),
            "Unquoted attr should be quoted. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_strips_mobi_namespace_tags() {
        let html = r#"</mbp:pagebreak><p>Content</p><mbp:nu/>"#;
        let result = sanitize_html_for_xhtml(html);
        assert!(
            !result.contains("mbp:"),
            "MOBI tags should be stripped. Got: {}",
            result
        );
        assert!(result.contains("<p>Content</p>"));
    }

    #[test]
    fn sanitize_escapes_bare_ampersand() {
        let html = "<p>A & B</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("A &amp; B"),
            "Bare & should be escaped. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_preserves_valid_entities() {
        let html = "<p>&amp; &lt; &#x4F60; &#123;</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(result.contains("&amp;"), "Got: {}", result);
        assert!(result.contains("&lt;"), "Got: {}", result);
        assert!(result.contains("&#x4F60;"), "Got: {}", result);
        assert!(result.contains("&#123;"), "Got: {}", result);
    }

    #[test]
    fn xhtml_bytes_sanitizes_mobi_html() {
        // Simulate MOBI reader output with invalid HTML
        let mobi_html = r#"<html><head><guide><reference type="toc" title="TOC" filepos=0002371959 /></guide></head><body><p height="6em" width="0pt">Hello</p></body></html>"#;
        let result = xhtml_bytes(mobi_html, Some("zh"));
        let text = std::str::from_utf8(&result).unwrap();
        assert!(
            text.starts_with("<?xml"),
            "Should be wrapped in XHTML: {:?}",
            &text[..80.min(text.len())]
        );
        assert!(
            !text.contains("filepos=0002371959"),
            "Unquoted attr should be fixed"
        );
        assert!(text.contains("Hello"), "Content should be preserved");
        assert!(!text.contains("<guide>"), "MOBI guide should be stripped");
    }

    // ── HTML entity → numeric reference conversion tests ──────────────

    #[test]
    fn is_xml_builtin_entity_accepts_builtins() {
        assert!(is_xml_builtin_entity("amp"));
        assert!(is_xml_builtin_entity("lt"));
        assert!(is_xml_builtin_entity("gt"));
        assert!(is_xml_builtin_entity("quot"));
        assert!(is_xml_builtin_entity("apos"));
    }

    #[test]
    fn is_xml_builtin_entity_rejects_html_entities() {
        assert!(!is_xml_builtin_entity("nbsp"));
        assert!(!is_xml_builtin_entity("mdash"));
        assert!(!is_xml_builtin_entity("copy"));
        assert!(!is_xml_builtin_entity("eacute"));
        assert!(!is_xml_builtin_entity(""));
    }

    #[test]
    fn html_entity_codepoint_common_entities() {
        assert_eq!(html_entity_to_codepoint("nbsp"), Some(160));
        assert_eq!(html_entity_to_codepoint("mdash"), Some(8212));
        assert_eq!(html_entity_to_codepoint("ndash"), Some(8211));
        assert_eq!(html_entity_to_codepoint("ldquo"), Some(8220));
        assert_eq!(html_entity_to_codepoint("rdquo"), Some(8221));
        assert_eq!(html_entity_to_codepoint("lsquo"), Some(8216));
        assert_eq!(html_entity_to_codepoint("rsquo"), Some(8217));
        assert_eq!(html_entity_to_codepoint("hellip"), Some(8230));
        assert_eq!(html_entity_to_codepoint("copy"), Some(169));
        assert_eq!(html_entity_to_codepoint("reg"), Some(174));
        assert_eq!(html_entity_to_codepoint("trade"), Some(8482));
        assert_eq!(html_entity_to_codepoint("euro"), Some(8364));
    }

    #[test]
    fn html_entity_codepoint_accented_chars() {
        assert_eq!(html_entity_to_codepoint("eacute"), Some(233));
        assert_eq!(html_entity_to_codepoint("Eacute"), Some(201));
        assert_eq!(html_entity_to_codepoint("agrave"), Some(224));
        assert_eq!(html_entity_to_codepoint("ntilde"), Some(241));
        assert_eq!(html_entity_to_codepoint("ouml"), Some(246));
        assert_eq!(html_entity_to_codepoint("ccedil"), Some(231));
    }

    #[test]
    fn html_entity_codepoint_greek() {
        assert_eq!(html_entity_to_codepoint("alpha"), Some(945));
        assert_eq!(html_entity_to_codepoint("Alpha"), Some(913));
        assert_eq!(html_entity_to_codepoint("omega"), Some(969));
        assert_eq!(html_entity_to_codepoint("Omega"), Some(937));
        assert_eq!(html_entity_to_codepoint("pi"), Some(960));
    }

    #[test]
    fn html_entity_codepoint_unknown_returns_none() {
        assert_eq!(html_entity_to_codepoint("notarealentity"), None);
        assert_eq!(html_entity_to_codepoint(""), None);
        assert_eq!(html_entity_to_codepoint("NBSP"), None); // case-sensitive
    }

    #[test]
    fn sanitize_converts_nbsp_to_numeric() {
        let html = "<p>Hello&nbsp;World</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&#160;"),
            "&nbsp; should become &#160;. Got: {}",
            result
        );
        assert!(
            !result.contains("&nbsp;"),
            "&nbsp; should not remain in output. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_converts_mdash_to_numeric() {
        let html = "<p>one&mdash;two</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&#8212;"),
            "&mdash; should become &#8212;. Got: {}",
            result
        );
        assert!(
            !result.contains("&mdash;"),
            "&mdash; should not remain in output. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_converts_multiple_html_entities() {
        let html = "<p>&ldquo;Hello&rdquo; &mdash; &copy; 2024</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(result.contains("&#8220;"), "ldquo. Got: {}", result);
        assert!(result.contains("&#8221;"), "rdquo. Got: {}", result);
        assert!(result.contains("&#8212;"), "mdash. Got: {}", result);
        assert!(result.contains("&#169;"), "copy. Got: {}", result);
        assert!(!result.contains("&ldquo;"), "Got: {}", result);
        assert!(!result.contains("&rdquo;"), "Got: {}", result);
        assert!(!result.contains("&mdash;"), "Got: {}", result);
        assert!(!result.contains("&copy;"), "Got: {}", result);
    }

    #[test]
    fn sanitize_preserves_xml_builtin_entities() {
        let html = "<p>&amp; &lt; &gt; &quot; &apos;</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(result.contains("&amp;"), "Got: {}", result);
        assert!(result.contains("&lt;"), "Got: {}", result);
        assert!(result.contains("&gt;"), "Got: {}", result);
        assert!(result.contains("&quot;"), "Got: {}", result);
        assert!(result.contains("&apos;"), "Got: {}", result);
    }

    #[test]
    fn sanitize_preserves_numeric_entities() {
        let html = "<p>&#160; &#x4F60; &#xA0;</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(result.contains("&#160;"), "Got: {}", result);
        assert!(result.contains("&#x4F60;"), "Got: {}", result);
        assert!(result.contains("&#xA0;"), "Got: {}", result);
    }

    #[test]
    fn sanitize_escapes_unknown_named_entities() {
        // An entity name that isn't in our lookup table should have its & escaped.
        let html = "<p>&notarealentity;</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&amp;notarealentity;"),
            "Unknown entity should be escaped. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_handles_mixed_entities_and_bare_ampersands() {
        let html = "<p>A & B &amp; C &nbsp; D &mdash; E &#8226; F</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(result.contains("A &amp; B"), "bare &. Got: {}", result);
        assert!(result.contains("&amp;"), "xml builtin. Got: {}", result);
        assert!(result.contains("&#160;"), "nbsp. Got: {}", result);
        assert!(result.contains("&#8212;"), "mdash. Got: {}", result);
        assert!(result.contains("&#8226;"), "numeric. Got: {}", result);
    }

    #[test]
    fn sanitize_converts_accented_entities() {
        let html = "<p>caf&eacute; na&iuml;ve</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&#233;"),
            "eacute should become &#233;. Got: {}",
            result
        );
        assert!(
            result.contains("&#239;"),
            "iuml should become &#239;. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_escapes_entity_without_semicolon() {
        // A trailing entity-like pattern with no ';' should have its '&' escaped.
        let html = "<p>text &nbsp</p>";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&amp;nbsp"),
            "Entity without semicolon should have bare & escaped. Got: {}",
            result
        );
        assert!(
            !result.contains("&#160;"),
            "Should not convert entity without semicolon. Got: {}",
            result
        );
    }

    #[test]
    fn sanitize_entity_at_end_of_input_without_semicolon() {
        let html = "text &nbsp";
        let result = sanitize_html_for_xhtml(html);
        assert!(
            result.contains("&amp;"),
            "Trailing entity without ';' should be escaped. Got: {}",
            result
        );
    }

    #[test]
    fn xhtml_bytes_converts_html_entities_in_mobi() {
        // Simulate MOBI reader output that contains &nbsp; and &mdash;
        let mobi_html = "<html><body><p>Hello&nbsp;World &mdash; done</p></body></html>";
        let result = xhtml_bytes(mobi_html, Some("en"));
        let text = std::str::from_utf8(&result).unwrap();
        assert!(
            text.contains("&#160;"),
            "&nbsp; should be converted. Got: {}",
            &text[..text.len().min(400)]
        );
        assert!(
            text.contains("&#8212;"),
            "&mdash; should be converted. Got: {}",
            &text[..text.len().min(400)]
        );
        assert!(
            !text.contains("&nbsp;"),
            "&nbsp; should not remain. Got: {}",
            &text[..text.len().min(400)]
        );
    }

    #[test]
    fn epub_with_nbsp_produces_parseable_xhtml() {
        // End-to-end: a book with &nbsp; in chapter content should produce
        // XHTML that an XML parser can handle without "entity not defined".
        let mut book = Book::new();
        book.metadata.title = Some("Entity Test".into());
        book.metadata.language = Some("en".into());
        book.add_chapter(Chapter {
            title: Some("Ch1".into()),
            content: "<p>Hello&nbsp;World &mdash; &ldquo;test&rdquo;</p>".into(),
            id: Some("ch1".into()),
        });

        let mut output = Cursor::new(Vec::new());
        write_epub(&book, &mut output).unwrap();

        output.set_position(0);
        let mut archive = zip::ZipArchive::new(output).unwrap();
        let mut content = String::new();
        {
            use std::io::Read as _;
            archive
                .by_name("OEBPS/ch1.xhtml")
                .unwrap()
                .read_to_string(&mut content)
                .unwrap();
        }

        // Must not contain any HTML-only named entities.
        assert!(!content.contains("&nbsp;"), "Got: {}", content);
        assert!(!content.contains("&mdash;"), "Got: {}", content);
        assert!(!content.contains("&ldquo;"), "Got: {}", content);
        assert!(!content.contains("&rdquo;"), "Got: {}", content);

        // Must contain the numeric equivalents.
        assert!(content.contains("&#160;"), "Got: {}", content);
        assert!(content.contains("&#8212;"), "Got: {}", content);
        assert!(content.contains("&#8220;"), "Got: {}", content);
        assert!(content.contains("&#8221;"), "Got: {}", content);

        // Verify it parses as valid XML.
        assert!(
            content.starts_with("<?xml"),
            "Should be valid XHTML document"
        );
    }
}
