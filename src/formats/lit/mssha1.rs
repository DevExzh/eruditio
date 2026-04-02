//! Microsoft-specific SHA1 variant used in LIT file DRM.
//!
//! This is NOT standard SHA-1. It differs in three ways:
//!
//! 1. **Non-standard initial hash values** (byte-swapped/mangled from FIPS 180-1).
//! 2. **Custom round function** `f6_42(B, C, D) = (B + C) ^ C` using modular
//!    addition instead of pure bitwise operations.
//! 3. **Scrambled round-function assignments** at indices 3, 6, 10, 15, 26, 31,
//!    42, 51, and 68, replacing the standard round function for that index with
//!    a different one (or the custom `f6_42`).
//!
//! Ported from calibre's `calibre/ebooks/lit/mssha1.py`.
#![allow(dead_code)]

// Round function identifiers
const F_0_19: u8 = 0;
const F_20_39: u8 = 1;
const F_40_59: u8 = 2;
const F_6_42: u8 = 4;

/// Round-function table with Microsoft's modifications.
///
/// Standard SHA-1 uses `f0_19` for 0..19, `f20_39` for 20..39,
/// `f40_59` for 40..59, `f60_79` (= `f20_39`) for 60..79.
/// Microsoft replaces entries at [3, 6, 10, 15, 26, 31, 42, 51, 68].
static ROUND_FN: [u8; 80] = {
    let mut f = [0u8; 80];
    let mut i = 0;
    while i < 20 {
        f[i] = F_0_19;
        i += 1;
    }
    while i < 40 {
        f[i] = F_20_39;
        i += 1;
    }
    while i < 60 {
        f[i] = F_40_59;
        i += 1;
    }
    while i < 80 {
        f[i] = F_20_39; // f60_79 == f20_39
        i += 1;
    }
    // Microsoft's changes
    f[3] = F_20_39;
    f[6] = F_6_42;
    f[10] = F_20_39;
    f[15] = F_20_39;
    f[26] = F_0_19;
    f[31] = F_40_59;
    f[42] = F_6_42;
    f[51] = F_20_39;
    f[68] = F_0_19;
    f
};

static K: [u32; 4] = [
    0x5A82_7999, // 0..19
    0x6ED9_EBA1, // 20..39
    0x8F1B_BCDC, // 40..59
    0xCA62_C1D6, // 60..79
];

/// Microsoft-modified initial hash values (NOT the FIPS 180-1 constants).
const INIT_H: [u32; 5] = [
    0x3210_7654,
    0x2301_6745,
    0xC4E6_80A2,
    0xDC67_9823,
    0xD085_7A34,
];

fn round_fn(id: u8, b: u32, c: u32, d: u32) -> u32 {
    match id {
        F_0_19 => (b & (c ^ d)) ^ d,
        F_20_39 => b ^ c ^ d,
        F_40_59 => ((b | c) & d) | (b & c),
        F_6_42 => b.wrapping_add(c) ^ c,
        _ => unreachable!(),
    }
}

