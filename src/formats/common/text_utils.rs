//! Shared text processing utilities optimized with `memchr` for SIMD-accelerated
//! byte scanning. Consolidates duplicate escape, strip, decode functions from
//! format-specific modules.

use std::borrow::Cow;

use memchr::{memchr, memchr2};

// ---------------------------------------------------------------------------
// HTML / XML escaping (single-pass, one allocation)
// ---------------------------------------------------------------------------

/// Escapes `&`, `<`, `>` for safe embedding in HTML body content.
///
/// Single-pass implementation using `memchr` to skip over runs of safe bytes
/// in bulk, replacing the chained `.replace()` pattern (which allocates 3
/// intermediate `String`s) with a single allocation.
///
/// Returns `Cow::Borrowed` when the input contains no special characters,
/// avoiding allocation entirely in the common case.
pub fn escape_html(text: &str) -> Cow<'_, str> {
    escape_impl(text, false)
}

/// Escapes `&`, `<`, `>`, `"`, `'` for safe embedding in XML/HTML attributes.
///
/// Returns `Cow::Borrowed` when the input contains no special characters.
pub fn escape_xml(text: &str) -> Cow<'_, str> {
    escape_impl(text, true)
}


fn escape_impl(text: &str, xml_mode: bool) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let len = bytes.len();

    let set: &[u8] = if xml_mode { b"&<>\"'" } else { b"&<>" };

    // Fast path: no special characters found -- zero allocation.
    let first_special = super::intrinsics::byte_scan::find_first_in_set(bytes, set);
    let first_special = match first_special {
        Some(idx) => idx,
        None => return Cow::Borrowed(text),
    };

    // Estimate capacity: each special char expands by at most 4 bytes (& -> &amp;).
    // For sparse input the default is fine; for dense input we need much more.
    // Sample the first 256 bytes to estimate density.
    let sample_end = len.min(256);
    let mut sample_count = 0u32;
    for &b in &bytes[..sample_end] {
        sample_count += is_html_special(b, xml_mode) as u32;
    }
    let estimated_extra = if sample_end > 0 {
        // Average expansion ~4 bytes per special char, scaled to full length.
        ((sample_count as usize) * 4 * len).div_ceil(sample_end)
    } else {
        len / 8
    };
    let mut result = String::with_capacity(len + estimated_extra);

    // Copy everything before the first special char in bulk.
    if first_special > 0 {
        result.push_str(&text[..first_special]);
    }

    let mut pos = first_special;

    // Process the first (already-found) special char, then enter the main loop.
    loop {
        // At this point, bytes[pos] is a special character. Process a run of
        // special chars with a tight scalar loop to avoid SIMD dispatch overhead
        // when specials are clustered.
        while pos < len && is_html_special(bytes[pos], xml_mode) {
            push_escape_byte(&mut result, bytes[pos]);
            pos += 1;
        }

        if pos >= len {
            break;
        }

        // Now bytes[pos] is safe. Use SIMD to find the next special char.
        match super::intrinsics::byte_scan::find_first_in_set(&bytes[pos..], set) {
            Some(offset) => {
                // Copy safe prefix in bulk.
                result.push_str(&text[pos..pos + offset]);
                pos += offset;
                // Loop back to handle the special char(s).
            },
            None => {
                // No more specials -- copy remainder.
                result.push_str(&text[pos..]);
                break;
            },
        }
    }

    Cow::Owned(result)
}

/// Escapes `&`, `<`, `>` and pushes the result directly into `buf`,
/// avoiding the intermediate `Cow`/`String` allocation of [`escape_html`].
///
/// Equivalent to `buf.push_str(&escape_html(text))` but zero-alloc when the
/// input contains no special characters and single-alloc-free when it does
/// (all output goes straight into `buf`).
pub fn push_escape_html(buf: &mut String, text: &str) {
    push_escape_impl(buf, text, false);
}

/// Escapes `&`, `<`, `>`, `"`, `'` and pushes directly into `buf`.
pub fn push_escape_xml(buf: &mut String, text: &str) {
    push_escape_impl(buf, text, true);
}

