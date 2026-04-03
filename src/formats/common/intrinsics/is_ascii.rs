//! Bulk ASCII validation with SIMD acceleration.
//!
//! Provides [`is_all_ascii`], which returns `true` if every byte in the input
//! is in the ASCII range (0x00-0x7F).
//!
//! The dispatch function selects the best available SIMD backend at runtime:
//!
//! | Architecture  | Backend                                |
//! |---------------|----------------------------------------|
//! | x86 / x86_64  | AVX2 (32 B) then SSE2 (16 B) fallback |
//! | aarch64       | NEON (16 B)                            |
//! | wasm32        | SIMD128 (16 B)                         |
//! | *other*       | scalar loop                            |

// ---------------------------------------------------------------------------
// Scalar fallback
// ---------------------------------------------------------------------------

/// Byte-by-byte ASCII check (portable fallback).
pub(crate) fn is_all_ascii_scalar(data: &[u8]) -> bool {
    data.iter().all(|&b| b < 0x80)
}

// ---------------------------------------------------------------------------
// Dispatch function (scalar-only until SIMD is added in Task 2)
// ---------------------------------------------------------------------------

/// Returns `true` if every byte in `data` is in the ASCII range (0x00-0x7F).
/// Returns `true` for empty input.
///
/// Selects the best available SIMD implementation at runtime.
pub(crate) fn is_all_ascii(data: &[u8]) -> bool {
    is_all_ascii_scalar(data)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert!(is_all_ascii(b""));
        assert!(is_all_ascii_scalar(b""));
    }

    #[test]
    fn pure_ascii() {
        assert!(is_all_ascii(b"Hello, World!"));
        assert!(is_all_ascii_scalar(b"Hello, World!"));
    }

    #[test]
    fn all_zero() {
        assert!(is_all_ascii(&[0u8; 64]));
    }

    #[test]
    fn all_0x7f() {
        assert!(is_all_ascii(&[0x7Fu8; 64]));
    }

    #[test]
    fn single_non_ascii_at_start() {
        assert!(!is_all_ascii(&[0x80]));
        assert!(!is_all_ascii_scalar(&[0x80]));
    }

    #[test]
    fn single_non_ascii_at_end() {
        let mut data = vec![b'A'; 33];
        data[32] = 0x80;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_15() {
        let mut data = vec![b'A'; 32];
        data[15] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_16() {
        let mut data = vec![b'A'; 32];
        data[16] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn non_ascii_at_position_31() {
        let mut data = vec![b'A'; 32];
        data[31] = 0xFF;
        assert!(!is_all_ascii(&data));
    }

    #[test]
    fn all_0xff() {
        assert!(!is_all_ascii(&[0xFFu8; 64]));
    }

    #[test]
    fn exactly_16_bytes_ascii() {
        assert!(is_all_ascii(b"0123456789abcdef"));
    }

    #[test]
    fn exactly_32_bytes_ascii() {
        assert!(is_all_ascii(b"0123456789abcdef0123456789abcdef"));
    }

    #[test]
    fn short_input_under_16() {
        assert!(is_all_ascii(b"tiny"));
        assert!(!is_all_ascii(&[0x80]));
    }
}