fn transform(h: &mut [u32; 5], initial_w: [u32; 16]) {
    let mut w = [0u32; 80];
    w[..16].copy_from_slice(&initial_w);

    for t in 16..80 {
        w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *h;

    for t in 0..80 {
        let temp = a
            .rotate_left(5)
            .wrapping_add(round_fn(ROUND_FN[t], b, c, d))
            .wrapping_add(e)
            .wrapping_add(w[t])
            .wrapping_add(K[t / 20]);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = temp;
    }

    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

fn bytes_to_words(data: &[u8]) -> [u32; 16] {
    let mut words = [0u32; 16];
    for (i, chunk) in data.chunks_exact(4).enumerate().take(16) {
        words[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    words
}

/// Compute the Microsoft SHA1 digest of the given data.
pub(crate) fn mssha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h = INIT_H;
    let total_bits = (data.len() as u64) * 8;

    // Process full 64-byte blocks
    let mut offset = 0;
    while offset + 64 <= data.len() {
        let w = bytes_to_words(&data[offset..offset + 64]);
        transform(&mut h, w);
        offset += 64;
    }

    // Pad the final block(s): standard SHA-1 padding
    let mut tail = data[offset..].to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0x00);
    }
    tail.extend_from_slice(&total_bits.to_be_bytes());

    for block in tail.chunks_exact(64) {
        let w = bytes_to_words(block);
        transform(&mut h, w);
    }

    let mut result = [0u8; 20];
    for (i, &val) in h.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

/// Streaming Microsoft SHA1 hasher, matching calibre's `mssha1.new()` / `.update()` / `.digest()` API.
pub struct MsSha1 {
    h: [u32; 5],
    buffer: Vec<u8>,
    total_len: u64,
}

impl Default for MsSha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl MsSha1 {
    pub fn new() -> Self {
        Self {
            h: INIT_H,
            buffer: Vec::new(),
            total_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        self.buffer.extend_from_slice(data);

        // Process all complete 64-byte blocks
        let mut offset = 0;
        while offset + 64 <= self.buffer.len() {
            let w = bytes_to_words(&self.buffer[offset..offset + 64]);
            transform(&mut self.h, w);
            offset += 64;
        }
        if offset > 0 {
            self.buffer.drain(..offset);
        }
    }

    pub fn finalize(&self) -> [u8; 20] {
        let mut h = self.h;
        let total_bits = self.total_len * 8;

        // Pad remaining buffer
        let mut tail = self.buffer.clone();
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0x00);
        }
        tail.extend_from_slice(&total_bits.to_be_bytes());

        for block in tail.chunks_exact(64) {
            let w = bytes_to_words(block);
            transform(&mut h, w);
        }

        let mut result = [0u8; 20];
        for (i, &val) in h.iter().enumerate() {
            result[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Cross-validated against calibre's Python mssha1 implementation.
    #[test]
    fn matches_calibre_python_reference() {
        // Python: mssha1.new().digest().hex()
        assert_eq!(
            hex(&mssha1_digest(b"")),
            "2944876c93c20a7dde5b3a5cc6d231a39ba593ff"
        );
        // Python: mssha1.new(b'abc').digest().hex()
        assert_eq!(
            hex(&mssha1_digest(b"abc")),
            "afa1704d4d9cea5c72ae64cad51b563c00c8af7d"
        );
        // Python: mssha1.new(b'x' * 66).digest().hex()
        assert_eq!(
            hex(&mssha1_digest(&vec![b'x'; 66])),
            "c2f5037436691955c5724b56a2a806a087c27add"
        );
    }

    #[test]
    fn streaming_matches_oneshot() {
        let data = b"Hello, Microsoft LIT world! This is a test of the mssha1 hash.";
        let oneshot = mssha1_digest(data);

        let mut streaming = MsSha1::new();
        streaming.update(&data[..10]);
        streaming.update(&data[10..30]);
        streaming.update(&data[30..]);
        let streamed = streaming.finalize();

        assert_eq!(oneshot, streamed);
    }

    #[test]
    fn finalize_is_nondestructive() {
        let mut hasher = MsSha1::new();
        hasher.update(b"part1");
        let d1 = hasher.finalize();
        let d2 = hasher.finalize();
        assert_eq!(d1, d2, "finalize should not alter internal state");
    }

    #[test]
    fn digest_changes_with_input() {
        let d1 = mssha1_digest(b"abc");
        let d2 = mssha1_digest(b"abd");
        assert_ne!(d1, d2);
    }

    #[test]
    fn large_input_works() {
        // Input larger than one block (64 bytes)
        let data = vec![0x42u8; 200];
        let digest = mssha1_digest(&data);
        assert_eq!(digest.len(), 20);
        // Just verify it doesn't panic and produces a valid-length digest
    }

    #[test]
    fn exact_block_boundary() {
        // Exactly 64 bytes — one full block, padding starts a new block
        let data = [0xABu8; 64];
        let digest = mssha1_digest(&data);
        assert_eq!(digest.len(), 20);
    }

    #[test]
    fn padding_boundary_56_bytes() {
        // 56 bytes — padding will push to 120 bytes (56 + 1 + 63 zeros wouldn't
        // leave room for the 8-byte length, so it needs a second block)
        let data = [0xCDu8; 56];
        let digest = mssha1_digest(&data);
        assert_eq!(digest.len(), 20);
    }
}