fn push_escape_impl(buf: &mut String, text: &str, xml_mode: bool) {
    let bytes = text.as_bytes();
    let len = bytes.len();

    let set: &[u8] = if xml_mode { b"&<>\"'" } else { b"&<>" };

    // Fast path: no special characters — push the whole string and return.
    let first_special = match super::intrinsics::byte_scan::find_first_in_set(bytes, set) {
        Some(idx) => idx,
        None => {
            buf.push_str(text);
            return;
        },
    };

    buf.reserve(len + len / 8);

    // Copy everything before the first special char in bulk.
    if first_special > 0 {
        buf.push_str(&text[..first_special]);
    }

    let mut pos = first_special;

    loop {
        while pos < len && is_html_special(bytes[pos], xml_mode) {
            push_escape_byte(buf, bytes[pos]);
            pos += 1;
        }

        if pos >= len {
            break;
        }

        match super::intrinsics::byte_scan::find_first_in_set(&bytes[pos..], set) {
            Some(offset) => {
                buf.push_str(&text[pos..pos + offset]);
                pos += offset;
            },
            None => {
                buf.push_str(&text[pos..]);
                break;
            },
        }
    }
}

/// Returns `true` if the byte is an HTML/XML special character that needs escaping.
///
/// Uses conditional branches (if/else) instead of a match/jump-table. The match
/// compiled to an indirect branch that suffered 68.8% misprediction on mixed text
/// (Cachegrind: 77.3M mispredictions at this call site). Conditional branches are
/// independently predicted by the CPU, eliminating the indirect-branch storm.
#[inline(always)]
fn is_html_special(b: u8, xml_mode: bool) -> bool {
    b == b'&' || b == b'<' || b == b'>' || (xml_mode && (b == b'"' || b == b'\''))
}

/// Push the escape sequence for a known-special byte using conditional branches
/// instead of a match/jump-table. Conditional branches are independently predicted
/// by the CPU's branch predictor, avoiding the indirect-branch misprediction storm
/// that a match-based jump table causes on mixed character sequences.
#[inline(always)]
fn push_escape_byte(buf: &mut String, b: u8) {
    if b == b'&' {
        buf.push_str("&amp;");
    } else if b == b'<' {
        buf.push_str("&lt;");
    } else if b == b'>' {
        buf.push_str("&gt;");
    } else if b == b'"' {
        buf.push_str("&quot;");
    } else {
        buf.push_str("&apos;");
    }
}

// ---------------------------------------------------------------------------
// HTML tag stripping (memchr-accelerated)
// ---------------------------------------------------------------------------

/// Strips all HTML/XML tags, returning plain text.
///
/// Uses `memchr` to jump between `<` and `>` delimiters instead of scanning
/// character by character.
///
/// Block-level elements (`<br>`, `<p>`, `<div>`, `<h1>`–`<h6>`, `<li>`,
/// `<tr>`, `<td>`, `<th>`, `<blockquote>`) insert a space when removed so
/// that adjacent text nodes are not concatenated.  A post-processing step
/// collapses runs of whitespace and trims the result.
///
/// Returns `Cow::Borrowed` when the input contains no tags, avoiding
/// allocation entirely.
pub fn strip_tags(html: &str) -> Cow<'_, str> {
    let bytes = html.as_bytes();

    // Fast path: no tags at all -- zero allocation.
    if memchr(b'<', bytes).is_none() {
        return Cow::Borrowed(html);
    }

    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut pos = 0;
    // Depth counter for <head> nesting.  While > 0, suppress all text output
    // so that content inside <head> (including <title>) is not emitted.
    let mut head_depth: u32 = 0;
    // Whitespace collapsing state (integrated to avoid second allocation).
    let mut in_ws = true; // treat start as whitespace so leading is trimmed
    let mut newline_count: usize = 0;

    loop {
        // Find the next '<' (tag start).
        match memchr(b'<', &bytes[pos..]) {
            Some(offset) => {
                // Copy text before the tag (only when outside <head>),
                // collapsing whitespace inline via byte iteration.
                if offset > 0 && head_depth == 0 {
                    append_ws_collapsed(
                        &html[pos..pos + offset],
                        &mut result,
                        &mut in_ws,
                        &mut newline_count,
                    );
                }
                let tag_start = pos + offset;
                // Find the closing '>'.
                match memchr(b'>', &bytes[tag_start..]) {
                    Some(end_offset) => {
                        let tag_bytes = &bytes[tag_start..tag_start + end_offset + 1];
                        // Track <head> / </head> transitions.
                        if is_head_open_tag(tag_bytes) {
                            head_depth += 1;
                        } else if is_head_close_tag(tag_bytes) {
                            head_depth = head_depth.saturating_sub(1);
                        }
                        if head_depth == 0 && is_block_level_tag(tag_bytes) {
                            // Feed block-level newline into whitespace state
                            // instead of pushing directly into result.
                            if !in_ws {
                                in_ws = true;
                                newline_count = 1;
                            } else {
                                newline_count += 1;
                            }
                        }
                        pos = tag_start + end_offset + 1;
                    },
                    None => {
                        // Unclosed tag -- skip the rest.
                        break;
                    },
                }
            },
            None => {
                // No more tags -- copy remaining text.
                if head_depth == 0 {
                    append_ws_collapsed(&html[pos..], &mut result, &mut in_ws, &mut newline_count);
                }
                break;
            },
        }
    }

    Cow::Owned(result)
}

