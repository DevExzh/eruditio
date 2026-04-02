/// BZZ decompressor — ZP arithmetic coding + quasi-MTF + inverse BWT.
///
/// Ported from calibre's `djvubzzdec.py`, which is itself adapted from
/// DjVuLibre's `BSByteStream.cpp` and `ZPCodec.cpp`.
use crate::error::{EruditioError, Result};

const MAXBLOCK: usize = 4096 * 1024;
const FREQMAX: usize = 4;
const CTXIDS: usize = 3;

/// ZP probability table entry: (p, m, up, dn)
#[derive(Clone, Copy)]
struct ZpEntry {
    p: u16,
    m: u16,
    up: u8,
    dn: u8,
}

/// The default ZP table (256 entries). Designed for the ZP coder by running
/// `(fast-crude (steady-mat 0.0035 0.0002) 260)` in `zptable.sn`.
#[rustfmt::skip]
static ZP_TABLE: [ZpEntry; 256] = [
    ZpEntry{p:0x8000,m:0x0000,up:84,dn:145},  ZpEntry{p:0x8000,m:0x0000,up:3,dn:4},
    ZpEntry{p:0x8000,m:0x0000,up:4,dn:3},      ZpEntry{p:0x6bbd,m:0x10a5,up:5,dn:1},
    ZpEntry{p:0x6bbd,m:0x10a5,up:6,dn:2},      ZpEntry{p:0x5d45,m:0x1f28,up:7,dn:3},
    ZpEntry{p:0x5d45,m:0x1f28,up:8,dn:4},      ZpEntry{p:0x51b9,m:0x2bd3,up:9,dn:5},
    ZpEntry{p:0x51b9,m:0x2bd3,up:10,dn:6},     ZpEntry{p:0x4813,m:0x36e3,up:11,dn:7},
    ZpEntry{p:0x4813,m:0x36e3,up:12,dn:8},     ZpEntry{p:0x3fd5,m:0x408c,up:13,dn:9},
    ZpEntry{p:0x3fd5,m:0x408c,up:14,dn:10},    ZpEntry{p:0x38b1,m:0x48fd,up:15,dn:11},
    ZpEntry{p:0x38b1,m:0x48fd,up:16,dn:12},    ZpEntry{p:0x3275,m:0x505d,up:17,dn:13},
    ZpEntry{p:0x3275,m:0x505d,up:18,dn:14},    ZpEntry{p:0x2cfd,m:0x56d0,up:19,dn:15},
    ZpEntry{p:0x2cfd,m:0x56d0,up:20,dn:16},    ZpEntry{p:0x2825,m:0x5c71,up:21,dn:17},
    ZpEntry{p:0x2825,m:0x5c71,up:22,dn:18},    ZpEntry{p:0x23ab,m:0x615b,up:23,dn:19},
    ZpEntry{p:0x23ab,m:0x615b,up:24,dn:20},    ZpEntry{p:0x1f87,m:0x65a5,up:25,dn:21},
    ZpEntry{p:0x1f87,m:0x65a5,up:26,dn:22},    ZpEntry{p:0x1bbb,m:0x6962,up:27,dn:23},
    ZpEntry{p:0x1bbb,m:0x6962,up:28,dn:24},    ZpEntry{p:0x1845,m:0x6ca2,up:29,dn:25},
    ZpEntry{p:0x1845,m:0x6ca2,up:30,dn:26},    ZpEntry{p:0x1523,m:0x6f74,up:31,dn:27},
    ZpEntry{p:0x1523,m:0x6f74,up:32,dn:28},    ZpEntry{p:0x1253,m:0x71e6,up:33,dn:29},
    ZpEntry{p:0x1253,m:0x71e6,up:34,dn:30},    ZpEntry{p:0x0fcf,m:0x7404,up:35,dn:31},
    ZpEntry{p:0x0fcf,m:0x7404,up:36,dn:32},    ZpEntry{p:0x0d95,m:0x75d6,up:37,dn:33},
    ZpEntry{p:0x0d95,m:0x75d6,up:38,dn:34},    ZpEntry{p:0x0b9d,m:0x7768,up:39,dn:35},
    ZpEntry{p:0x0b9d,m:0x7768,up:40,dn:36},    ZpEntry{p:0x09e3,m:0x78c2,up:41,dn:37},
    ZpEntry{p:0x09e3,m:0x78c2,up:42,dn:38},    ZpEntry{p:0x0861,m:0x79ea,up:43,dn:39},
    ZpEntry{p:0x0861,m:0x79ea,up:44,dn:40},    ZpEntry{p:0x0711,m:0x7ae7,up:45,dn:41},
    ZpEntry{p:0x0711,m:0x7ae7,up:46,dn:42},    ZpEntry{p:0x05f1,m:0x7bbe,up:47,dn:43},
    ZpEntry{p:0x05f1,m:0x7bbe,up:48,dn:44},    ZpEntry{p:0x04f9,m:0x7c75,up:49,dn:45},
    ZpEntry{p:0x04f9,m:0x7c75,up:50,dn:46},    ZpEntry{p:0x0425,m:0x7d0f,up:51,dn:47},
    ZpEntry{p:0x0425,m:0x7d0f,up:52,dn:48},    ZpEntry{p:0x0371,m:0x7d91,up:53,dn:49},
    ZpEntry{p:0x0371,m:0x7d91,up:54,dn:50},    ZpEntry{p:0x02d9,m:0x7dfe,up:55,dn:51},
    ZpEntry{p:0x02d9,m:0x7dfe,up:56,dn:52},    ZpEntry{p:0x0259,m:0x7e5a,up:57,dn:53},
    ZpEntry{p:0x0259,m:0x7e5a,up:58,dn:54},    ZpEntry{p:0x01ed,m:0x7ea6,up:59,dn:55},
    ZpEntry{p:0x01ed,m:0x7ea6,up:60,dn:56},    ZpEntry{p:0x0193,m:0x7ee6,up:61,dn:57},
    ZpEntry{p:0x0193,m:0x7ee6,up:62,dn:58},    ZpEntry{p:0x0149,m:0x7f1a,up:63,dn:59},
    ZpEntry{p:0x0149,m:0x7f1a,up:64,dn:60},    ZpEntry{p:0x010b,m:0x7f45,up:65,dn:61},
    ZpEntry{p:0x010b,m:0x7f45,up:66,dn:62},    ZpEntry{p:0x00d5,m:0x7f6b,up:67,dn:63},
    ZpEntry{p:0x00d5,m:0x7f6b,up:68,dn:64},    ZpEntry{p:0x00a5,m:0x7f8d,up:69,dn:65},
    ZpEntry{p:0x00a5,m:0x7f8d,up:70,dn:66},    ZpEntry{p:0x007b,m:0x7faa,up:71,dn:67},
    ZpEntry{p:0x007b,m:0x7faa,up:72,dn:68},    ZpEntry{p:0x0057,m:0x7fc3,up:73,dn:69},
    ZpEntry{p:0x0057,m:0x7fc3,up:74,dn:70},    ZpEntry{p:0x003b,m:0x7fd7,up:75,dn:71},
    ZpEntry{p:0x003b,m:0x7fd7,up:76,dn:72},    ZpEntry{p:0x0023,m:0x7fe7,up:77,dn:73},
    ZpEntry{p:0x0023,m:0x7fe7,up:78,dn:74},    ZpEntry{p:0x0013,m:0x7ff2,up:79,dn:75},
    ZpEntry{p:0x0013,m:0x7ff2,up:80,dn:76},    ZpEntry{p:0x0007,m:0x7ffa,up:81,dn:77},
    ZpEntry{p:0x0007,m:0x7ffa,up:82,dn:78},    ZpEntry{p:0x0001,m:0x7fff,up:81,dn:79},
    ZpEntry{p:0x0001,m:0x7fff,up:82,dn:80},    ZpEntry{p:0x5695,m:0x0000,up:9,dn:85},
    ZpEntry{p:0x24ee,m:0x0000,up:86,dn:226},   ZpEntry{p:0x8000,m:0x0000,up:5,dn:6},
    ZpEntry{p:0x0d30,m:0x0000,up:88,dn:176},   ZpEntry{p:0x481a,m:0x0000,up:89,dn:143},
    ZpEntry{p:0x0481,m:0x0000,up:90,dn:138},   ZpEntry{p:0x3579,m:0x0000,up:91,dn:141},
    ZpEntry{p:0x017a,m:0x0000,up:92,dn:112},   ZpEntry{p:0x24ef,m:0x0000,up:93,dn:135},
    ZpEntry{p:0x007b,m:0x0000,up:94,dn:104},   ZpEntry{p:0x1978,m:0x0000,up:95,dn:133},
    ZpEntry{p:0x0028,m:0x0000,up:96,dn:100},   ZpEntry{p:0x10ca,m:0x0000,up:97,dn:129},
    ZpEntry{p:0x000d,m:0x0000,up:82,dn:98},    ZpEntry{p:0x0b5d,m:0x0000,up:99,dn:127},
    ZpEntry{p:0x0034,m:0x0000,up:76,dn:72},    ZpEntry{p:0x078a,m:0x0000,up:101,dn:125},
    ZpEntry{p:0x00a0,m:0x0000,up:70,dn:102},   ZpEntry{p:0x050f,m:0x0000,up:103,dn:123},
    ZpEntry{p:0x0117,m:0x0000,up:66,dn:60},    ZpEntry{p:0x0358,m:0x0000,up:105,dn:121},
    ZpEntry{p:0x01ea,m:0x0000,up:106,dn:110},  ZpEntry{p:0x0234,m:0x0000,up:107,dn:119},
    ZpEntry{p:0x0144,m:0x0000,up:66,dn:108},   ZpEntry{p:0x0173,m:0x0000,up:109,dn:117},
    ZpEntry{p:0x0234,m:0x0000,up:60,dn:54},    ZpEntry{p:0x00f5,m:0x0000,up:111,dn:115},
    ZpEntry{p:0x0353,m:0x0000,up:56,dn:48},    ZpEntry{p:0x00a1,m:0x0000,up:69,dn:113},
    ZpEntry{p:0x05c5,m:0x0000,up:114,dn:134},  ZpEntry{p:0x011a,m:0x0000,up:65,dn:59},
    ZpEntry{p:0x03cf,m:0x0000,up:116,dn:132},  ZpEntry{p:0x01aa,m:0x0000,up:61,dn:55},
    ZpEntry{p:0x0285,m:0x0000,up:118,dn:130},  ZpEntry{p:0x0286,m:0x0000,up:57,dn:51},
    ZpEntry{p:0x01ab,m:0x0000,up:120,dn:128},  ZpEntry{p:0x03d3,m:0x0000,up:53,dn:47},
    ZpEntry{p:0x011a,m:0x0000,up:122,dn:126},  ZpEntry{p:0x05c5,m:0x0000,up:49,dn:41},
    ZpEntry{p:0x00ba,m:0x0000,up:124,dn:62},   ZpEntry{p:0x08ad,m:0x0000,up:43,dn:37},
    ZpEntry{p:0x007a,m:0x0000,up:72,dn:66},    ZpEntry{p:0x0ccc,m:0x0000,up:39,dn:31},
    ZpEntry{p:0x01eb,m:0x0000,up:60,dn:54},    ZpEntry{p:0x1302,m:0x0000,up:33,dn:25},
    ZpEntry{p:0x02e6,m:0x0000,up:56,dn:50},    ZpEntry{p:0x1b81,m:0x0000,up:29,dn:131},
    ZpEntry{p:0x045e,m:0x0000,up:52,dn:46},    ZpEntry{p:0x24ef,m:0x0000,up:23,dn:17},
    ZpEntry{p:0x0690,m:0x0000,up:48,dn:40},    ZpEntry{p:0x2865,m:0x0000,up:23,dn:15},
    ZpEntry{p:0x09de,m:0x0000,up:42,dn:136},   ZpEntry{p:0x3987,m:0x0000,up:137,dn:7},
    ZpEntry{p:0x0dc8,m:0x0000,up:38,dn:32},    ZpEntry{p:0x2c99,m:0x0000,up:21,dn:139},
    ZpEntry{p:0x10ca,m:0x0000,up:140,dn:172},  ZpEntry{p:0x3b5f,m:0x0000,up:15,dn:9},
    ZpEntry{p:0x0b5d,m:0x0000,up:142,dn:170},  ZpEntry{p:0x5695,m:0x0000,up:9,dn:85},
    ZpEntry{p:0x078a,m:0x0000,up:144,dn:168},  ZpEntry{p:0x8000,m:0x0000,up:141,dn:248},
    ZpEntry{p:0x050f,m:0x0000,up:146,dn:166},  ZpEntry{p:0x24ee,m:0x0000,up:147,dn:247},
    ZpEntry{p:0x0358,m:0x0000,up:148,dn:164},  ZpEntry{p:0x0d30,m:0x0000,up:149,dn:197},
    ZpEntry{p:0x0234,m:0x0000,up:150,dn:162},  ZpEntry{p:0x0481,m:0x0000,up:151,dn:95},
    ZpEntry{p:0x0173,m:0x0000,up:152,dn:160},  ZpEntry{p:0x017a,m:0x0000,up:153,dn:173},
    ZpEntry{p:0x00f5,m:0x0000,up:154,dn:158},  ZpEntry{p:0x007b,m:0x0000,up:155,dn:165},
    ZpEntry{p:0x00a1,m:0x0000,up:70,dn:156},   ZpEntry{p:0x0028,m:0x0000,up:157,dn:161},
    ZpEntry{p:0x011a,m:0x0000,up:66,dn:60},    ZpEntry{p:0x000d,m:0x0000,up:81,dn:159},
    ZpEntry{p:0x01aa,m:0x0000,up:62,dn:56},    ZpEntry{p:0x0034,m:0x0000,up:75,dn:71},
    ZpEntry{p:0x0286,m:0x0000,up:58,dn:52},    ZpEntry{p:0x00a0,m:0x0000,up:69,dn:163},
    ZpEntry{p:0x03d3,m:0x0000,up:54,dn:48},    ZpEntry{p:0x0117,m:0x0000,up:65,dn:59},
    ZpEntry{p:0x05c5,m:0x0000,up:50,dn:42},    ZpEntry{p:0x01ea,m:0x0000,up:167,dn:171},
    ZpEntry{p:0x08ad,m:0x0000,up:44,dn:38},    ZpEntry{p:0x0144,m:0x0000,up:65,dn:169},
    ZpEntry{p:0x0ccc,m:0x0000,up:40,dn:32},    ZpEntry{p:0x0234,m:0x0000,up:59,dn:53},
    ZpEntry{p:0x1302,m:0x0000,up:34,dn:26},    ZpEntry{p:0x0353,m:0x0000,up:55,dn:47},
    ZpEntry{p:0x1b81,m:0x0000,up:30,dn:174},   ZpEntry{p:0x05c5,m:0x0000,up:175,dn:193},
    ZpEntry{p:0x24ef,m:0x0000,up:24,dn:18},    ZpEntry{p:0x03cf,m:0x0000,up:177,dn:191},
    ZpEntry{p:0x2b74,m:0x0000,up:178,dn:222},  ZpEntry{p:0x0285,m:0x0000,up:179,dn:189},
    ZpEntry{p:0x201d,m:0x0000,up:180,dn:218},  ZpEntry{p:0x01ab,m:0x0000,up:181,dn:187},
    ZpEntry{p:0x1715,m:0x0000,up:182,dn:216},  ZpEntry{p:0x011a,m:0x0000,up:183,dn:185},
    ZpEntry{p:0x0fb7,m:0x0000,up:184,dn:214},  ZpEntry{p:0x00ba,m:0x0000,up:69,dn:61},
    ZpEntry{p:0x0a67,m:0x0000,up:186,dn:212},  ZpEntry{p:0x01eb,m:0x0000,up:59,dn:53},
    ZpEntry{p:0x06e7,m:0x0000,up:188,dn:210},  ZpEntry{p:0x02e6,m:0x0000,up:55,dn:49},
    ZpEntry{p:0x0496,m:0x0000,up:190,dn:208},  ZpEntry{p:0x045e,m:0x0000,up:51,dn:45},
    ZpEntry{p:0x030d,m:0x0000,up:192,dn:206},  ZpEntry{p:0x0690,m:0x0000,up:47,dn:39},
    ZpEntry{p:0x0206,m:0x0000,up:194,dn:204},  ZpEntry{p:0x09de,m:0x0000,up:41,dn:195},
    ZpEntry{p:0x0155,m:0x0000,up:196,dn:202},  ZpEntry{p:0x0dc8,m:0x0000,up:37,dn:31},
    ZpEntry{p:0x00e1,m:0x0000,up:198,dn:200},  ZpEntry{p:0x2b74,m:0x0000,up:199,dn:243},
    ZpEntry{p:0x0094,m:0x0000,up:72,dn:64},    ZpEntry{p:0x201d,m:0x0000,up:201,dn:239},
    ZpEntry{p:0x0188,m:0x0000,up:62,dn:56},    ZpEntry{p:0x1715,m:0x0000,up:203,dn:237},
    ZpEntry{p:0x0252,m:0x0000,up:58,dn:52},    ZpEntry{p:0x0fb7,m:0x0000,up:205,dn:235},
    ZpEntry{p:0x0383,m:0x0000,up:54,dn:48},    ZpEntry{p:0x0a67,m:0x0000,up:207,dn:233},
    ZpEntry{p:0x0547,m:0x0000,up:50,dn:44},    ZpEntry{p:0x06e7,m:0x0000,up:209,dn:231},
    ZpEntry{p:0x07e2,m:0x0000,up:46,dn:38},    ZpEntry{p:0x0496,m:0x0000,up:211,dn:229},
    ZpEntry{p:0x0bc0,m:0x0000,up:40,dn:34},    ZpEntry{p:0x030d,m:0x0000,up:213,dn:227},
    ZpEntry{p:0x1178,m:0x0000,up:36,dn:28},    ZpEntry{p:0x0206,m:0x0000,up:215,dn:225},
    ZpEntry{p:0x19da,m:0x0000,up:30,dn:22},    ZpEntry{p:0x0155,m:0x0000,up:217,dn:223},
    ZpEntry{p:0x24ef,m:0x0000,up:26,dn:16},    ZpEntry{p:0x00e1,m:0x0000,up:219,dn:221},
    ZpEntry{p:0x320e,m:0x0000,up:20,dn:220},   ZpEntry{p:0x0094,m:0x0000,up:71,dn:63},
    ZpEntry{p:0x432a,m:0x0000,up:14,dn:8},     ZpEntry{p:0x0188,m:0x0000,up:61,dn:55},
    ZpEntry{p:0x447d,m:0x0000,up:14,dn:224},   ZpEntry{p:0x0252,m:0x0000,up:57,dn:51},
    ZpEntry{p:0x5ece,m:0x0000,up:8,dn:2},      ZpEntry{p:0x0383,m:0x0000,up:53,dn:47},
    ZpEntry{p:0x8000,m:0x0000,up:228,dn:87},   ZpEntry{p:0x0547,m:0x0000,up:49,dn:43},
    ZpEntry{p:0x481a,m:0x0000,up:230,dn:246},  ZpEntry{p:0x07e2,m:0x0000,up:45,dn:37},
    ZpEntry{p:0x3579,m:0x0000,up:232,dn:244},  ZpEntry{p:0x0bc0,m:0x0000,up:39,dn:33},
    ZpEntry{p:0x24ef,m:0x0000,up:234,dn:238},  ZpEntry{p:0x1178,m:0x0000,up:35,dn:27},
    ZpEntry{p:0x1978,m:0x0000,up:138,dn:236},  ZpEntry{p:0x19da,m:0x0000,up:29,dn:21},
    ZpEntry{p:0x2865,m:0x0000,up:24,dn:16},    ZpEntry{p:0x24ef,m:0x0000,up:25,dn:15},
    ZpEntry{p:0x3987,m:0x0000,up:240,dn:8},    ZpEntry{p:0x320e,m:0x0000,up:19,dn:241},
    ZpEntry{p:0x2c99,m:0x0000,up:22,dn:242},   ZpEntry{p:0x432a,m:0x0000,up:13,dn:7},
    ZpEntry{p:0x3b5f,m:0x0000,up:16,dn:10},    ZpEntry{p:0x447d,m:0x0000,up:13,dn:245},
    ZpEntry{p:0x5695,m:0x0000,up:10,dn:2},     ZpEntry{p:0x5ece,m:0x0000,up:7,dn:1},
    ZpEntry{p:0x8000,m:0x0000,up:244,dn:83},   ZpEntry{p:0x8000,m:0x0000,up:249,dn:250},
    ZpEntry{p:0x5695,m:0x0000,up:10,dn:2},     ZpEntry{p:0x481a,m:0x0000,up:89,dn:143},
    ZpEntry{p:0x481a,m:0x0000,up:230,dn:246},  ZpEntry{p:0,m:0,up:0,dn:0},
    ZpEntry{p:0,m:0,up:0,dn:0},                ZpEntry{p:0,m:0,up:0,dn:0},
    ZpEntry{p:0,m:0,up:0,dn:0},                ZpEntry{p:0,m:0,up:0,dn:0},
];

