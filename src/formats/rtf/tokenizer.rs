//! RTF tokenizer — lexes an RTF byte stream into tokens.
//!
//! Performance-critical path: this module uses lookup tables for ASCII
//! classification, `unsafe` UTF-8 elision where byte contents are proven
//! ASCII-only, and inlined hex parsing to minimize per-token overhead.

use std::borrow::Cow;

/// A token produced by the RTF lexer.
///
/// Lifetime `'a` ties borrowed string data back to the input byte slice,
/// enabling zero-copy tokenization for the common ASCII/UTF-8 case.
#[derive(Debug, Clone, PartialEq)]
pub enum RtfToken<'a> {
    /// `{` — opens a group.
    GroupStart,
    /// `}` — closes a group.
    GroupEnd,
    /// A control word like `\par`, `\b`, `\fs24`.
    /// The parameter is optional (e.g., `\b` has no param, `\fs24` has param 24).
    ControlWord {
        name: Cow<'a, str>,
        param: Option<i32>,
    },
    /// A control symbol like `\\`, `\{`, `\}`, `\~`, `\-`, `\_`.
    ControlSymbol(char),
    /// A hex-encoded byte: `\'HH`.
    HexByte(u8),
    /// Plain text content.
    Text(Cow<'a, str>),
    /// A Unicode escape: `\uN` (signed 16-bit value).
    Unicode(i32),
}

/// Maximum number of tokens to prevent DoS from crafted RTF files.
const MAX_TOKENS: usize = 10_000_000;

/// Lookup table for hex digit values. 0xFF means "not a hex digit".
static HEX_LUT: [u8; 256] = {
    let mut lut = [0xFFu8; 256];
    let mut i = 0u8;
    loop {
        lut[i as usize] = match i {
            b'0'..=b'9' => i - b'0',
            b'a'..=b'f' => i - b'a' + 10,
            b'A'..=b'F' => i - b'A' + 10,
            _ => 0xFF,
        };
        if i == 255 {
            break;
        }
        i += 1;
    }
    lut
};

/// Lookup table: `true` for ASCII alphabetic bytes (a-z, A-Z).
static IS_ALPHA: [bool; 256] = {
    let mut lut = [false; 256];
    let mut i = 0u8;
    loop {
        lut[i as usize] = i.is_ascii_alphabetic();
        if i == 255 {
            break;
        }
        i += 1;
    }
    lut
};

