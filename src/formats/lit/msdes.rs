//! Microsoft-specific DES implementation for LIT DRM decryption.
//!
//! This is a port of D3DES V5.09 by Richard Outerbridge (public domain) with
//! non-standard S-box substitution tables from Dan A. Jackson's openclit
//! library. Microsoft's LIT format uses these custom S-boxes instead of the
//! FIPS 46-3 standard tables, making standard DES implementations incompatible.
//!
//! Reference: `3rdparty/calibre/src/calibre/utils/msdes/des.c` + `spr.h`

// ---------------------------------------------------------------------------
// Microsoft-specific S-box tables (from openclit spr.h)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
static SP1: [u32; 64] = [
    0x02080800, 0x00080000, 0x02000002, 0x02080802,
    0x02000000, 0x00080802, 0x00080002, 0x02000002,
    0x00080802, 0x02080800, 0x02080000, 0x00000802,
    0x02000802, 0x02000000, 0x00000000, 0x00080002,
    0x00080000, 0x00000002, 0x02000800, 0x00080800,
    0x02080802, 0x02080000, 0x00000802, 0x02000800,
    0x00000002, 0x00000800, 0x00080800, 0x02080002,
    0x00000800, 0x02000802, 0x02080002, 0x00000000,
    0x00000000, 0x02080802, 0x02000800, 0x00080002,
    0x02080800, 0x00080000, 0x00000802, 0x02000800,
    0x02080002, 0x00000800, 0x00080800, 0x02000002,
    0x00080802, 0x00000002, 0x02000002, 0x02080000,
    0x02080802, 0x00080800, 0x02080000, 0x02000802,
    0x02000000, 0x00000802, 0x00080002, 0x00000000,
    0x00080000, 0x02000000, 0x02000802, 0x02080800,
    0x00000002, 0x02080002, 0x00000800, 0x00080802,
];

#[rustfmt::skip]
static SP2: [u32; 64] = [
    0x40108010, 0x00000000, 0x00108000, 0x40100000,
    0x40000010, 0x00008010, 0x40008000, 0x00108000,
    0x00008000, 0x40100010, 0x00000010, 0x40008000,
    0x00100010, 0x40108000, 0x40100000, 0x00000010,
    0x00100000, 0x40008010, 0x40100010, 0x00008000,
    0x00108010, 0x40000000, 0x00000000, 0x00100010,
    0x40008010, 0x00108010, 0x40108000, 0x40000010,
    0x40000000, 0x00100000, 0x00008010, 0x40108010,
    0x00100010, 0x40108000, 0x40008000, 0x00108010,
    0x40108010, 0x00100010, 0x40000010, 0x00000000,
    0x40000000, 0x00008010, 0x00100000, 0x40100010,
    0x00008000, 0x40000000, 0x00108010, 0x40008010,
    0x40108000, 0x00008000, 0x00000000, 0x40000010,
    0x00000010, 0x40108010, 0x00108000, 0x40100000,
    0x40100010, 0x00100000, 0x00008010, 0x40008000,
    0x40008010, 0x00000010, 0x40100000, 0x00108000,
];

#[rustfmt::skip]
static SP3: [u32; 64] = [
    0x04000001, 0x04040100, 0x00000100, 0x04000101,
    0x00040001, 0x04000000, 0x04000101, 0x00040100,
    0x04000100, 0x00040000, 0x04040000, 0x00000001,
    0x04040101, 0x00000101, 0x00000001, 0x04040001,
    0x00000000, 0x00040001, 0x04040100, 0x00000100,
    0x00000101, 0x04040101, 0x00040000, 0x04000001,
    0x04040001, 0x04000100, 0x00040101, 0x04040000,
    0x00040100, 0x00000000, 0x04000000, 0x00040101,
    0x04040100, 0x00000100, 0x00000001, 0x00040000,
    0x00000101, 0x00040001, 0x04040000, 0x04000101,
    0x00000000, 0x04040100, 0x00040100, 0x04040001,
    0x00040001, 0x04000000, 0x04040101, 0x00000001,
    0x00040101, 0x04000001, 0x04000000, 0x04040101,
    0x00040000, 0x04000100, 0x04000101, 0x00040100,
    0x04000100, 0x00000000, 0x04040001, 0x00000101,
    0x04000001, 0x00040101, 0x00000100, 0x04040000,
];

