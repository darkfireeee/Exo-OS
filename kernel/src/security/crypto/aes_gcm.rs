// kernel/src/security/crypto/aes_gcm.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// AES-256-GCM — Software Implementation + AES-NI Runtime Detection
// ═══════════════════════════════════════════════════════════════════════════════
//
// Full AES-256-GCM implementation conforming to NIST SP 800-38D:
//   • Software AES-256 via internal software rounds (no SIMD/SSE2)
//   • GCM mode with GHASH multiplication (pure u64 arithmetic, no SIMD)
//   • AES-NI runtime detection (CPUID via CPU_FEATURES)
//   • If AES-NI available → hardware acceleration (inline asm .byte)
//   • If AES-NI absent → software path (T-tables)
//   • NEVER NotAvailableOnThisTarget
//
// TARGET CONSTRAINT: aes-gcm crate DOES NOT COMPILE on x86_64-unknown-none
// (polyval/ghash generates 128-bit operations → LLVM ERROR: split 128-bit).
// Solution: use internal software AES-256 (SubBytes/ShiftRows/MixColumns)
// + manually implemented GCM with GHASH in pure u64 arithmetic.
//
// RULE CRYPTO-CRATES: bare-metal kernel path must remain soft pure-Rust.
// GCM/GHASH is manually implemented because aes-gcm crate is incompatible
// with target. GHASH only uses u64 operations (no SIMD).
//
// REFERENCES:
//   NIST SP 800-38D (GCM Specification)
//   NIST SP 800-38A (AES Specification)
//   Intel Carry-Less Multiplication Instruction (White Paper)
// ═══════════════════════════════════════════════════════════════════════════════

use subtle::ConstantTimeEq;

use crate::arch::x86_64::cpu::features::cpu_features_or_none;

// ─────────────────────────────────────────────────────────────────────────────
// Public constants
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256 key length (32 bytes).
pub const AES_KEY_LEN: usize = 32;
/// AES-GCM nonce length (12 bytes = 96 bits, NIST recommended).
pub const AES_GCM_NONCE_LEN: usize = 12;
/// GCM tag length (16 bytes = 128 bits).
pub const AES_GCM_TAG_LEN: usize = 16;

/// AES block size (16 bytes = 128 bits).
const BLOCK_SIZE: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// AES-GCM Error
// ─────────────────────────────────────────────────────────────────────────────

/// AES-GCM error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AesGcmError {
    /// Authentication failed (invalid tag or corrupted data).
    AuthenticationFailed,
    /// Invalid parameter (incorrect length).
    InvalidParameter,
}

impl core::fmt::Display for AesGcmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AesGcmError::AuthenticationFailed => write!(f, "AES-256-GCM: authentication failed"),
            AesGcmError::InvalidParameter => write!(f, "AES-256-GCM: invalid parameter"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AES S-Box (for AES-NI path key expansion)
// ─────────────────────────────────────────────────────────────────────────────

/// S-Box AES standard (FIPS 197).
const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Round constants for AES-256 key expansion.
const AES_RCON: [u32; 7] = [
    0x0100_0000,
    0x0200_0000,
    0x0400_0000,
    0x0800_0000,
    0x1000_0000,
    0x2000_0000,
    0x4000_0000,
];

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 Key Expansion (for AES-NI path)
// ─────────────────────────────────────────────────────────────────────────────

/// Round keys for AES-256 (15 × 16 bytes = 240 bytes).
struct Aes256RoundKeys {
    keys: [[u8; 16]; 15],
}

/// SubWord: applies S-Box to each byte of a 32-bit word.
fn sub_word(w: u32) -> u32 {
    let b0 = AES_SBOX[((w >> 24) & 0xFF) as usize] as u32;
    let b1 = AES_SBOX[((w >> 16) & 0xFF) as usize] as u32;
    let b2 = AES_SBOX[((w >> 8) & 0xFF) as usize] as u32;
    let b3 = AES_SBOX[((w) & 0xFF) as usize] as u32;
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

/// RotWord: 8-bit left rotation of a 32-bit word.
#[inline]
fn rot_word(w: u32) -> u32 {
    w.rotate_left(8)
}

/// Standard AES-256 key expansion (FIPS 197 §5.2).
///
/// Produces 15 round keys (60 words) from 256-bit key.
fn aes256_expand_key(key: &[u8; 32]) -> Aes256RoundKeys {
    let mut w = [0u32; 60];

    // Initialize first 8 words from key
    for i in 0..8 {
        w[i] = u32::from_be_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }

    // Expansion
    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            temp = sub_word(rot_word(temp)) ^ AES_RCON[i / 8 - 1];
        } else if i % 8 == 4 {
            temp = sub_word(temp);
        }
        w[i] = w[i - 8] ^ temp;
    }

    // Convertir les mots en round keys
    let mut keys = [[0u8; 16]; 15];
    for i in 0..15 {
        for j in 0..4 {
            keys[i][j * 4..j * 4 + 4].copy_from_slice(&w[i * 4 + j].to_be_bytes());
        }
    }

    Aes256RoundKeys { keys }
}

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 Encryption — Software path
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn aes_add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for i in 0..BLOCK_SIZE {
        state[i] ^= round_key[i];
    }
}