/// Tokenizes an RTF byte stream into a sequence of tokens.
///
/// Handles control words, control symbols, hex escapes, Unicode escapes,
/// group delimiters, and plain text. Newlines and carriage returns in
/// the RTF source are ignored (they're not meaningful in RTF).
///
/// Returns an error if the token count exceeds `MAX_TOKENS`.
///
/// This implementation is zero-copy: token string data borrows directly
/// from the input slice when the bytes are valid UTF-8 (the common case
/// for RTF, which is predominantly ASCII). Heap allocation only occurs
/// for the rare lossy-UTF-8 fallback path.
pub fn tokenize(input: &[u8]) -> std::result::Result<Vec<RtfToken<'_>>, &'static str> {
    // Pre-allocate: ~1 token per 8 bytes is a reasonable estimate for RTF.
    let mut tokens = Vec::with_capacity(input.len() / 8);
    let mut pos = 0;
    let len = input.len();

    while pos < len {
        if tokens.len() >= MAX_TOKENS {
            return Err("RTF token limit exceeded (possible malformed input)");
        }

        match input[pos] {
            b'{' => {
                tokens.push(RtfToken::GroupStart);
                pos += 1;
            }
            b'}' => {
                tokens.push(RtfToken::GroupEnd);
                pos += 1;
            }
            b'\\' => {
                pos += 1;
                if pos >= len {
                    break;
                }
                match input[pos] {
                    // Hex escape: \'HH
                    b'\'' => {
                        pos += 1;
                        if pos + 1 < len {
                            let hi = input[pos];
                            let lo = input[pos + 1];
                            if let Some(byte) = parse_hex_byte_lut(hi, lo) {
                                tokens.push(RtfToken::HexByte(byte));
                            }
                            pos += 2;
                        }
                    }
                    // Control symbols: \\ \{ \} \~ \- \_
                    c @ (b'\\' | b'{' | b'}' | b'~' | b'-' | b'_' | b'*') => {
                        tokens.push(RtfToken::ControlSymbol(c as char));
                        pos += 1;
                    }
                    // Newline after backslash = \par equivalent
                    b'\n' | b'\r' => {
                        tokens.push(RtfToken::ControlWord {
                            name: Cow::Borrowed("par"),
                            param: None,
                        });
                        pos += 1;
                        // Skip \r\n pair.
                        if pos < len && input[pos] == b'\n' {
                            pos += 1;
                        }
                    }
                    // Control word: letters followed by optional numeric parameter.
                    c if IS_ALPHA[c as usize] => {
                        let (name, param, new_pos) = read_control_word_fast(input, pos);

                        // Check for Unicode escape: \uN
                        if name == "u" {
                            if let Some(val) = param {
                                tokens.push(RtfToken::Unicode(val));
                                // Skip the replacement character after \uN.
                                let skip_pos = skip_unicode_replacement_fast(input, new_pos);
                                pos = skip_pos;
                            } else {
                                tokens.push(RtfToken::ControlWord {
                                    name: Cow::Borrowed(name),
                                    param,
                                });
                                pos = new_pos;
                            }
                        } else {
                            tokens.push(RtfToken::ControlWord {
                                name: Cow::Borrowed(name),
                                param,
                            });
                            pos = new_pos;
                        }
                    }
                    _ => {
                        // Unknown control symbol — treat as symbol.
                        tokens.push(RtfToken::ControlSymbol(input[pos] as char));
                        pos += 1;
                    }
                }
            }
            b'\n' | b'\r' => {
                // Bare newlines and carriage returns are ignored in RTF.
                // Skip consecutive newlines/CRs but NOT spaces/tabs (those are text).
                while pos < len && matches!(input[pos], b'\n' | b'\r') {
                    pos += 1;
                }
            }
            _ => {
                // Plain text — collect until we hit a control character.
                let start = pos;
                let remaining = &input[pos..len];
                // Find next structural delimiter (\ { })
                let struct_end =
                    memchr::memchr3(b'\\', b'{', b'}', remaining).unwrap_or(remaining.len());
                // Also check for newlines within that range
                let nl_end =
                    memchr::memchr2(b'\n', b'\r', &remaining[..struct_end]).unwrap_or(struct_end);
                pos += struct_end.min(nl_end);
                let slice = &input[start..pos];
                if !slice.is_empty() {
                    // Fast path: if the text is valid UTF-8, borrow directly (zero-copy).
                    // RTF is predominantly ASCII, so this almost always succeeds.
                    let text = match std::str::from_utf8(slice) {
                        Ok(s) => Cow::Borrowed(s),
                        Err(_) => Cow::Owned(String::from_utf8_lossy(slice).into_owned()),
                    };
                    tokens.push(RtfToken::Text(text));
                }
            }
        }
    }

    Ok(tokens)
}