#[rustfmt::skip]
static SP4: [u32; 64] = [
    0x00401008, 0x10001000, 0x00000008, 0x10401008,
    0x00000000, 0x10400000, 0x10001008, 0x00400008,
    0x10401000, 0x10000008, 0x10000000, 0x00001008,
    0x10000008, 0x00401008, 0x00400000, 0x10000000,
    0x10400008, 0x00401000, 0x00001000, 0x00000008,
    0x00401000, 0x10001008, 0x10400000, 0x00001000,
    0x00001008, 0x00000000, 0x00400008, 0x10401000,
    0x10001000, 0x10400008, 0x10401008, 0x00400000,
    0x10400008, 0x00001008, 0x00400000, 0x10000008,
    0x00401000, 0x10001000, 0x00000008, 0x10400000,
    0x10001008, 0x00000000, 0x00001000, 0x00400008,
    0x00000000, 0x10400008, 0x10401000, 0x00001000,
    0x10000000, 0x10401008, 0x00401008, 0x00400000,
    0x10401008, 0x00000008, 0x10001000, 0x00401008,
    0x00400008, 0x00401000, 0x10400000, 0x10001008,
    0x00001008, 0x10000000, 0x10000008, 0x10401000,
];

#[rustfmt::skip]
static SP5: [u32; 64] = [
    0x08000000, 0x00010000, 0x00000400, 0x08010420,
    0x08010020, 0x08000400, 0x00010420, 0x08010000,
    0x00010000, 0x00000020, 0x08000020, 0x00010400,
    0x08000420, 0x08010020, 0x08010400, 0x00000000,
    0x00010400, 0x08000000, 0x00010020, 0x00000420,
    0x08000400, 0x00010420, 0x00000000, 0x08000020,
    0x00000020, 0x08000420, 0x08010420, 0x00010020,
    0x08010000, 0x00000400, 0x00000420, 0x08010400,
    0x08010400, 0x08000420, 0x00010020, 0x08010000,
    0x00010000, 0x00000020, 0x08000020, 0x08000400,
    0x08000000, 0x00010400, 0x08010420, 0x00000000,
    0x00010420, 0x08000000, 0x00000400, 0x00010020,
    0x08000420, 0x00000400, 0x00000000, 0x08010420,
    0x08010020, 0x08010400, 0x00000420, 0x00010000,
    0x00010400, 0x08010020, 0x08000400, 0x00000420,
    0x00000020, 0x00010420, 0x08010000, 0x08000020,
];

#[rustfmt::skip]
static SP6: [u32; 64] = [
    0x80000040, 0x00200040, 0x00000000, 0x80202000,
    0x00200040, 0x00002000, 0x80002040, 0x00200000,
    0x00002040, 0x80202040, 0x00202000, 0x80000000,
    0x80002000, 0x80000040, 0x80200000, 0x00202040,
    0x00200000, 0x80002040, 0x80200040, 0x00000000,
    0x00002000, 0x00000040, 0x80202000, 0x80200040,
    0x80202040, 0x80200000, 0x80000000, 0x00002040,
    0x00000040, 0x00202000, 0x00202040, 0x80002000,
    0x00002040, 0x80000000, 0x80002000, 0x00202040,
    0x80202000, 0x00200040, 0x00000000, 0x80002000,
    0x80000000, 0x00002000, 0x80200040, 0x00200000,
    0x00200040, 0x80202040, 0x00202000, 0x00000040,
    0x80202040, 0x00202000, 0x00200000, 0x80002040,
    0x80000040, 0x80200000, 0x00202040, 0x00000000,
    0x00002000, 0x80000040, 0x80002040, 0x80202000,
    0x80200000, 0x00002040, 0x00000040, 0x80200040,
];