struct BzzDecoder<'a> {
    data: &'a [u8],
    inptr: usize,
    a: u32,
    code: u32,
    fence: u32,
    bufint: u32,
    scount: i32,
    delay: i32,
    ctx: [u8; 300],
    ffzt: [u8; 256],
}

impl<'a> BzzDecoder<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        let mut dec = Self {
            data,
            inptr: 0,
            a: 0,
            code: 0,
            fence: 0,
            bufint: 0,
            scount: 0,
            delay: 25,
            ctx: [0; 300],
            ffzt: [0; 256],
        };

        // Build machine-independent FFZ (find-first-zero) table
        for i in 0..256u32 {
            let mut j = i;
            while j & 0x80 != 0 {
                dec.ffzt[i as usize] += 1;
                j <<= 1;
            }
        }

        // Read first 16 bits of code
        let b0 = dec.read_byte().unwrap_or(0xFF) as u32;
        let b1 = dec.read_byte().unwrap_or(0xFF) as u32;
        dec.code = (b0 << 8) | b1;

        // Preload buffer
        dec.preload()?;

        // Compute initial fence
        dec.fence = dec.code;
        if dec.code >= 0x8000 {
            dec.fence = 0x7FFF;
        }

        Ok(dec)
    }

    fn read_byte(&mut self) -> Option<u8> {
        if self.inptr < self.data.len() {
            let b = self.data[self.inptr];
            self.inptr += 1;
            Some(b)
        } else {
            None
        }
    }

    fn preload(&mut self) -> Result<()> {
        while self.scount <= 24 {
            let byte = match self.read_byte() {
                Some(b) => b as u32,
                None => {
                    self.delay -= 1;
                    if self.delay < 1 {
                        return Err(EruditioError::Format(
                            "BZZ: unexpected end of stream".into(),
                        ));
                    }
                    0xFF
                }
            };
            self.bufint = (self.bufint << 8) | byte;
            self.scount += 8;
        }
        Ok(())
    }

    fn ffz(&self) -> u32 {
        let x = self.a;
        if x >= 0xFF00 {
            self.ffzt[(x & 0xFF) as usize] as u32 + 8
        } else {
            self.ffzt[((x >> 8) & 0xFF) as usize] as u32
        }
    }

    fn decode_sub_simple(&mut self, mps: u32, z: u32) -> Result<u32> {
        if z > self.code {
            // LPS branch
            let z_inv = 0x10000 - z;
            self.a += z_inv;
            self.code += z_inv;
            let shift = self.ffz();
            self.scount -= shift as i32;
            self.a = (self.a << shift) & 0xFFFF;
            self.code = ((self.code << shift)
                | ((self.bufint >> self.scount as u32) & ((1 << shift) - 1)))
                & 0xFFFF;
            if self.scount < 16 {
                self.preload()?;
            }
            self.fence = self.code;
            if self.code >= 0x8000 {
                self.fence = 0x7FFF;
            }
            Ok(mps ^ 1)
        } else {
            // MPS branch
            self.scount -= 1;
            self.a = (z << 1) & 0xFFFF;
            self.code =
                ((self.code << 1) | ((self.bufint >> self.scount as u32) & 1)) & 0xFFFF;
            if self.scount < 16 {
                self.preload()?;
            }
            self.fence = self.code;
            if self.code >= 0x8000 {
                self.fence = 0x7FFF;
            }
            Ok(mps)
        }
    }

    fn decode_sub(&mut self, ctx: usize, z: u32) -> Result<u32> {
        let bit = (self.ctx[ctx] & 1) as u32;
        let d = 0x6000 + ((z + self.a) >> 2);
        let z = z.min(d);

        if z > self.code {
            // LPS branch
            let z_inv = 0x10000 - z;
            self.a += z_inv;
            self.code += z_inv;
            self.ctx[ctx] = ZP_TABLE[self.ctx[ctx] as usize].dn;
            let shift = self.ffz();
            self.scount -= shift as i32;
            self.a = (self.a << shift) & 0xFFFF;
            self.code = ((self.code << shift)
                | ((self.bufint >> self.scount as u32) & ((1 << shift) - 1)))
                & 0xFFFF;
            if self.scount < 16 {
                self.preload()?;
            }
            self.fence = self.code;
            if self.code >= 0x8000 {
                self.fence = 0x7FFF;
            }
            Ok(bit ^ 1)
        } else {
            // MPS branch
            let entry = &ZP_TABLE[self.ctx[ctx] as usize];
            if self.a >= entry.m as u32 {
                self.ctx[ctx] = entry.up;
            }
            self.scount -= 1;
            self.a = (z << 1) & 0xFFFF;
            self.code =
                ((self.code << 1) | ((self.bufint >> self.scount as u32) & 1)) & 0xFFFF;
            if self.scount < 16 {
                self.preload()?;
            }
            self.fence = self.code;
            if self.code >= 0x8000 {
                self.fence = 0x7FFF;
            }
            Ok(bit)
        }
    }

    fn zpcodec_decoder(&mut self) -> Result<u32> {
        self.decode_sub_simple(0, 0x8000 + (self.a >> 1))
    }

    fn zpcodec_decode(&mut self, ctx: usize) -> Result<u32> {
        let z = self.a + ZP_TABLE[self.ctx[ctx] as usize].p as u32;
        if z <= self.fence {
            self.a = z;
            Ok((self.ctx[ctx] & 1) as u32)
        } else {
            self.decode_sub(ctx, z)
        }
    }

    fn decode_raw(&mut self, bits: u32) -> Result<u32> {
        let mut n: u32 = 1;
        let m: u32 = 1 << bits;
        while n < m {
            let b = self.zpcodec_decoder()?;
            n = (n << 1) | b;
        }
        Ok(n - m)
    }

    fn decode_binary(&mut self, index: usize, bits: u32) -> Result<u32> {
        let mut n: u32 = 1;
        let m: u32 = 1 << bits;
        while n < m {
            let b = self.zpcodec_decode(index + (n as usize) - 1)?;
            n = (n << 1) | b;
        }
        Ok(n - m)
    }

    fn decode_block(&mut self) -> Result<Option<Vec<u8>>> {
        let xsize = self.decode_raw(24)? as usize;
        if xsize == 0 {
            return Ok(None);
        }
        if xsize > MAXBLOCK {
            return Err(EruditioError::Format("BZZ: block size too large".into()));
        }

        // Decode estimation speed (fshift)
        let mut fshift: u32 = 0;
        if self.zpcodec_decoder()? != 0 {
            fshift += 1;
            if self.zpcodec_decoder()? != 0 {
                fshift += 1;
            }
        }

        // Prepare quasi-MTF
        let mut mtf: [u8; 256] = [0; 256];
        for (i, slot) in mtf.iter_mut().enumerate() {
            *slot = i as u8;
        }
        let mut freq = [0u32; FREQMAX];
        let mut fadd: u32 = 4;

        let mut outbuf = vec![0u8; xsize];
        let mut mtfno: u32 = 3;
        let mut markerpos: Option<usize> = None;

        #[allow(clippy::needless_range_loop)]
        for i in 0..xsize {
            let ctxid = (CTXIDS - 1).min(mtfno as usize);

            if self.zpcodec_decode(ctxid)? != 0 {
                mtfno = 0;
                outbuf[i] = mtf[0];
            } else if self.zpcodec_decode(ctxid + CTXIDS)? != 0 {
                mtfno = 1;
                outbuf[i] = mtf[1];
            } else if self.zpcodec_decode(2 * CTXIDS)? != 0 {
                mtfno = 2 + self.decode_binary(2 * CTXIDS + 1, 1)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 2)? != 0 {
                mtfno = 4 + self.decode_binary(2 * CTXIDS + 3, 2)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 6)? != 0 {
                mtfno = 8 + self.decode_binary(2 * CTXIDS + 7, 3)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 14)? != 0 {
                mtfno = 16 + self.decode_binary(2 * CTXIDS + 15, 4)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 30)? != 0 {
                mtfno = 32 + self.decode_binary(2 * CTXIDS + 31, 5)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 62)? != 0 {
                mtfno = 64 + self.decode_binary(2 * CTXIDS + 63, 6)?;
                outbuf[i] = mtf[mtfno as usize];
            } else if self.zpcodec_decode(2 * CTXIDS + 126)? != 0 {
                mtfno = 128 + self.decode_binary(2 * CTXIDS + 127, 7)?;
                outbuf[i] = mtf[mtfno as usize];
            } else {
                // EOB marker (mtfno = 256)
                mtfno = 256;
                outbuf[i] = 0;
                markerpos = Some(i);
                continue;
            }

            // Quasi-MTF rotation by empirical frequency
            fadd += fadd >> fshift;
            if fadd > 0x1000_0000 {
                fadd >>= 24;
                for f in freq.iter_mut() {
                    *f >>= 24;
                }
            }

            let mut fc = fadd;
            if (mtfno as usize) < FREQMAX {
                fc += freq[mtfno as usize];
            }

            let mut k = mtfno as usize;
            while k >= FREQMAX {
                mtf[k] = mtf[k - 1];
                k -= 1;
            }
            while k > 0 && fc >= freq[k - 1] {
                mtf[k] = mtf[k - 1];
                freq[k] = freq[k - 1];
                k -= 1;
            }
            mtf[k] = outbuf[i];
            freq[k] = fc;
        }

        // Reconstruct string via inverse BWT
        let markerpos = markerpos.ok_or_else(|| {
            EruditioError::Format("BZZ: corrupt block — no marker position".into())
        })?;

        if markerpos < 1 || markerpos >= xsize {
            return Err(EruditioError::Format("BZZ: corrupt block — invalid marker".into()));
        }

        // Build position chain
        let mut posn = vec![0u32; xsize];
        let mut count = [0u32; 256];

        for i in 0..markerpos {
            let c = outbuf[i] as usize;
            posn[i] = ((c as u32) << 24) | (count[c] & 0xFF_FFFF);
            count[c] += 1;
        }
        for i in (markerpos + 1)..xsize {
            let c = outbuf[i] as usize;
            posn[i] = ((c as u32) << 24) | (count[c] & 0xFF_FFFF);
            count[c] += 1;
        }

        // Compute sorted character positions
        let mut last: u32 = 1;
        for c in count.iter_mut() {
            let tmp = *c;
            *c = last;
            last += tmp;
        }

        // Undo the sort transform
        let mut idx: usize = 0;
        let mut remaining = xsize - 1;
        while remaining > 0 {
            let n = posn[idx];
            let c = (n >> 24) as u8;
            remaining -= 1;
            outbuf[remaining] = c;
            idx = (count[c as usize] + (n & 0xFF_FFFF)) as usize;
        }

        if idx != markerpos {
            return Err(EruditioError::Format("BZZ: corrupt block — BWT verify failed".into()));
        }

        Ok(Some(outbuf))
    }
}

/// Decompress BZZ-encoded data, returning the decoded bytes.
pub fn bzz_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut decoder = BzzDecoder::new(data)?;
    let mut output = Vec::new();

    while let Some(block) = decoder.decode_block()? {
        output.extend_from_slice(&block);
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        let result = bzz_decompress(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn ffz_table_first_entries() {
        let dec = BzzDecoder::new(&[0, 0]).unwrap();
        // 0x00 has no leading 1-bits → ffzt[0] = 0
        assert_eq!(dec.ffzt[0], 0);
        // 0xFF has 8 leading 1-bits → ffzt[255] = 8
        assert_eq!(dec.ffzt[255], 8);
        // 0x80 has 1 leading 1-bit → ffzt[128] = 1
        assert_eq!(dec.ffzt[128], 1);
        // 0xC0 = 0b11000000 has 2 leading 1-bits
        assert_eq!(dec.ffzt[0xC0], 2);
    }

    #[test]
    fn zp_table_has_correct_size() {
        assert_eq!(ZP_TABLE.len(), 256);
        // Entry 0 should be p=0x8000, m=0x0000
        assert_eq!(ZP_TABLE[0].p, 0x8000);
        assert_eq!(ZP_TABLE[0].m, 0x0000);
    }
}
