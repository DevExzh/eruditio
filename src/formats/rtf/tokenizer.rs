//! RTF tokenizer — lexes an RTF byte stream into tokens.

/// A token produced by the RTF lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum RtfToken {
    /// `{` — opens a group.
    GroupStart,
    /// `}` — closes a group.
    GroupEnd,
    /// A control word like `\par`, `\b`, `\fs24`.
    /// The parameter is optional (e.g., `\b` has no param, `\fs24` has param 24).
    ControlWord { name: String, param: Option<i32> },
    /// A control symbol like `\\`, `\{`, `\}`, `\~`, `\-`, `\_`.
    ControlSymbol(char),
    /// A hex-encoded byte: `\'HH`.
    HexByte(u8),
    /// Plain text content.
    Text(String),
    /// A Unicode escape: `\uN` (signed 16-bit value).
    Unicode(i32),
}

/// Maximum number of tokens to prevent DoS from crafted RTF files.
const MAX_TOKENS: usize = 10_000_000;

/// Tokenizes an RTF byte stream into a sequence of tokens.
///
/// Handles control words, control symbols, hex escapes, Unicode escapes,
/// group delimiters, and plain text. Newlines and carriage returns in
/// the RTF source are ignored (they're not meaningful in RTF).
///
/// Returns an error if the token count exceeds `MAX_TOKENS`.
pub fn tokenize(input: &[u8]) -> std::result::Result<Vec<RtfToken>, &'static str> {
    let mut tokens = Vec::new();
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
            },
            b'}' => {
                tokens.push(RtfToken::GroupEnd);
                pos += 1;
            },
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
                            if let Some(byte) = parse_hex_byte(hi, lo) {
                                tokens.push(RtfToken::HexByte(byte));
                            }
                            pos += 2;
                        }
                    },
                    // Control symbols: \\ \{ \} \~ \- \_
                    c @ (b'\\' | b'{' | b'}' | b'~' | b'-' | b'_' | b'*') => {
                        tokens.push(RtfToken::ControlSymbol(c as char));
                        pos += 1;
                    },
                    // Newline after backslash = \par equivalent
                    b'\n' | b'\r' => {
                        tokens.push(RtfToken::ControlWord {
                            name: "par".into(),
                            param: None,
                        });
                        pos += 1;
                        // Skip \r\n pair.
                        if pos < len && input[pos] == b'\n' {
                            pos += 1;
                        }
                    },
                    // Control word: letters followed by optional numeric parameter.
                    c if c.is_ascii_alphabetic() => {
                        let (name, param, new_pos) = read_control_word(input, pos);

                        // Check for Unicode escape: \uN
                        if name == "u" {
                            if let Some(val) = param {
                                tokens.push(RtfToken::Unicode(val));
                                // Skip the replacement character after \uN.
                                let skip_pos = skip_unicode_replacement(input, new_pos);
                                pos = skip_pos;
                            } else {
                                tokens.push(RtfToken::ControlWord { name, param });
                                pos = new_pos;
                            }
                        } else {
                            tokens.push(RtfToken::ControlWord { name, param });
                            pos = new_pos;
                        }
                    },
                    _ => {
                        // Unknown control symbol — treat as symbol.
                        tokens.push(RtfToken::ControlSymbol(input[pos] as char));
                        pos += 1;
                    },
                }
            },
            b'\n' | b'\r' => {
                // Bare newlines (and any following whitespace) are ignored in RTF.
                // Use SIMD-accelerated skip for consecutive whitespace bytes.
                let remaining = &input[pos..len];
                let ws_count = crate::formats::common::intrinsics::skip_ws::skip_whitespace(remaining);
                pos += ws_count.max(1); // advance at least 1 to avoid infinite loop
            },
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
                let text = String::from_utf8_lossy(&input[start..pos]).into_owned();
                if !text.is_empty() {
                    tokens.push(RtfToken::Text(text));
                }
            },
        }
    }

    Ok(tokens)
}

/// Reads a control word starting at `pos` (which points to the first letter).
/// Returns (name, optional_param, new_position).
fn read_control_word(input: &[u8], mut pos: usize) -> (String, Option<i32>, usize) {
    let len = input.len();
    let start = pos;

    // Read alphabetic name.
    while pos < len && input[pos].is_ascii_alphabetic() {
        pos += 1;
    }
    let name = String::from_utf8_lossy(&input[start..pos]).into_owned();

    // Read optional numeric parameter (may start with '-').
    let param = if pos < len && (input[pos].is_ascii_digit() || input[pos] == b'-') {
        let param_start = pos;
        if input[pos] == b'-' {
            pos += 1;
        }
        while pos < len && input[pos].is_ascii_digit() {
            pos += 1;
        }
        let param_str = String::from_utf8_lossy(&input[param_start..pos]);
        param_str.parse::<i32>().ok()
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
fn skip_unicode_replacement(input: &[u8], mut pos: usize) -> usize {
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
        && input[pos + 1].is_ascii_alphabetic()
    {
        // Skip control word replacement.
        pos += 1;
        while pos < len && input[pos].is_ascii_alphabetic() {
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

/// Parses two hex ASCII characters into a byte.
fn parse_hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let h = hex_digit(hi)?;
    let l = hex_digit(lo)?;
    Some((h << 4) | l)
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
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
}