#[inline]
fn aes_sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = AES_SBOX[*byte as usize];
    }
}

#[inline]
fn aes_shift_rows(state: &mut [u8; 16]) {
    let original = *state;
    state[0] = original[0];
    state[1] = original[5];
    state[2] = original[10];
    state[3] = original[15];
    state[4] = original[4];
    state[5] = original[9];
    state[6] = original[14];
    state[7] = original[3];
    state[8] = original[8];
    state[9] = original[13];
    state[10] = original[2];
    state[11] = original[7];
    state[12] = original[12];
    state[13] = original[1];
    state[14] = original[6];
    state[15] = original[11];
}

#[inline]
fn aes_xtime(x: u8) -> u8 {
    if x & 0x80 != 0 {
        (x << 1) ^ 0x1B
    } else {
        x << 1
    }
}

#[inline]
fn aes_mul2(x: u8) -> u8 {
    aes_xtime(x)
}

#[inline]
fn aes_mul3(x: u8) -> u8 {
    aes_mul2(x) ^ x
}

#[inline]
fn aes_mix_single_column(col: &mut [u8; 4]) {
    let a0 = col[0];
    let a1 = col[1];
    let a2 = col[2];
    let a3 = col[3];

    col[0] = aes_mul2(a0) ^ aes_mul3(a1) ^ a2 ^ a3;
    col[1] = a0 ^ aes_mul2(a1) ^ aes_mul3(a2) ^ a3;
    col[2] = a0 ^ a1 ^ aes_mul2(a2) ^ aes_mul3(a3);
    col[3] = aes_mul3(a0) ^ a1 ^ a2 ^ aes_mul2(a3);
}

#[inline]
fn aes_mix_columns(state: &mut [u8; 16]) {
    for col_idx in 0..4 {
        let start = col_idx * 4;
        let mut col = [
            state[start],
            state[start + 1],
            state[start + 2],
            state[start + 3],
        ];
        aes_mix_single_column(&mut col);
        state[start..start + 4].copy_from_slice(&col);
    }
}

/// Chiffre un bloc 16 octets via le chemin software pure-Rust.
fn aes256_encrypt_block_sw(round_keys: &Aes256RoundKeys, block: &mut [u8; 16]) {
    aes_add_round_key(block, &round_keys.keys[0]);
    for round in 1..14 {
        aes_sub_bytes(block);
        aes_shift_rows(block);
        aes_mix_columns(block);
        aes_add_round_key(block, &round_keys.keys[round]);
    }
    aes_sub_bytes(block);
    aes_shift_rows(block);
    aes_add_round_key(block, &round_keys.keys[14]);
}

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 Encryption — AES-NI hardware path (inline assembly)
// ─────────────────────────────────────────────────────────────────────────────