/// Appends `chunk` to `result` while collapsing whitespace runs.
///
/// Uses a 256-byte const lookup table for O(1) per-byte whitespace
/// classification, avoiding the per-call SIMD setup cost that
/// `find_first_in_set` incurs (PSHUFB LUT rebuild + AVX2 lane broadcast).
/// For the typical HTML→text workload (many small-to-medium text chunks
/// between tags), the table-driven loop is faster because the 256-byte
/// table stays in L1d cache and pays zero dispatch overhead.
///
/// All ASCII whitespace characters are single-byte, so splitting at
/// whitespace boundaries cannot break multi-byte UTF-8 sequences.
#[inline]
fn append_ws_collapsed(
    chunk: &str,
    result: &mut String,
    in_ws: &mut bool,
    newline_count: &mut usize,
) {
    /// 256-byte lookup table: `true` for the 5 ASCII whitespace bytes
    /// (SP 0x20, TAB 0x09, LF 0x0A, CR 0x0D, FF 0x0C), `false` otherwise.
    const IS_WS: [bool; 256] = {
        let mut t = [false; 256];
        t[b' ' as usize] = true;
        t[b'\t' as usize] = true;
        t[b'\n' as usize] = true;
        t[b'\r' as usize] = true;
        t[0x0C] = true; // form feed
        t
    };

    let bytes = chunk.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if IS_WS[bytes[i] as usize] {
            // Consume the whitespace run, counting newlines in the same pass.
            if !*in_ws {
                *in_ws = true;
                *newline_count = 0;
            }
            while i < len && IS_WS[bytes[i] as usize] {
                if bytes[i] == b'\n' {
                    *newline_count += 1;
                }
                i += 1;
            }
        } else {
            // Emit pending whitespace separator.
            if *in_ws && !result.is_empty() {
                if *newline_count >= 2 {
                    result.push_str("\n\n");
                } else if *newline_count == 1 {
                    result.push('\n');
                } else {
                    result.push(' ');
                }
            }
            *in_ws = false;
            *newline_count = 0;

            // Scan for next whitespace byte using the table.
            let start = i;
            i += 1;
            while i < len && !IS_WS[bytes[i] as usize] {
                i += 1;
            }
            result.push_str(&chunk[start..i]);
        }
    }
}

/// Returns `true` if the tag is an opening `<head` tag (case-insensitive).
fn is_head_open_tag(tag: &[u8]) -> bool {
    // Minimum: <head> = 6 bytes.  Must not be a closing tag.
    if tag.len() < 6 || tag[0] != b'<' || tag[1] == b'/' {
        return false;
    }
    // The tag name must be exactly "head" (case-insensitive), followed by
    // '>', ' ', '\t', '\n', '\r', or '/'.
    if !tag[1].eq_ignore_ascii_case(&b'h')
        || !tag[2].eq_ignore_ascii_case(&b'e')
        || !tag[3].eq_ignore_ascii_case(&b'a')
        || !tag[4].eq_ignore_ascii_case(&b'd')
    {
        return false;
    }
    matches!(tag[5], b'>' | b' ' | b'\t' | b'\n' | b'\r')
}

/// Returns `true` if the tag is a closing `</head>` tag (case-insensitive).
fn is_head_close_tag(tag: &[u8]) -> bool {
    // Minimum: </head> = 7 bytes.
    if tag.len() < 7 || tag[0] != b'<' || tag[1] != b'/' {
        return false;
    }
    if !tag[2].eq_ignore_ascii_case(&b'h')
        || !tag[3].eq_ignore_ascii_case(&b'e')
        || !tag[4].eq_ignore_ascii_case(&b'a')
        || !tag[5].eq_ignore_ascii_case(&b'd')
    {
        return false;
    }
    matches!(tag[6], b'>' | b' ' | b'\t' | b'\n' | b'\r')
}