#[rustfmt::skip]
static SP7: [u32; 64] = [
    0x00004000, 0x00000200, 0x01000200, 0x01000004,
    0x01004204, 0x00004004, 0x00004200, 0x00000000,
    0x01000000, 0x01000204, 0x00000204, 0x01004000,
    0x00000004, 0x01004200, 0x01004000, 0x00000204,
    0x01000204, 0x00004000, 0x00004004, 0x01004204,
    0x00000000, 0x01000200, 0x01000004, 0x00004200,
    0x01004004, 0x00004204, 0x01004200, 0x00000004,
    0x00004204, 0x01004004, 0x00000200, 0x01000000,
    0x00004204, 0x01004000, 0x01004004, 0x00000204,
    0x00004000, 0x00000200, 0x01000000, 0x01004004,
    0x01000204, 0x00004204, 0x00004200, 0x00000000,
    0x00000200, 0x01000004, 0x00000004, 0x01000200,
    0x00000000, 0x01000204, 0x01000200, 0x00004200,
    0x00000204, 0x00004000, 0x01004204, 0x01000000,
    0x01004200, 0x00000004, 0x00004004, 0x01004204,
    0x01000004, 0x01004200, 0x01004000, 0x00004004,
];

#[rustfmt::skip]
static SP8: [u32; 64] = [
    0x20800080, 0x20820000, 0x00020080, 0x00000000,
    0x20020000, 0x00800080, 0x20800000, 0x20820080,
    0x00000080, 0x20000000, 0x00820000, 0x00020080,
    0x00820080, 0x20020080, 0x20000080, 0x20800000,
    0x00020000, 0x00820080, 0x00800080, 0x20020000,
    0x20820080, 0x20000080, 0x00000000, 0x00820000,
    0x20000000, 0x00800000, 0x20020080, 0x20800080,
    0x00800000, 0x00020000, 0x20820000, 0x00000080,
    0x00800000, 0x00020000, 0x20000080, 0x20820080,
    0x00020080, 0x20000000, 0x00000000, 0x00820000,
    0x20800080, 0x20020080, 0x20020000, 0x00800080,
    0x20820000, 0x00000080, 0x00800080, 0x20020000,
    0x20820080, 0x00800000, 0x20800000, 0x20000080,
    0x00820000, 0x00020080, 0x20020080, 0x20800000,
    0x00000080, 0x20820000, 0x00820080, 0x00000000,
    0x20000000, 0x20800080, 0x00020000, 0x00820080,
];

// ---------------------------------------------------------------------------
// Standard DES key schedule tables (same as FIPS 46-3)
// ---------------------------------------------------------------------------

#[rustfmt::skip]
static PC1: [u8; 56] = [
    56, 48, 40, 32, 24, 16,  8,  0, 57, 49, 41, 33, 25, 17,
     9,  1, 58, 50, 42, 34, 26, 18, 10,  2, 59, 51, 43, 35,
    62, 54, 46, 38, 30, 22, 14,  6, 61, 53, 45, 37, 29, 21,
    13,  5, 60, 52, 44, 36, 28, 20, 12,  4, 27, 19, 11,  3,
];

#[rustfmt::skip]
static TOTROT: [u8; 16] = [
    1, 2, 4, 6, 8, 10, 12, 14, 15, 17, 19, 21, 23, 25, 27, 28,
];

#[rustfmt::skip]
static PC2: [u8; 48] = [
    13, 16, 10, 23,  0,  4,  2, 27, 14,  5, 20,  9,
    22, 18, 11,  3, 25,  7, 15,  6, 26, 19, 12,  1,
    40, 51, 30, 36, 46, 54, 29, 39, 50, 44, 32, 47,
    43, 48, 38, 55, 33, 52, 45, 41, 49, 35, 28, 31,
];

static BYTEBIT: [u16; 8] = [128, 64, 32, 16, 8, 4, 2, 1];

#[rustfmt::skip]
static BIGBYTE: [u32; 24] = [
    0x800000, 0x400000, 0x200000, 0x100000,
    0x080000, 0x040000, 0x020000, 0x010000,
    0x008000, 0x004000, 0x002000, 0x001000,
    0x000800, 0x000400, 0x000200, 0x000100,
    0x000080, 0x000040, 0x000020, 0x000010,
    0x000008, 0x000004, 0x000002, 0x000001,
];