/// Encrypts a 16-byte block via AES-NI (hardware acceleration).
///
/// Uses inline assembly with .byte encoding for SSE/AES-NI instructions,
/// in order to bypass the LLVM restriction on x86_64-unknown-none target
/// (no SSE2 in target features).
///
/// XMM registers are manually saved and restored to
/// avoid corruption of current thread FPU state.
///
/// # Safety
/// - Must be called only when AES-NI is available
/// - `block` must point to 16 readable/writable bytes
/// - `round_keys` must point to 240 readable bytes (15 × 16)
#[cfg(target_arch = "x86_64")]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn aes256_encrypt_block_ni(block: *mut u8, round_keys: *const u8) {
    // Encodes AES-NI instructions as .byte to avoid LLVM.
    //
    // Registers used:
    //   rdi = pointer to block (in/out)
    //   rsi = pointer to round keys
    //   rax = current pointer to round keys
    //   xmm0 = AES state
    //   xmm1 = temporary round key
    //
    // Encoded instructions:
    //   movdqu xmm0, [rdi]       = F3 0F 6F 07
    //   movdqu xmm1, [rax]       = F3 0F 6F 08
    //   pxor   xmm0, xmm1       = 66 0F EF C1
    //   aesenc xmm0, xmm1       = 66 0F 38 DC C1
    //   aesenclast xmm0, xmm1   = 66 0F 38 DD C1
    //   movdqu [rdi], xmm0       = F3 0F 7F 07
    //   movdqu [rsp], xmm0      = F3 0F 7F 04 24
    //   movdqu [rsp+16], xmm1   = F3 0F 7F 4C 24 10
    //   movdqu xmm0, [rsp]      = F3 0F 6F 04 24
    //   movdqu xmm1, [rsp+16]   = F3 0F 6F 4C 24 10

    core::arch::asm!(
        // ── Save XMM0 and XMM1 on stack ──────────────────────────────
        "sub rsp, 32",
        // movdqu [rsp], xmm0
        ".byte 0xF3, 0x0F, 0x7F, 0x04, 0x24",
        // movdqu [rsp+16], xmm1
        ".byte 0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x10",

        // ── Load plaintext into xmm0 ─────────────────────────────────
        // movdqu xmm0, [rdi]
        ".byte 0xF3, 0x0F, 0x6F, 0x07",

        // ── XOR with round key 0 ─────────────────────────────────────
        // movdqu xmm1, [rsi]
        ".byte 0xF3, 0x0F, 0x6F, 0x0E",
        // pxor xmm0, xmm1
        ".byte 0x66, 0x0F, 0xEF, 0xC1",

        // ── Setup: rax points to round key 1 ─────────────────────────
        "lea rax, [rsi + 16]",

        // ── 13 AESENC rounds (rounds 1 to 13) ────────────────────────
        // Round 1
        ".byte 0xF3, 0x0F, 0x6F, 0x08",        // movdqu xmm1, [rax]
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",  // aesenc xmm0, xmm1
        "add rax, 16",
        // Round 2
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 3
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 4
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 5
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 6
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 7
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 8
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 9
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 10
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 11
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 12
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",
        // Round 13
        ".byte 0xF3, 0x0F, 0x6F, 0x08",
        ".byte 0x66, 0x0F, 0x38, 0xDC, 0xC1",
        "add rax, 16",

        // ── AESENCLAST (round 14) ──────────────────────────────────────
        ".byte 0xF3, 0x0F, 0x6F, 0x08",        // movdqu xmm1, [rax]
        ".byte 0x66, 0x0F, 0x38, 0xDD, 0xC1",  // aesenclast xmm0, xmm1

        // ── Store result ─────────────────────────────────────────────
        // movdqu [rdi], xmm0
        ".byte 0xF3, 0x0F, 0x7F, 0x07",

        // ── Restore XMM0 and XMM1 ────────────────────────────────────
        // movdqu xmm0, [rsp]
        ".byte 0xF3, 0x0F, 0x6F, 0x04, 0x24",
        // movdqu xmm1, [rsp+16]
        ".byte 0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x10",

        // ── Restore stack ────────────────────────────────────────────
        "add rsp, 32",

        in("rdi") block,
        in("rsi") round_keys,
        out("rax") _,
        // AUDIT: "add rax, 16" modifie les flags EFLAGS — pas de preserves_flags
    );
}

