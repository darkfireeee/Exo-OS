//! Trained Q15 weights for ExoShield NGAV MLP (32->128->64->4) & Isolation Forest (8 trees).
//!
//! Binary layout in `servers/exo_shield/assets/trained_weights.bin` (little-endian):
//! - `[0..4]`: magic `b"EXML"`
//! - `[4..8]`: `TRAINED_MLP_VERSION` (`u32`)
//! - `[8..16]`: `TRAINED_MLP_CHECKSUM` (`u64`)
//! - `[16..144]`: `FEATURE_MAX` (`[i32; 32]`)
//! - `[144..16528]`: `MLP_W1` (`[i32; 4096]`)
//! - `[16528..17040]`: `MLP_B1` (`[i32; 128]`)
//! - `[17040..49808]`: `MLP_W2` (`[i32; 8192]`)
//! - `[49808..50064]`: `MLP_B2` (`[i32; 64]`)
//! - `[50064..51088]`: `MLP_W3` (`[i32; 256]`)
//! - `[51088..51104]`: `MLP_B3` (`[i32; 4]`)
//! - `[51104..51608]`: `IF_NODE_FEATURE` (`[u8; 504]`)
//! - `[51608..52616]`: `IF_NODE_THRESHOLD` (`[u16; 504]`)

const WEIGHTS_BLOB: &[u8] = include_bytes!("../../assets/trained_weights.bin");

const OFF_FEATURE_MAX: usize = 16;
const OFF_MLP_W1: usize = OFF_FEATURE_MAX + 32 * 4;
const OFF_MLP_B1: usize = OFF_MLP_W1 + 4096 * 4;
const OFF_MLP_W2: usize = OFF_MLP_B1 + 128 * 4;
const OFF_MLP_B2: usize = OFF_MLP_W2 + 8192 * 4;
const OFF_MLP_W3: usize = OFF_MLP_B2 + 64 * 4;
const OFF_MLP_B3: usize = OFF_MLP_W3 + 256 * 4;
const OFF_IF_FEATURE: usize = OFF_MLP_B3 + 4 * 4;
const OFF_IF_THRESHOLD: usize = OFF_IF_FEATURE + 504;
const EXPECTED_LEN: usize = OFF_IF_THRESHOLD + 504 * 2;

const _: () = assert!(WEIGHTS_BLOB.len() == EXPECTED_LEN);
const _: () = assert!(
    WEIGHTS_BLOB[0] == b'E'
        && WEIGHTS_BLOB[1] == b'X'
        && WEIGHTS_BLOB[2] == b'M'
        && WEIGHTS_BLOB[3] == b'L'
);

const fn read_u32_le(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

const fn read_u64_le(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        bytes[off],
        bytes[off + 1],
        bytes[off + 2],
        bytes[off + 3],
        bytes[off + 4],
        bytes[off + 5],
        bytes[off + 6],
        bytes[off + 7],
    ])
}

const fn decode_i32_array<const N: usize>(bytes: &[u8], base: usize) -> [i32; N] {
    let mut out = [0i32; N];
    let mut i = 0usize;
    while i < N {
        let off = base + i * 4;
        out[i] = i32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        i += 1;
    }
    out
}

const fn decode_u8_array<const N: usize>(bytes: &[u8], base: usize) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0usize;
    while i < N {
        out[i] = bytes[base + i];
        i += 1;
    }
    out
}

const fn decode_u16_array<const N: usize>(bytes: &[u8], base: usize) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0usize;
    while i < N {
        let off = base + i * 2;
        out[i] = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
        i += 1;
    }
    out
}

pub const TRAINED_MLP_VERSION: u32 = read_u32_le(WEIGHTS_BLOB, 4);
pub const TRAINED_MLP_CHECKSUM: u64 = read_u64_le(WEIGHTS_BLOB, 8);
pub const FEATURE_MAX: [i32; 32] = decode_i32_array(WEIGHTS_BLOB, OFF_FEATURE_MAX);
pub const MLP_W1: [i32; 4096] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_W1);
pub const MLP_B1: [i32; 128] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_B1);
pub const MLP_W2: [i32; 8192] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_W2);
pub const MLP_B2: [i32; 64] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_B2);
pub const MLP_W3: [i32; 256] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_W3);
pub const MLP_B3: [i32; 4] = decode_i32_array(WEIGHTS_BLOB, OFF_MLP_B3);
pub const IF_NODE_FEATURE: [u8; 504] = decode_u8_array(WEIGHTS_BLOB, OFF_IF_FEATURE);
pub const IF_NODE_THRESHOLD: [u16; 504] = decode_u16_array(WEIGHTS_BLOB, OFF_IF_THRESHOLD);