// ---------------------------------------------------------------------------
// MsDes — Microsoft DES cipher
// ---------------------------------------------------------------------------

/// Microsoft-specific DES cipher using non-standard S-box tables.
pub struct MsDes {
    keys: [u32; 32],
}

impl MsDes {
    /// Create a decryptor for the given 8-byte key.
    pub fn new_decrypt(key: &[u8; 8]) -> Self {
        let keys = deskey(key, true);
        Self { keys }
    }

    /// Create an encryptor for the given 8-byte key.
    pub fn new_encrypt(key: &[u8; 8]) -> Self {
        let keys = deskey(key, false);
        Self { keys }
    }

    /// Process a single 8-byte block in place (encrypt or decrypt depending on construction).
    pub fn process_block(&self, block: &mut [u8; 8]) {
        let mut work = scrunch(block);
        desfunc(&mut work, &self.keys);
        unscrun(&work, block);
    }

    /// Decrypt a single 8-byte block in place.
    pub fn decrypt_block(&self, block: &mut [u8; 8]) {
        self.process_block(block);
    }

    /// Decrypt data in ECB mode. Pads the last block with zeros if needed.
    /// Returns the decrypted bytes (same length as padded input).
    pub fn decrypt_ecb(&self, data: &[u8]) -> Vec<u8> {
        let mut result = data.to_vec();
        let extra = result.len() % 8;
        if extra != 0 {
            result.resize(result.len() + (8 - extra), 0);
        }
        for chunk in result.chunks_exact_mut(8) {
            let block: &mut [u8; 8] = chunk.try_into().unwrap();
            self.decrypt_block(block);
        }
        result
    }