/// Returns `true` if the tag (including `<` and `>`) is a block-level element
/// that should produce whitespace when stripped.
///
/// Matches opening and closing forms of: `br`, `p`, `div`, `h1`–`h6`, `li`,
/// `tr`, `td`, `th`, `blockquote`.
fn is_block_level_tag(tag: &[u8]) -> bool {
    // Minimum valid tag is `<p>` (3 bytes).
    if tag.len() < 3 {
        return false;
    }
    // Skip '<' and optional '/'.
    let start = if tag.len() > 1 && tag[1] == b'/' {
        2
    } else {
        1
    };
    // Find end of tag name: first space, '/', or '>'.
    let rest = &tag[start..];
    let name_end = rest
        .iter()
        .position(|&b| {
            b == b' ' || b == b'>' || b == b'/' || b == b'\t' || b == b'\n' || b == b'\r'
        })
        .unwrap_or(rest.len());
    let name = &rest[..name_end];

    // Case-insensitive comparison against known block-level tag names.
    matches!(name.len(), 1..=10)
        && match name.len() {
            2 => {
                // br, li, td, th, tr, h1-h6, hr
                let a = name[0].to_ascii_lowercase();
                let b = name[1].to_ascii_lowercase();
                matches!(
                    (a, b),
                    (b'b', b'r')
                        | (b'l', b'i')
                        | (b't', b'd')
                        | (b't', b'h')
                        | (b't', b'r')
                        | (b'h', b'r')
                        | (b'h', b'1')
                        | (b'h', b'2')
                        | (b'h', b'3')
                        | (b'h', b'4')
                        | (b'h', b'5')
                        | (b'h', b'6')
                )
            },
            1 => {
                // p
                name[0].eq_ignore_ascii_case(&b'p')
            },
            3 => {
                // div
                let a = name[0].to_ascii_lowercase();
                let b = name[1].to_ascii_lowercase();
                let c = name[2].to_ascii_lowercase();
                (a, b, c) == (b'd', b'i', b'v')
            },
            10 => {
                // blockquote
                name.eq_ignore_ascii_case(b"blockquote")
            },
            _ => false,
        }
}

/// Strips HTML tags and unescapes basic HTML entities, returning plain text.
pub fn strip_tags_and_unescape(html: &str) -> String {
    let stripped = strip_tags(html);
    unescape_basic_entities(&stripped).into_owned()
}

// ---------------------------------------------------------------------------
// Case-insensitive ASCII helpers (zero allocation)
// ---------------------------------------------------------------------------