/// Reads a control word starting at `pos` (which points to the first letter).
/// Returns (name, optional_param, new_position).
///
/// Uses a lookup table for ASCII alphabetic classification and
/// `unsafe from_utf8_unchecked` since the loop guarantees all bytes
/// are in [A-Za-z].
#[inline]
fn read_control_word_fast(input: &[u8], mut pos: usize) -> (&str, Option<i32>, usize) {
    let len = input.len();
    let start = pos;

    // Read alphabetic name using lookup table.
    while pos < len && IS_ALPHA[input[pos] as usize] {
        pos += 1;
    }
    // SAFETY: every byte in input[start..pos] was verified as ASCII
    // alphabetic (A-Z, a-z) by the loop. ASCII bytes are valid UTF-8.
    let name = unsafe { std::str::from_utf8_unchecked(&input[start..pos]) };

    // Read optional numeric parameter (may start with '-') directly from bytes.
    let param = if pos < len && (input[pos].is_ascii_digit() || input[pos] == b'-') {
        let negative = input[pos] == b'-';
        if negative {
            pos += 1;
        }
        let mut val: i32 = 0;
        while pos < len && input[pos].is_ascii_digit() {
            val = val.wrapping_mul(10).wrapping_add((input[pos] - b'0') as i32);
            pos += 1;
        }
        Some(if negative { -val } else { val })
    } else {
        None
    };

    // A single trailing space is consumed as part of the control word delimiter.
    if pos < len && input[pos] == b' ' {
        pos += 1;
    }

    (name, param, pos)
}

/// Skips the Unicode replacement character after `\uN`.
/// RTF specifies that one byte (or the number set by `\ucN`) follows as a fallback.
#[inline]
fn skip_unicode_replacement_fast(input: &[u8], mut pos: usize) -> usize {
    let len = input.len();
    if pos >= len {
        return pos;
    }

    // Skip a hex escape (\'HH = 4 bytes total), a control word, or a single byte.
    if pos + 3 < len && input[pos] == b'\\' && input[pos + 1] == b'\'' {
        pos += 4; // \' + HH (backslash, apostrophe, hex-hi, hex-lo)
    } else if pos < len
        && input[pos] == b'\\'
        && pos + 1 < len
        && IS_ALPHA[input[pos + 1] as usize]
    {
        // Skip control word replacement.
        pos += 1;
        while pos < len && IS_ALPHA[input[pos] as usize] {
            pos += 1;
        }
        while pos < len && (input[pos].is_ascii_digit() || input[pos] == b'-') {
            pos += 1;
        }
        if pos < len && input[pos] == b' ' {
            pos += 1;
        }
    } else if pos < len {
        // Skip one byte.
        pos += 1;
    }

    pos
}