#[cfg(not(target_arch = "x86_64"))]
// SAFETY: validated low-level operation — see block documentation.
unsafe fn aes256_encrypt_block_ni(_block: *mut u8, _round_keys: *const u8) {
    // No AES-NI on non-x86_64 architecture
    unreachable!()
}

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 Encryption — Software / AES-NI dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256 context for GCM encryption.
///
/// Encapsulates both pure-Rust software path and AES-NI
/// path (precomputed round keys + inline assembly).
struct Aes256GcmCipher {
    /// Round keys used by software path and AES-NI path.
    round_keys: Aes256RoundKeys,
    /// AES-NI available on this CPU.
    has_aesni: bool,
}

impl Aes256GcmCipher {
    fn new(key: &[u8; 32]) -> Self {
        let round_keys = aes256_expand_key(key);
        let has_aesni = cpu_features_or_none().map_or(false, |features| features.has_aes());
        Self {
            round_keys,
            has_aesni,
        }
    }

    /// Encrypts a 16-byte block (software / AES-NI dispatch).
    fn encrypt_block(&self, block: &mut [u8; 16]) {
        if self.has_aesni {
            // SAFETY: validated low-level operation — see block documentation.
            unsafe {
                aes256_encrypt_block_ni(block.as_mut_ptr(), self.round_keys.keys[0].as_ptr());
            }
            return;
        }
        aes256_encrypt_block_sw(&self.round_keys, block);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GF(2^128) Multiplication — GHASH core
// ─────────────────────────────────────────────────────────────────────────────

/// Multiplication in GF(2^128) according to GCM convention.
///
/// Representation: element = [u64; 2] where [0] = 8 most significant bytes
/// (MSB = x^0 in GCM convention) and [1] = 8 least significant bytes.
///
/// Reduction polynomial: x^128 + x^7 + x^2 + x + 1
/// Reduction constant: 0xE100_0000_0000_0000 (in word [0]).
///
/// Algorithm: shift-and-XOR (schoolbook), 128 iterations.
///
/// SECURITY — **constant-time**: no branch depends on data. During
/// tag computation, `b` contains the hash subkey `H = AES_K(0)`; branching
/// on its bits (`if bit_set`) would leak `H` via timing channel, allowing
/// GCM tag forgery. We therefore replace data-dependent branches
/// with **masks** (`0 - bit` = all-ones if bit=1, all-zeros otherwise). The only
/// `if i < 64` depends on loop index (public) → identical on every call.
fn gf128_mul(a: [u64; 2], b: [u64; 2]) -> [u64; 2] {
    let mut z = [0u64, 0u64];
    let mut v = [a[0], a[1]];

    for i in 0..128 {
        // Bit i of b (MSB first, GCM convention). Shift by `i` (public).
        let bit = if i < 64 {
            (b[0] >> (63 - i)) & 1
        } else {
            (b[1] >> (127 - i)) & 1
        };
        // Constant-time mask: 0xFFFF…FFFF if bit==1, 0 otherwise (no branch).
        let bit_mask = 0u64.wrapping_sub(bit);
        z[0] ^= v[0] & bit_mask;
        z[1] ^= v[1] & bit_mask;

        // Multiply v by x: shift right by 1 bit.
        let lsb = v[1] & 1;
        let carry = v[0] & 1;
        v[0] >>= 1;
        v[1] = (v[1] >> 1) | (carry << 63);

        // Conditional reduction (constant-time) if bit x^127 was set.
        let lsb_mask = 0u64.wrapping_sub(lsb);
        v[0] ^= 0xE100_0000_0000_0000 & lsb_mask;
    }

    z
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversion bytes ↔ GF(2^128) block
// ─────────────────────────────────────────────────────────────────────────────

/// Converts 16 bytes (big-endian) into GF(2^128) element.
#[inline]
fn bytes_to_gf(b: &[u8; 16]) -> [u64; 2] {
    [
        u64::from_be_bytes(b[0..8].try_into().unwrap()),
        u64::from_be_bytes(b[8..16].try_into().unwrap()),
    ]
}

/// Converts a GF(2^128) element into 16 bytes (big-endian).
#[inline]
fn gf_to_bytes(gf: [u64; 2]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&gf[0].to_be_bytes());
    out[8..16].copy_from_slice(&gf[1].to_be_bytes());
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// GHASH
// ─────────────────────────────────────────────────────────────────────────────

fn ghash_update(y: &mut [u64; 2], h: [u64; 2], data: &[u8]) {
    let mut pos = 0;
    while pos < data.len() {
        let mut block = [0u8; BLOCK_SIZE];
        let remaining = data.len() - pos;
        let chunk_len = remaining.min(BLOCK_SIZE);
        block[..chunk_len].copy_from_slice(&data[pos..pos + chunk_len]);

        let x = bytes_to_gf(&block);
        y[0] ^= x[0];
        y[1] ^= x[1];
        *y = gf128_mul(*y, h);

        pos += chunk_len;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM Counter Increment
// ─────────────────────────────────────────────────────────────────────────────

/// Increments GCM counter (32 least significant bits in big-endian).
///
/// Counter is a 16-byte block where the last 4 bytes form
/// a 32-bit big-endian counter (NIST SP 800-38D §5.2.1.1).
fn gcm_inc32(counter: &mut [u8; 16]) {
    let c = u32::from_be_bytes(counter[12..16].try_into().unwrap());
    let c = c.wrapping_add(1);
    counter[12..16].copy_from_slice(&c.to_be_bytes());
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM CTR Encryption
// ─────────────────────────────────────────────────────────────────────────────

/// CTR (Counter mode) encryption for GCM.
///
/// Generates a keystream from initial counter `icb` and XORs it
/// with `data`. The counter is incremented after each block.
fn gcm_ctr_encrypt(cipher: &Aes256GcmCipher, mut icb: [u8; 16], data: &mut [u8]) {
    let mut offset = 0;
    while offset < data.len() {
        // Encrypt counter to obtain keystream
        let mut keystream = icb;
        cipher.encrypt_block(&mut keystream);

        // XOR keystream with data
        let remaining = data.len() - offset;
        let chunk_len = remaining.min(BLOCK_SIZE);
        for i in 0..chunk_len {
            data[offset + i] ^= keystream[i];
        }

        // Increment counter
        gcm_inc32(&mut icb);
        offset += chunk_len;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM Seal / Open
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256-GCM seal (encrypt + authenticate).
///
/// Encrypts `plaintext` in place, computes GCM tag over (aad, ciphertext)
/// and writes tag into `tag_out`.
///
/// # Arguments
/// * `key`       — AES-256 key (32 bytes)
/// * `iv`        — Initialization vector (12 bytes, 96 bits)
/// * `aad`       — Additional authenticated data (unencrypted)
/// * `plaintext` — Data to encrypt (encrypted in place)
/// * `tag_out`   — Output authentication tag (16 bytes)
pub fn aes_gcm_seal(
    key: &[u8; AES_KEY_LEN],
    iv: &[u8; AES_GCM_NONCE_LEN],
    aad: &[u8],
    plaintext: &mut [u8],
    tag_out: &mut [u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    let cipher = Aes256GcmCipher::new(key);

    // 1. Compute H = AES_K(0^128) — hash subkey
    let mut h_block = [0u8; 16];
    cipher.encrypt_block(&mut h_block);
    let h = bytes_to_gf(&h_block);

    // 2. Build J0 = IV || 0^31 || 1
    let mut j0 = [0u8; 16];
    j0[0..12].copy_from_slice(iv);
    j0[15] = 1;

    // 3. Encrypt in CTR mode starting from inc32(J0)
    let mut counter = j0;
    gcm_inc32(&mut counter);
    gcm_ctr_encrypt(&cipher, counter, plaintext);

    // 4. Compute GHASH tag
    let tag = compute_gcm_tag(&cipher, h, j0, aad, plaintext);
    tag_out.copy_from_slice(&tag);

    Ok(())
}

/// AES-256-GCM open (verify + decrypt).
///
/// Verifies GCM tag, and if valid, decrypts `ciphertext` in place.
/// If tag is invalid, data is NOT decrypted and
/// `Err(AesGcmError::AuthenticationFailed)` is returned.
///
/// # Arguments
/// * `key`        — AES-256 key (32 bytes)
/// * `iv`         — Initialization vector (12 bytes)
/// * `aad`        — Additional authenticated data
/// * `ciphertext` — Encrypted data (decrypted in place if tag OK)
/// * `tag`        — Authentication tag (16 bytes)
pub fn aes_gcm_open(
    key: &[u8; AES_KEY_LEN],
    iv: &[u8; AES_GCM_NONCE_LEN],
    aad: &[u8],
    ciphertext: &mut [u8],
    tag: &[u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    let cipher = Aes256GcmCipher::new(key);

    // 1. Compute H = AES_K(0^128)
    let mut h_block = [0u8; 16];
    cipher.encrypt_block(&mut h_block);
    let h = bytes_to_gf(&h_block);

    // 2. Build J0 = IV || 0^31 || 1
    let mut j0 = [0u8; 16];
    j0[0..12].copy_from_slice(iv);
    j0[15] = 1;

    // 3. Verify tag BEFORE decrypting
    let expected_tag = compute_gcm_tag(&cipher, h, j0, aad, ciphertext);
    if !bool::from(expected_tag.ct_eq(tag)) {
        for byte in &mut h_block {
            // SAFETY: validated low-level operation — see block documentation.
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        return Err(AesGcmError::AuthenticationFailed);
    }

    // 4. Decrypt in CTR mode starting from inc32(J0)
    let mut counter = j0;
    gcm_inc32(&mut counter);
    gcm_ctr_encrypt(&cipher, counter, ciphertext);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM Tag Computation
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le tag GCM selon NIST SP 800-38D §7.1.
///
/// Tag = GHASH(H, A || C || len(A) || len(C)) ⊕ AES_K(J0)
fn compute_gcm_tag(
    cipher: &Aes256GcmCipher,
    h: [u64; 2],
    j0: [u8; 16],
    aad: &[u8],
    ciphertext: &[u8],
) -> [u8; 16] {
    let mut y = [0u64, 0u64];
    ghash_update(&mut y, h, aad);
    ghash_update(&mut y, h, ciphertext);

    // Traiter le bloc de longueurs : len(A) en bits (64-bit BE) || len(C) en bits (64-bit BE)
    let mut len_block = [0u8; BLOCK_SIZE];
    let aad_bits = (aad.len() as u64) * 8;
    let ct_bits = (ciphertext.len() as u64) * 8;
    len_block[0..8].copy_from_slice(&aad_bits.to_be_bytes());
    len_block[8..16].copy_from_slice(&ct_bits.to_be_bytes());

    let x = bytes_to_gf(&len_block);
    y[0] ^= x[0];
    y[1] ^= x[1];
    y = gf128_mul(y, h);

    // Convertir Y en bytes
    let ghash_result = gf_to_bytes(y);

    // Tag = GHASH_result ⊕ AES_K(J0)
    let mut tag = j0;
    cipher.encrypt_block(&mut tag);
    for i in 0..BLOCK_SIZE {
        tag[i] ^= ghash_result[i];
    }

    tag
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn init_crypto_test_env() {
        crate::arch::x86_64::cpu::features::init_cpu_features();
    }

    /// Test: GF(2^128) multiplication by 1 is identity.
    #[test]
    fn gf128_mul_identity() {
        init_crypto_test_env();
        let one: [u64; 2] = [0x8000_0000_0000_0000, 0]; // x^0 = 1 in GCM convention
        let a: [u64; 2] = [0x1234_5678_9ABC_DEF0, 0xFEDC_BA98_7654_3210];
        let result = gf128_mul(a, one);
        assert_eq!(result, a, "a * 1 should equal a");
    }

    /// Test: GF(2^128) multiplication by 0 is 0.
    #[test]
    fn gf128_mul_zero() {
        init_crypto_test_env();
        let zero: [u64; 2] = [0, 0];
        let a: [u64; 2] = [0x1234_5678_9ABC_DEF0, 0xFEDC_BA98_7654_3210];
        let result = gf128_mul(a, zero);
        assert_eq!(result, zero, "a * 0 should be 0");
    }

    /// Test: roundtrip seal/open with empty AAD.
    #[test]
    fn roundtrip_empty_aad() {
        init_crypto_test_env();
        let key = [0x42u8; AES_KEY_LEN];
        let iv = [0x24u8; AES_GCM_NONCE_LEN];
        let mut data = *b"AES-256-GCM test!";
        let original = data;
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"", &mut data, &mut tag).unwrap();
        assert_ne!(data, original, "Ciphertext must differ from plaintext");

        aes_gcm_open(&key, &iv, b"", &mut data, &tag).unwrap();
        assert_eq!(data, original, "Decrypted data must match original");
    }

    /// Test: corrupted tag is rejected.
    #[test]
    fn tampered_tag_rejected() {
        init_crypto_test_env();
        let key = [0x11u8; AES_KEY_LEN];
        let iv = [0x22u8; AES_GCM_NONCE_LEN];
        let mut data = *b"tamper-test-data!";
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"aad", &mut data, &mut tag).unwrap();
        tag[0] ^= 0x80;

        assert_eq!(
            aes_gcm_open(&key, &iv, b"aad", &mut data, &tag),
            Err(AesGcmError::AuthenticationFailed),
        );
    }

    /// Test: corrupted ciphertext is rejected.
    #[test]
    fn tampered_ciphertext_rejected() {
        init_crypto_test_env();
        let key = [0x33u8; AES_KEY_LEN];
        let iv = [0x44u8; AES_GCM_NONCE_LEN];
        let mut data = *b"corrupt-me-please";
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"", &mut data, &mut tag).unwrap();
        data[0] ^= 0x01;

        assert_eq!(
            aes_gcm_open(&key, &iv, b"", &mut data, &tag),
            Err(AesGcmError::AuthenticationFailed),
        );
    }

    /// Test: different AAD is rejected.
    #[test]
    fn wrong_aad_rejected() {
        init_crypto_test_env();
        let key = [0x55u8; AES_KEY_LEN];
        let iv = [0x66u8; AES_GCM_NONCE_LEN];
        let mut data = *b"aad-mismatch-test";
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"correct-aad", &mut data, &mut tag).unwrap();

        assert_eq!(
            aes_gcm_open(&key, &iv, b"wrong-aad", &mut data, &tag),
            Err(AesGcmError::AuthenticationFailed),
        );
    }

    /// Test: roundtrip with non-empty AAD.
    #[test]
    fn roundtrip_with_aad() {
        init_crypto_test_env();
        let key = [0x77u8; AES_KEY_LEN];
        let iv = [0x88u8; AES_GCM_NONCE_LEN];
        let mut data = *b"data-with-aad-ok!";
        let original = data;
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"authenticated-data", &mut data, &mut tag).unwrap();
        aes_gcm_open(&key, &iv, b"authenticated-data", &mut data, &tag).unwrap();
        assert_eq!(data, original);
    }

    /// Test: data not aligned to 16 bytes.
    #[test]
    fn roundtrip_unaligned() {
        init_crypto_test_env();
        let key = [0x99u8; AES_KEY_LEN];
        let iv = [0xAAu8; AES_GCM_NONCE_LEN];
        let mut data = *b"short"; // 5 bytes
        let original = data;
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"", &mut data, &mut tag).unwrap();
        aes_gcm_open(&key, &iv, b"", &mut data, &tag).unwrap();
        assert_eq!(data, original);
    }

    /// Test: empty data.
    #[test]
    fn roundtrip_empty_plaintext() {
        init_crypto_test_env();
        let key = [0xBBu8; AES_KEY_LEN];
        let iv = [0xCCu8; AES_GCM_NONCE_LEN];
        let mut data = [0u8; 0];
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"some-aad", &mut data, &mut tag).unwrap();
        aes_gcm_open(&key, &iv, b"some-aad", &mut data, &tag).unwrap();
        // No data to verify, but tag must be valid
    }
}