/// Case-insensitive ASCII suffix check without heap allocation.
///
/// Equivalent to `s.to_lowercase().ends_with(suffix)` for ASCII suffixes,
/// but avoids allocating a temporary `String`.
#[inline]
pub fn ends_with_ascii_ci(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.as_bytes()[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Case-insensitive ASCII substring check without heap allocation.
///
/// Delegates to [`find_case_insensitive`] which uses memchr + SIMD case-fold,
/// replacing the previous O(n*m) `.windows().any()` brute-force scan.
#[inline]
pub fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    find_ci(haystack.as_bytes(), needle.as_bytes()).is_some()
}

// ---------------------------------------------------------------------------
// HTML entity unescaping
// ---------------------------------------------------------------------------

/// Decodes the most common HTML/XML character entities.
///
/// Returns `Cow::Borrowed` when the input contains no entities, avoiding
/// allocation entirely.
pub fn unescape_basic_entities(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let len = bytes.len();

    // Fast path: no ampersands means nothing to unescape.
    if memchr(b'&', bytes).is_none() {
        return Cow::Borrowed(text);
    }

    let mut result = String::with_capacity(len);
    let mut pos = 0;

    while pos < len {
        match memchr(b'&', &bytes[pos..]) {
            Some(offset) => {
                result.push_str(&text[pos..pos + offset]);
                let entity_start = pos + offset;
                // Find the semicolon.
                let rest = &text[entity_start..];
                if let Some(semi) = rest.find(';') {
                    let entity = &rest[..semi + 1];
                    // First-byte dispatch: branch on the character after '&'
                    // to avoid a chain of 6 full-string comparisons.
                    let entity_bytes = entity.as_bytes();
                    let replaced = if entity_bytes.len() >= 4 {
                        match entity_bytes[1] {
                            b'a' => {
                                if entity_bytes.len() == 5
                                    && entity_bytes[2] == b'm'
                                    && entity_bytes[3] == b'p'
                                {
                                    // &amp;
                                    Some('&')
                                } else if entity_bytes.len() == 6
                                    && entity_bytes[2] == b'p'
                                    && entity_bytes[3] == b'o'
                                    && entity_bytes[4] == b's'
                                {
                                    // &apos;
                                    Some('\'')
                                } else {
                                    None
                                }
                            }
                            b'l' if entity_bytes.len() == 4 && entity_bytes[2] == b't' => {
                                // &lt;
                                Some('<')
                            }
                            b'g' if entity_bytes.len() == 4 && entity_bytes[2] == b't' => {
                                // &gt;
                                Some('>')
                            }
                            b'q' if entity_bytes.len() == 6
                                && entity_bytes[2] == b'u'
                                && entity_bytes[3] == b'o'
                                && entity_bytes[4] == b't' =>
                            {
                                // &quot;
                                Some('"')
                            }
                            b'n' if entity_bytes.len() == 6
                                && entity_bytes[2] == b'b'
                                && entity_bytes[3] == b's'
                                && entity_bytes[4] == b'p' =>
                            {
                                // &nbsp;
                                Some('\u{00A0}')
                            }
                            b'#' => {
                                // Numeric entity: &#NNN; or &#xHHH;
                                let num_part = &entity[2..entity.len() - 1];
                                let code_point = if let Some(hex) = num_part
                                    .strip_prefix('x')
                                    .or_else(|| num_part.strip_prefix('X'))
                                {
                                    u32::from_str_radix(hex, 16).ok()
                                } else {
                                    num_part.parse::<u32>().ok()
                                };
                                if let Some(cp) = code_point
                                    && let Some(ch) = char::from_u32(cp)
                                {
                                    result.push(ch);
                                    pos = entity_start + semi + 1;
                                    continue;
                                }
                                None
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    if let Some(ch) = replaced {
                        result.push(ch);
                    } else {
                        // Unknown entity -- keep as-is
                        result.push_str(entity);
                    }
                    pos = entity_start + semi + 1;
                } else {
                    // No semicolon found -- copy the '&' literally.
                    result.push('&');
                    pos = entity_start + 1;
                }
            },
            None => {
                result.push_str(&text[pos..]);
                break;
            },
        }
    }

    Cow::Owned(result)
}

// ---------------------------------------------------------------------------
// CP-1252 decoding (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Converts a single CP-1252 byte to its Unicode character.
#[inline]
pub fn cp1252_byte_to_char(byte: u8) -> char {
    super::intrinsics::cp1252::cp1252_byte_to_char(byte)
}

/// Decodes a CP-1252 (Windows-1252) byte slice to a Unicode `String`.
pub fn decode_cp1252(data: &[u8]) -> String {
    super::intrinsics::cp1252::decode_cp1252(data)
}

// ---------------------------------------------------------------------------
// Hex decoding (delegated to intrinsics)
// ---------------------------------------------------------------------------

/// Decodes pairs of hex ASCII characters into bytes.
///
/// Skips whitespace between hex digits. Non-hex characters (other than
/// whitespace) cause the pair to be skipped.
pub fn decode_hex_pairs(hex: &str) -> Vec<u8> {
    super::intrinsics::hex_decode::decode_hex_pairs(hex)
}

// ---------------------------------------------------------------------------
// ASCII detection (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Returns `true` if every byte in `data` is in the ASCII range (0x00–0x7F).
pub fn is_all_ascii(data: &[u8]) -> bool {
    super::intrinsics::is_ascii::is_all_ascii(data)
}

// ---------------------------------------------------------------------------
// Whitespace skipping (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Returns the number of leading XML-whitespace bytes (0x20, 0x09, 0x0A, 0x0D).
pub fn skip_whitespace(data: &[u8]) -> usize {
    super::intrinsics::skip_ws::skip_whitespace(data)
}

// ---------------------------------------------------------------------------
// Short pattern search (delegated to intrinsics for SIMD acceleration)
// ---------------------------------------------------------------------------

/// Finds the first occurrence of a 2–4 byte `needle` in `haystack`.
pub fn find_short_pattern(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    super::intrinsics::short_pattern::find_short_pattern(haystack, needle)
}

// ---------------------------------------------------------------------------
// Case-insensitive ASCII search (allocation-free)
// ---------------------------------------------------------------------------

/// Finds a byte pattern case-insensitively, trying exact lowercase match first.
/// The fast path uses SIMD-accelerated `memchr::memmem::find`.
/// Falls back to byte-by-byte case-insensitive search only when needed
/// (e.g. `<TITLE>` in old-style HTML). Most modern HTML is lowercase so the
/// fast path succeeds and we skip the ~4× slower fallback.
#[inline]
pub fn find_ci(haystack: &[u8], lowercase_pattern: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, lowercase_pattern)
        .or_else(|| find_case_insensitive(haystack, lowercase_pattern))
}

/// Finds `needle` in `haystack` using ASCII case-insensitive comparison.
///
/// Both `haystack` and `needle` must be valid UTF-8 byte slices. This function
/// only folds ASCII letters (A-Z / a-z); non-ASCII bytes are compared exactly.
///
/// Uses `memchr` to quickly locate candidate start positions based on the first
/// byte of the needle.
pub fn find_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    let first_lower = needle[0].to_ascii_lowercase();
    let first_upper = needle[0].to_ascii_uppercase();

    let mut pos = 0;
    loop {
        // Use memchr2 to find the next candidate position matching first byte
        // in either case.
        let offset = if first_lower == first_upper {
            memchr(first_lower, &haystack[pos..])?
        } else {
            memchr2(first_lower, first_upper, &haystack[pos..])?
        };
        let candidate = pos + offset;

        if candidate + needle.len() > haystack.len() {
            return None;
        }

        // Compare the rest of the needle case-insensitively.
        let matched = super::intrinsics::case_fold::eq_ignore_ascii_case(
            &haystack[candidate..candidate + needle.len()],
            needle,
        );

        if matched {
            return Some(candidate);
        }

        pos = candidate + 1;
    }
}

/// Returns all byte offsets where `needle` matches case-insensitively in `haystack`.
///
/// Zero-allocation alternative to lowering the entire haystack and using
/// `memchr::memmem::find_iter`. Uses `find_case_insensitive` in a loop.
pub fn find_all_case_insensitive(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut positions = Vec::new();
    if needle.is_empty() || needle.len() > haystack.len() {
        return positions;
    }
    let mut offset = 0;
    while let Some(pos) = find_case_insensitive(&haystack[offset..], needle) {
        positions.push(offset + pos);
        offset += pos + 1;
    }
    positions
}

// ---------------------------------------------------------------------------
// Pre-lowered haystack for batch case-insensitive searching
// ---------------------------------------------------------------------------

/// Creates a lowered copy of ASCII bytes suitable for exact-match searching.
///
/// Positions in the returned buffer map 1:1 to the original since
/// `make_ascii_lowercase` preserves byte count. Used by MOBI/HTML parsers
/// that need to perform multiple case-insensitive tag searches on the same
/// large buffer — lowering once + `memchr::memmem` is far faster than
/// per-candidate AVX2 case-fold comparison.
pub fn ascii_lowercase_copy(bytes: &[u8]) -> Vec<u8> {
    let mut lowered = bytes.to_vec();
    lowered.make_ascii_lowercase();
    lowered
}

// ---------------------------------------------------------------------------
// Fast UTF-8 conversion with SIMD-accelerated ASCII fast path
// ---------------------------------------------------------------------------

/// Converts a byte slice to an owned `String`, avoiding the expensive
/// `Utf8Chunks` iterator when the content is valid UTF-8 (the common case).
///
/// Three-tier strategy:
/// 1. **ASCII fast path** (SIMD-accelerated): if every byte is < 0x80, wraps
///    the bytes directly without any UTF-8 validation.
/// 2. **UTF-8 fast path**: `str::from_utf8` validates in bulk; on success,
///    wraps with a single allocation.
/// 3. **Lossy fallback**: only for genuinely malformed input.
pub fn bytes_to_string(bytes: &[u8]) -> String {
    super::xml_utils::bytes_to_string(bytes)
}

/// Converts a byte slice to a `Cow<str>`, borrowing when possible.
///
/// Unlike [`bytes_to_string`], this avoids allocation when the input is
/// already valid UTF-8 by returning a `Cow::Borrowed`.
pub fn bytes_to_cow_str(bytes: &[u8]) -> Cow<'_, str> {
    // ASCII fast path: skip full UTF-8 validation when every byte is < 0x80.
    if super::intrinsics::is_ascii::is_all_ascii(bytes) {
        // SAFETY: all bytes are < 0x80, which is valid UTF-8.
        return Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(bytes) });
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(String::from_utf8_lossy(bytes).into_owned()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- escape_html ---------------------------------------------------------

    #[test]
    fn escape_html_empty() {
        assert_eq!(escape_html(""), "");
    }

    #[test]
    fn escape_html_no_special() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    #[test]
    fn escape_html_all_special() {
        assert_eq!(escape_html("&<>"), "&amp;&lt;&gt;");
    }

    #[test]
    fn escape_html_mixed() {
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn escape_html_unicode() {
        assert_eq!(
            escape_html("caf\u{00E9} & th\u{00E9}"),
            "caf\u{00E9} &amp; th\u{00E9}"
        );
    }

    // -- escape_xml ----------------------------------------------------------

    #[test]
    fn escape_xml_quotes() {
        assert_eq!(escape_xml("a\"b'c"), "a&quot;b&apos;c");
    }

    // -- strip_tags ----------------------------------------------------------

    #[test]
    fn strip_tags_basic() {
        assert_eq!(strip_tags("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn strip_tags_no_tags() {
        assert_eq!(strip_tags("no tags here"), "no tags here");
    }

    #[test]
    fn strip_tags_nested() {
        assert_eq!(strip_tags("<div><p>A</p><p>B</p></div>"), "A\n\nB");
    }

    #[test]
    fn strip_tags_empty() {
        assert_eq!(strip_tags(""), "");
    }

    #[test]
    fn strip_tags_unclosed() {
        assert_eq!(strip_tags("before <unclosed"), "before");
    }

    #[test]
    fn strip_tags_br_inserts_space() {
        assert_eq!(
            strip_tags("CHAPTER I.<br/>Down the Rabbit-Hole"),
            "CHAPTER I.\nDown the Rabbit-Hole"
        );
    }

    #[test]
    fn strip_tags_br_variants() {
        assert_eq!(strip_tags("A<br>B"), "A\nB");
        assert_eq!(strip_tags("A<br/>B"), "A\nB");
        assert_eq!(strip_tags("A<br />B"), "A\nB");
        assert_eq!(strip_tags("A<BR>B"), "A\nB");
    }

    #[test]
    fn strip_tags_headings_insert_space() {
        assert_eq!(strip_tags("<h1>Title</h1>Body"), "Title\nBody");
        assert_eq!(strip_tags("<h3>Sub</h3>Text"), "Sub\nText");
    }

    #[test]
    fn strip_tags_inline_no_extra_space() {
        // Inline tags like <b>, <i>, <span> should NOT insert extra space.
        assert_eq!(strip_tags("Hello <b>bold</b> world"), "Hello bold world");
        assert_eq!(strip_tags("<span>A</span><span>B</span>"), "AB");
    }

    #[test]
    fn strip_tags_paragraph_separation() {
        // Multiple paragraphs should be separated by blank lines (\n\n),
        // while single line breaks (br) remain single newlines.
        assert_eq!(
            strip_tags("<p>First paragraph.</p><p>Second paragraph.</p><p>Third paragraph.</p>"),
            "First paragraph.\n\nSecond paragraph.\n\nThird paragraph."
        );
        // A <br> within a paragraph should produce a single newline, not a blank line.
        assert_eq!(
            strip_tags("<p>Line one.<br>Line two.</p><p>Next paragraph.</p>"),
            "Line one.\nLine two.\n\nNext paragraph."
        );
    }

    // -- unescape_basic_entities ---------------------------------------------

    #[test]
    fn unescape_common() {
        assert_eq!(
            unescape_basic_entities("&amp; &lt; &gt; &quot;"),
            "& < > \""
        );
    }

    #[test]
    fn unescape_no_entities() {
        assert_eq!(unescape_basic_entities("hello"), "hello");
    }

    #[test]
    fn unescape_unknown_entity() {
        assert_eq!(unescape_basic_entities("&foo;"), "&foo;");
    }

    #[test]
    fn unescape_numeric_decimal_em_dash() {
        assert_eq!(unescape_basic_entities("&#8212;"), "\u{2014}");
    }

    #[test]
    fn unescape_numeric_hex_em_dash() {
        assert_eq!(unescape_basic_entities("&#x2014;"), "\u{2014}");
    }

    #[test]
    fn unescape_numeric_hex_uppercase() {
        assert_eq!(unescape_basic_entities("&#X2014;"), "\u{2014}");
    }

    #[test]
    fn unescape_numeric_copyright() {
        assert_eq!(unescape_basic_entities("&#169;"), "\u{00A9}");
    }

    #[test]
    fn unescape_numeric_curly_quotes() {
        assert_eq!(
            unescape_basic_entities("&#8220;text&#8221;"),
            "\u{201C}text\u{201D}"
        );
    }

    #[test]
    fn unescape_mixed_named_and_numeric() {
        assert_eq!(
            unescape_basic_entities("&amp; &#8212; &lt;"),
            "& \u{2014} <"
        );
    }

    #[test]
    fn unescape_invalid_numeric_entity_kept() {
        // 0xFFFFFFFF is not a valid Unicode scalar value
        assert_eq!(unescape_basic_entities("&#4294967295;"), "&#4294967295;");
    }

    #[test]
    fn unescape_apos() {
        assert_eq!(unescape_basic_entities("&apos;"), "'");
    }

    #[test]
    fn unescape_nbsp() {
        assert_eq!(unescape_basic_entities("&nbsp;"), "\u{00A0}");
    }

    // -- decode_cp1252 -------------------------------------------------------

    #[test]
    fn cp1252_ascii() {
        assert_eq!(decode_cp1252(b"Hello"), "Hello");
    }

    #[test]
    fn cp1252_special_bytes() {
        // 0x93 = left double quote, 0x94 = right double quote
        assert_eq!(decode_cp1252(&[0x93, 0x94]), "\u{201C}\u{201D}");
    }

    #[test]
    fn cp1252_euro() {
        assert_eq!(cp1252_byte_to_char(0x80), '\u{20AC}');
    }

    // -- decode_hex_pairs ----------------------------------------------------

    #[test]
    fn hex_decode_basic() {
        assert_eq!(decode_hex_pairs("48656c6c6f"), b"Hello");
    }

    #[test]
    fn hex_decode_with_whitespace() {
        assert_eq!(decode_hex_pairs("48 65 6c 6c 6f"), b"Hello");
    }

    #[test]
    fn hex_decode_uppercase() {
        assert_eq!(decode_hex_pairs("4F4B"), b"OK");
    }

    #[test]
    fn hex_decode_empty() {
        assert_eq!(decode_hex_pairs(""), b"");
    }

    // -- find_case_insensitive -----------------------------------------------

    #[test]
    fn case_insensitive_basic() {
        assert_eq!(find_case_insensitive(b"Hello World", b"hello"), Some(0));
    }

    #[test]
    fn case_insensitive_middle() {
        assert_eq!(find_case_insensitive(b"foo BAR baz", b"bar"), Some(4));
    }

    #[test]
    fn case_insensitive_no_match() {
        assert_eq!(find_case_insensitive(b"Hello", b"xyz"), None);
    }

    #[test]
    fn case_insensitive_empty_needle() {
        assert_eq!(find_case_insensitive(b"Hello", b""), None);
    }

    // -- strip_tags: <head> suppression -------------------------------------

    #[test]
    fn strip_tags_suppresses_head_content() {
        let html = r#"<html><head><title>My Book | Publisher</title></head><body><p>Content</p></body></html>"#;
        let result = strip_tags(html);
        assert!(
            !result.contains("My Book"),
            "Title tag text should be suppressed: {result}"
        );
        assert!(
            !result.contains("Publisher"),
            "Title tag text should be suppressed: {result}"
        );
        assert!(
            result.contains("Content"),
            "Body content should remain: {result}"
        );
    }

    #[test]
    fn strip_tags_suppresses_head_with_meta() {
        let html = r#"<html><HEAD><title>Title</title><meta name="author" content="Me"/></HEAD><body><p>Body</p></body></html>"#;
        let result = strip_tags(html);
        assert!(
            !result.contains("Title"),
            "HEAD content should be suppressed: {result}"
        );
        assert!(
            result.contains("Body"),
            "Body content should remain: {result}"
        );
    }

    #[test]
    fn strip_tags_no_head_unchanged() {
        // Without a <head> tag, behaviour is unchanged.
        let html = "<p>Hello</p>";
        assert_eq!(strip_tags(html), "Hello");
    }
}