/// Parses two hex ASCII characters into a byte using the lookup table.
/// Returns `None` if either character is not a valid hex digit.
#[inline(always)]
fn parse_hex_byte_lut(hi: u8, lo: u8) -> Option<u8> {
    let h = HEX_LUT[hi as usize];
    let l = HEX_LUT[lo as usize];
    // If either value is 0xFF (sentinel), the byte was not a hex digit.
    if (h | l) & 0xF0 != 0 {
        return None;
    }
    Some((h << 4) | l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_group_delimiters() {
        let tokens = tokenize(b"{}").unwrap();
        assert_eq!(tokens, vec![RtfToken::GroupStart, RtfToken::GroupEnd]);
    }

    #[test]
    fn tokenizes_control_word_no_param() {
        let tokens = tokenize(b"\\par ").unwrap();
        assert_eq!(
            tokens,
            vec![RtfToken::ControlWord {
                name: "par".into(),
                param: None,
            }]
        );
    }

    #[test]
    fn tokenizes_control_word_with_param() {
        let tokens = tokenize(b"\\fs24 ").unwrap();
        assert_eq!(
            tokens,
            vec![RtfToken::ControlWord {
                name: "fs".into(),
                param: Some(24),
            }]
        );
    }

    #[test]
    fn tokenizes_negative_param() {
        let tokens = tokenize(b"\\li-720 ").unwrap();
        assert_eq!(
            tokens,
            vec![RtfToken::ControlWord {
                name: "li".into(),
                param: Some(-720),
            }]
        );
    }

    #[test]
    fn tokenizes_control_symbols() {
        let tokens = tokenize(b"\\\\\\{\\}").unwrap();
        assert_eq!(
            tokens,
            vec![
                RtfToken::ControlSymbol('\\'),
                RtfToken::ControlSymbol('{'),
                RtfToken::ControlSymbol('}'),
            ]
        );
    }

    #[test]
    fn tokenizes_hex_escape() {
        let tokens = tokenize(b"\\'e9").unwrap();
        assert_eq!(tokens, vec![RtfToken::HexByte(0xe9)]);
    }

    #[test]
    fn tokenizes_unicode() {
        let tokens = tokenize(b"\\u8212?").unwrap();
        assert_eq!(tokens, vec![RtfToken::Unicode(8212)]);
    }

    #[test]
    fn tokenizes_plain_text() {
        let tokens = tokenize(b"Hello World").unwrap();
        assert_eq!(tokens, vec![RtfToken::Text("Hello World".into())]);
    }

    #[test]
    fn tokenizes_mixed_content() {
        let tokens = tokenize(b"{\\b Bold\\b0  text}").unwrap();
        assert_eq!(
            tokens,
            vec![
                RtfToken::GroupStart,
                RtfToken::ControlWord {
                    name: "b".into(),
                    param: None
                },
                RtfToken::Text("Bold".into()),
                RtfToken::ControlWord {
                    name: "b".into(),
                    param: Some(0)
                },
                RtfToken::Text(" text".into()),
                RtfToken::GroupEnd,
            ]
        );
    }

    #[test]
    fn skips_bare_newlines() {
        let tokens = tokenize(b"Hello\r\nWorld").unwrap();
        assert_eq!(
            tokens,
            vec![
                RtfToken::Text("Hello".into()),
                RtfToken::Text("World".into())
            ]
        );
    }

    #[test]
    fn rtf_header_tokenizes() {
        let input = b"{\\rtf1\\ansi\\deff0 Hello}";
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens[0], RtfToken::GroupStart);
        assert_eq!(
            tokens[1],
            RtfToken::ControlWord {
                name: "rtf".into(),
                param: Some(1)
            }
        );
    }

    #[test]
    fn hex_lut_matches_original() {
        for b in 0u8..=255 {
            let expected = match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            let got = {
                let v = HEX_LUT[b as usize];
                if v == 0xFF { None } else { Some(v) }
            };
            assert_eq!(got, expected, "HEX_LUT mismatch for byte 0x{:02X}", b);
        }
    }

    #[test]
    fn parse_hex_byte_lut_cases() {
        assert_eq!(parse_hex_byte_lut(b'e', b'9'), Some(0xe9));
        assert_eq!(parse_hex_byte_lut(b'0', b'0'), Some(0x00));
        assert_eq!(parse_hex_byte_lut(b'F', b'F'), Some(0xff));
        assert_eq!(parse_hex_byte_lut(b'g', b'0'), None);
        assert_eq!(parse_hex_byte_lut(b'0', b'z'), None);
    }

    #[test]
    fn is_alpha_lut_matches_stdlib() {
        for b in 0u8..=255 {
            assert_eq!(
                IS_ALPHA[b as usize],
                b.is_ascii_alphabetic(),
                "IS_ALPHA mismatch for byte 0x{:02X}",
                b
            );
        }
    }

    #[test]
    fn text_with_non_ascii_utf8() {
        // \xC3\xA9 is the UTF-8 encoding of U+00E9 (e-acute).
        let rtf: &[u8] = b"{\\rtf1 caf\xC3\xA9}";
        let tokens = tokenize(rtf).unwrap();
        let text_tokens: Vec<_> = tokens
            .iter()
            .filter_map(|t| match t {
                RtfToken::Text(s) => Some(s.as_ref()),
                _ => None,
            })
            .collect();
        assert!(
            text_tokens.iter().any(|t| t.contains("caf")),
            "Expected text containing 'caf', got: {:?}",
            text_tokens
        );
    }

    #[test]
    fn empty_input() {
        let tokens = tokenize(b"").unwrap();
        assert!(tokens.is_empty());
    }

    #[test]
    fn backslash_at_end() {
        let tokens = tokenize(b"text\\").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], RtfToken::Text("text".into()));
    }
}