    /// Encrypt data in ECB mode. Pads the last block with zeros if needed.
    /// Returns the encrypted bytes (same length as padded input).
    pub fn encrypt_ecb(&self, data: &[u8]) -> Vec<u8> {
        let mut result = data.to_vec();
        let extra = result.len() % 8;
        if extra != 0 {
            result.resize(result.len() + (8 - extra), 0);
        }
        for chunk in result.chunks_exact_mut(8) {
            let block: &mut [u8; 8] = chunk.try_into().unwrap();
            self.process_block(block);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Core DES functions
// ---------------------------------------------------------------------------

/// Compute the 32-word key schedule from an 8-byte key.
/// `decrypt` = true for decryption (DE1), false for encryption (EN0).
#[allow(clippy::needless_range_loop)]
fn deskey(key: &[u8; 8], decrypt: bool) -> [u32; 32] {
    let mut pc1m = [0u8; 56];
    let mut pcr = [0u8; 56];
    let mut kn = [0u32; 32];

    for j in 0..56 {
        let l = PC1[j] as usize;
        let m = l & 7;
        pc1m[j] = if (key[l >> 3] as u16 & BYTEBIT[m]) != 0 {
            1
        } else {
            0
        };
    }

    for i in 0..16u32 {
        let m = if decrypt {
            ((15 - i) << 1) as usize
        } else {
            (i << 1) as usize
        };
        let n = m + 1;
        kn[m] = 0;
        kn[n] = 0;

        for j in 0..28 {
            let l = j + TOTROT[i as usize] as usize;
            pcr[j] = if l < 28 { pc1m[l] } else { pc1m[l - 28] };
        }
        for j in 28..56 {
            let l = j + TOTROT[i as usize] as usize;
            pcr[j] = if l < 56 { pc1m[l] } else { pc1m[l - 28] };
        }

        for j in 0..24 {
            if pcr[PC2[j] as usize] != 0 {
                kn[m] |= BIGBYTE[j];
            }
            if pcr[PC2[j + 24] as usize] != 0 {
                kn[n] |= BIGBYTE[j];
            }
        }
    }

    cookey(&kn)
}

/// Rearrange the raw key schedule into the form used by `desfunc`.
fn cookey(raw: &[u32; 32]) -> [u32; 32] {
    let mut cooked = [0u32; 32];
    let mut ci = 0;
    for i in 0..16 {
        let raw0 = raw[i * 2];
        let raw1 = raw[i * 2 + 1];
        cooked[ci] = (raw0 & 0x00fc0000) << 6;
        cooked[ci] |= (raw0 & 0x00000fc0) << 10;
        cooked[ci] |= (raw1 & 0x00fc0000) >> 10;
        cooked[ci] |= (raw1 & 0x00000fc0) >> 6;
        ci += 1;
        cooked[ci] = (raw0 & 0x0003f000) << 12;
        cooked[ci] |= (raw0 & 0x0000003f) << 16;
        cooked[ci] |= (raw1 & 0x0003f000) >> 4;
        cooked[ci] |= raw1 & 0x0000003f;
        ci += 1;
    }
    cooked
}

/// Convert 8 bytes into two 32-bit words (big-endian).
fn scrunch(input: &[u8; 8]) -> [u32; 2] {
    [
        (u32::from(input[0]) << 24)
            | (u32::from(input[1]) << 16)
            | (u32::from(input[2]) << 8)
            | u32::from(input[3]),
        (u32::from(input[4]) << 24)
            | (u32::from(input[5]) << 16)
            | (u32::from(input[6]) << 8)
            | u32::from(input[7]),
    ]
}

/// Convert two 32-bit words back to 8 bytes (big-endian).
fn unscrun(input: &[u32; 2], output: &mut [u8; 8]) {
    output[0] = (input[0] >> 24) as u8;
    output[1] = (input[0] >> 16) as u8;
    output[2] = (input[0] >> 8) as u8;
    output[3] = input[0] as u8;
    output[4] = (input[1] >> 24) as u8;
    output[5] = (input[1] >> 16) as u8;
    output[6] = (input[1] >> 8) as u8;
    output[7] = input[1] as u8;
}

/// The core DES round function with initial and final permutations.
fn desfunc(block: &mut [u32; 2], keys: &[u32; 32]) {
    let mut leftt = block[0];
    let mut right = block[1];

    // Initial permutation (IP)
    let mut work = ((leftt >> 4) ^ right) & 0x0f0f0f0f;
    right ^= work;
    leftt ^= work << 4;
    work = ((leftt >> 16) ^ right) & 0x0000ffff;
    right ^= work;
    leftt ^= work << 16;
    work = ((right >> 2) ^ leftt) & 0x33333333;
    leftt ^= work;
    right ^= work << 2;
    work = ((right >> 8) ^ leftt) & 0x00ff00ff;
    leftt ^= work;
    right ^= work << 8;
    right = right.rotate_left(1);
    work = (leftt ^ right) & 0xaaaaaaaa;
    leftt ^= work;
    right ^= work;
    leftt = leftt.rotate_left(1);

    // 16 Feistel rounds (8 iterations, 2 rounds each)
    let mut ki = 0;
    for _ in 0..8 {
        work = right.rotate_right(4);
        work ^= keys[ki];
        let mut fval = SP7[(work & 0x3f) as usize];
        fval |= SP5[((work >> 8) & 0x3f) as usize];
        fval |= SP3[((work >> 16) & 0x3f) as usize];
        fval |= SP1[((work >> 24) & 0x3f) as usize];
        work = right ^ keys[ki + 1];
        fval |= SP8[(work & 0x3f) as usize];
        fval |= SP6[((work >> 8) & 0x3f) as usize];
        fval |= SP4[((work >> 16) & 0x3f) as usize];
        fval |= SP2[((work >> 24) & 0x3f) as usize];
        leftt ^= fval;

        work = leftt.rotate_right(4);
        work ^= keys[ki + 2];
        fval = SP7[(work & 0x3f) as usize];
        fval |= SP5[((work >> 8) & 0x3f) as usize];
        fval |= SP3[((work >> 16) & 0x3f) as usize];
        fval |= SP1[((work >> 24) & 0x3f) as usize];
        work = leftt ^ keys[ki + 3];
        fval |= SP8[(work & 0x3f) as usize];
        fval |= SP6[((work >> 8) & 0x3f) as usize];
        fval |= SP4[((work >> 16) & 0x3f) as usize];
        fval |= SP2[((work >> 24) & 0x3f) as usize];
        right ^= fval;

        ki += 4;
    }

    // Final permutation (FP)
    right = right.rotate_right(1);
    work = (leftt ^ right) & 0xaaaaaaaa;
    leftt ^= work;
    right ^= work;
    leftt = leftt.rotate_right(1);
    work = ((leftt >> 8) ^ right) & 0x00ff00ff;
    right ^= work;
    leftt ^= work << 8;
    work = ((leftt >> 2) ^ right) & 0x33333333;
    right ^= work;
    leftt ^= work << 2;
    work = ((right >> 16) ^ leftt) & 0x0000ffff;
    leftt ^= work;
    right ^= work << 16;
    work = ((right >> 4) ^ leftt) & 0x0f0f0f0f;
    leftt ^= work;
    right ^= work << 4;

    block[0] = right;
    block[1] = leftt;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrunch_and_unscrun_roundtrip() {
        let input: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let words = scrunch(&input);
        assert_eq!(words[0], 0x01234567);
        assert_eq!(words[1], 0x89abcdef);

        let mut output = [0u8; 8];
        unscrun(&words, &mut output);
        assert_eq!(output, input);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key: [u8; 8] = [0x94, 0x9b, 0xe5, 0x38, 0x19, 0x2e, 0x2e, 0xfd];
        let original: [u8; 8] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];

        // Encrypt
        let enc_keys = deskey(&key, false);
        let mut block = original;
        let mut work = scrunch(&block);
        desfunc(&mut work, &enc_keys);
        unscrun(&work, &mut block);

        // Ciphertext should differ from plaintext
        assert_ne!(block, original);

        // Decrypt
        let dec = MsDes::new_decrypt(&key);
        dec.decrypt_block(&mut block);
        assert_eq!(block, original);
    }

    #[test]
    fn decrypt_ecb_pads_correctly() {
        let key: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let dec = MsDes::new_decrypt(&key);

        // 5-byte input should be padded to 8 bytes
        let short = vec![0xaa; 5];
        let result = dec.decrypt_ecb(&short);
        assert_eq!(result.len(), 8);

        // 16-byte input stays 16 bytes
        let exact = vec![0xbb; 16];
        let result = dec.decrypt_ecb(&exact);
        assert_eq!(result.len(), 16);
    }

    #[test]
    fn known_lit_drm_key_decrypts() {
        // From Black_Beauty.lit DRM debugging:
        // DES key derived from mssha1 XOR-fold
        let key: [u8; 8] = [0x94, 0x9b, 0xe5, 0x38, 0x19, 0x2e, 0x2e, 0xfd];
        // Sealed data (first 8 bytes = first DES block)
        let sealed: [u8; 16] = [
            0x78, 0xa4, 0xbb, 0x2a, 0xda, 0xc2, 0x42, 0x62, 0x3e, 0x5d, 0x49,
            0x87, 0x24, 0xb3, 0x31, 0x2c,
        ];

        let dec = MsDes::new_decrypt(&key);
        let result = dec.decrypt_ecb(&sealed);

        // First byte of correctly decrypted sealed data must be 0x00
        assert_eq!(
            result[0], 0x00,
            "MS DES decryption failed: first byte is {:#04x}, expected 0x00",
            result[0]
        );
        // Bytes 1..9 are the book key (8 non-zero bytes expected)
        assert!(
            result[1..9].iter().any(|&b| b != 0),
            "Book key is all zeros — likely wrong decryption"
        );
    }

    #[test]
    fn ms_sboxes_differ_from_standard() {
        // Verify SP1[0] is the Microsoft value, not FIPS 46-3
        assert_eq!(SP1[0], 0x02080800);
        // Standard FIPS SP1[0] would be 0x01010400
        assert_ne!(SP1[0], 0x01010400);
    }
}
