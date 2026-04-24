// kernel/src/security/crypto/aes_gcm.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// AES-256-GCM — Software Implementation + AES-NI Runtime Detection
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation complète AES-256-GCM conforme NIST SP 800-38D :
//   • Software AES-256 via rounds software internes (pas de SIMD/SSE2)
//   • GCM mode avec GHASH multiplication (arithmétique u64 pure, pas de SIMD)
//   • Détection AES-NI à l'exécution (CPUID via CPU_FEATURES)
//   • Si AES-NI disponible → accélération matérielle (inline asm .byte)
//   • Si AES-NI absent → chemin software (T-tables)
//   • JAMAIS NotAvailableOnThisTarget
//
// CONTRAINTE TARGET : la crate aes-gcm NE COMPILE PAS sur x86_64-unknown-none
// (polyval/ghash génère des opérations 128-bit → LLVM ERROR: split 128-bit).
// Solution : utiliser un AES-256 software interne (SubBytes/ShiftRows/MixColumns)
// + GCM implémenté manuellement avec GHASH en arithmétique u64 pure.
//
// RÈGLE CRYPTO-CRATES : le chemin kernel bare-metal doit rester pure-Rust soft.
// Le GCM/GHASH est implémenté manuellement car la crate aes-gcm est incompatible
// avec le target. Le GHASH utilise uniquement des opérations u64 (pas de SIMD).
//
// RÉFÉRENCES :
//   NIST SP 800-38D (GCM Specification)
//   NIST SP 800-38A (AES Specification)
//   Intel Carry-Less Multiplication Instruction (White Paper)
// ═══════════════════════════════════════════════════════════════════════════════

use subtle::ConstantTimeEq;

use crate::arch::x86_64::cpu::features::cpu_features_or_none;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes publiques
// ─────────────────────────────────────────────────────────────────────────────

/// Longueur de la clé AES-256 (32 octets).
pub const AES_KEY_LEN: usize = 32;
/// Longueur du nonce AES-GCM (12 octets = 96 bits, recommandé NIST).
pub const AES_GCM_NONCE_LEN: usize = 12;
/// Longueur du tag GCM (16 octets = 128 bits).
pub const AES_GCM_TAG_LEN: usize = 16;

/// Taille de bloc AES (16 octets = 128 bits).
const BLOCK_SIZE: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// Erreur AES-GCM
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur AES-GCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AesGcmError {
    /// Authentification échouée (tag invalide ou données corrompues).
    AuthenticationFailed,
    /// Paramètre invalide (longueur incorrecte).
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
// AES S-Box (pour le key expansion du chemin AES-NI)
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

/// Round constants pour AES-256 key expansion.
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
// AES-256 Key Expansion (pour le chemin AES-NI)
// ─────────────────────────────────────────────────────────────────────────────

/// Round keys pour AES-256 (15 × 16 octets = 240 octets).
struct Aes256RoundKeys {
    keys: [[u8; 16]; 15],
}

/// SubWord : applique la S-Box à chaque octet d'un mot 32 bits.
fn sub_word(w: u32) -> u32 {
    let b0 = AES_SBOX[((w >> 24) & 0xFF) as usize] as u32;
    let b1 = AES_SBOX[((w >> 16) & 0xFF) as usize] as u32;
    let b2 = AES_SBOX[((w >> 8) & 0xFF) as usize] as u32;
    let b3 = AES_SBOX[((w) & 0xFF) as usize] as u32;
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

/// RotWord : rotation d'un mot 32 bits vers la gauche de 8 bits.
#[inline]
fn rot_word(w: u32) -> u32 {
    w.rotate_left(8)
}

/// Expansion de clé AES-256 standard (FIPS 197 §5.2).
///
/// Produit 15 round keys (60 mots) à partir de la clé 256-bit.
fn aes256_expand_key(key: &[u8; 32]) -> Aes256RoundKeys {
    let mut w = [0u32; 60];

    // Initialiser les 8 premiers mots depuis la clé
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

/// Chiffre un bloc 16 octets via AES-NI (accélération matérielle).
///
/// Utilise l'inline assembly avec encodage .byte pour les instructions
/// SSE/AES-NI, afin de contourner la restriction LLVM sur le target
/// x86_64-unknown-none (pas de SSE2 dans les target features).
///
/// Les registres XMM sont sauvegardés et restaurés manuellement pour
/// éviter la corruption de l'état FPU du thread courant.
///
/// # Safety
/// - Doit être appelé uniquement quand AES-NI est disponible
/// - `block` doit pointer vers 16 octets accessibles en lecture/écriture
/// - `round_keys` doit pointer vers 240 octets (15 × 16) lisibles
#[cfg(target_arch = "x86_64")]
unsafe fn aes256_encrypt_block_ni(block: *mut u8, round_keys: *const u8) {
    // Encode les instructions AES-NI en .byte pour éviter LLVM.
    //
    // Registres utilisés :
    //   rdi = pointeur vers le bloc (in/out)
    //   rsi = pointeur vers les round keys
    //   rax = pointeur courant vers les round keys
    //   xmm0 = état AES
    //   xmm1 = round key temporaire
    //
    // Instructions encodées :
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
        // ── Sauvegarder XMM0 et XMM1 sur la pile ──────────────────────
        "sub rsp, 32",
        // movdqu [rsp], xmm0
        ".byte 0xF3, 0x0F, 0x7F, 0x04, 0x24",
        // movdqu [rsp+16], xmm1
        ".byte 0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x10",

        // ── Charger le plaintext dans xmm0 ────────────────────────────
        // movdqu xmm0, [rdi]
        ".byte 0xF3, 0x0F, 0x6F, 0x07",

        // ── XOR avec la round key 0 ───────────────────────────────────
        // movdqu xmm1, [rsi]
        ".byte 0xF3, 0x0F, 0x6F, 0x0E",
        // pxor xmm0, xmm1
        ".byte 0x66, 0x0F, 0xEF, 0xC1",

        // ── Setup : rax pointe vers la round key 1 ────────────────────
        "lea rax, [rsi + 16]",

        // ── 13 AESENC rounds (rounds 1 à 13) ─────────────────────────
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

        // ── Stocker le résultat ────────────────────────────────────────
        // movdqu [rdi], xmm0
        ".byte 0xF3, 0x0F, 0x7F, 0x07",

        // ── Restaurer XMM0 et XMM1 ────────────────────────────────────
        // movdqu xmm0, [rsp]
        ".byte 0xF3, 0x0F, 0x6F, 0x04, 0x24",
        // movdqu xmm1, [rsp+16]
        ".byte 0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x10",

        // ── Restaurer la pile ──────────────────────────────────────────
        "add rsp, 32",

        in("rdi") block,
        in("rsi") round_keys,
        out("rax") _,
        options(preserves_flags),
    );
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn aes256_encrypt_block_ni(_block: *mut u8, _round_keys: *const u8) {
    // Pas d'AES-NI sur une architecture non-x86_64
    unreachable!()
}

// ─────────────────────────────────────────────────────────────────────────────
// AES-256 Encryption — Dispatch software / AES-NI
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte AES-256 pour le chiffrement GCM.
///
/// Encapsule à la fois le chemin software pure-Rust et le chemin
/// AES-NI (round keys pré-calculées + inline assembly).
struct Aes256GcmCipher {
    /// Round keys utilisées par le chemin software et le chemin AES-NI.
    round_keys: Aes256RoundKeys,
    /// AES-NI disponible sur ce CPU.
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

    /// Chiffre un bloc 16 octets (dispatch software / AES-NI).
    fn encrypt_block(&self, block: &mut [u8; 16]) {
        if self.has_aesni {
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

/// Multiplication dans GF(2^128) selon la convention GCM.
///
/// Représentation : élément = [u64; 2] où [0] = 8 octets de poids fort
/// (MSB = x^0 dans la convention GCM) et [1] = 8 octets de poids faible.
///
/// Polynôme de réduction : x^128 + x^7 + x^2 + x + 1
/// Constante de réduction : 0xE100_0000_0000_0000 (dans le mot [0]).
///
/// Algorithme : shift-and-XOR (schoolbook), 128 itérations.
fn gf128_mul(a: [u64; 2], b: [u64; 2]) -> [u64; 2] {
    let mut z = [0u64, 0u64];
    let mut v = [a[0], a[1]];

    for i in 0..128 {
        // Vérifier le bit i de b (MSB en premier, convention GCM)
        let bit_set = if i < 64 {
            (b[0] >> (63 - i)) & 1
        } else {
            (b[1] >> (127 - i)) & 1
        };

        if bit_set != 0 {
            z[0] ^= v[0];
            z[1] ^= v[1];
        }

        // Multiplier v par x : décaler à droite de 1 bit
        let lsb = v[1] & 1;
        let carry = v[0] & 1;
        v[0] >>= 1;
        v[1] = (v[1] >> 1) | (carry << 63);

        // Réduction si le bit x^127 était positionné
        if lsb != 0 {
            v[0] ^= 0xE100_0000_0000_0000;
        }
    }

    z
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversion bytes ↔ GF(2^128) block
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit 16 octets (big-endian) en élément GF(2^128).
#[inline]
fn bytes_to_gf(b: &[u8; 16]) -> [u64; 2] {
    [
        u64::from_be_bytes(b[0..8].try_into().unwrap()),
        u64::from_be_bytes(b[8..16].try_into().unwrap()),
    ]
}

/// Convertit un élément GF(2^128) en 16 octets (big-endian).
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

/// Incrémente le compteur GCM (32 bits de poids faible en big-endian).
///
/// Le compteur est un bloc de 16 octets où les 4 derniers octets forment
/// un compteur 32 bits en big-endian (NIST SP 800-38D §5.2.1.1).
fn gcm_inc32(counter: &mut [u8; 16]) {
    let c = u32::from_be_bytes(counter[12..16].try_into().unwrap());
    let c = c.wrapping_add(1);
    counter[12..16].copy_from_slice(&c.to_be_bytes());
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM CTR Encryption
// ─────────────────────────────────────────────────────────────────────────────

/// Chiffrement CTR (Counter mode) pour GCM.
///
/// Génère un keystream à partir du compteur initial `icb` et le XOR
/// avec `data`. Le compteur est incrémenté après chaque bloc.
fn gcm_ctr_encrypt(cipher: &Aes256GcmCipher, mut icb: [u8; 16], data: &mut [u8]) {
    let mut offset = 0;
    while offset < data.len() {
        // Chiffrer le compteur pour obtenir le keystream
        let mut keystream = icb;
        cipher.encrypt_block(&mut keystream);

        // XOR le keystream avec les données
        let remaining = data.len() - offset;
        let chunk_len = remaining.min(BLOCK_SIZE);
        for i in 0..chunk_len {
            data[offset + i] ^= keystream[i];
        }

        // Incrémenter le compteur
        gcm_inc32(&mut icb);
        offset += chunk_len;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GCM Seal / Open
// ─────────────────────────────────────────────────────────────────────────────

/// AES-256-GCM seal (encrypt + authenticate).
///
/// Chiffre `plaintext` en place, calcule le tag GCM sur (aad, ciphertext)
/// et écrit le tag dans `tag_out`.
///
/// # Arguments
/// * `key`       — Clé AES-256 (32 octets)
/// * `iv`        — Vecteur d'initialisation (12 octets, 96 bits)
/// * `aad`       — Données additionnelles authentifiées (non chiffrées)
/// * `plaintext` — Données à chiffrer (chiffrées en place)
/// * `tag_out`   — Tag d'authentification en sortie (16 octets)
pub fn aes_gcm_seal(
    key: &[u8; AES_KEY_LEN],
    iv: &[u8; AES_GCM_NONCE_LEN],
    aad: &[u8],
    plaintext: &mut [u8],
    tag_out: &mut [u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    let cipher = Aes256GcmCipher::new(key);

    // 1. Calculer H = AES_K(0^128) — hash subkey
    let mut h_block = [0u8; 16];
    cipher.encrypt_block(&mut h_block);
    let h = bytes_to_gf(&h_block);

    // 2. Construire J0 = IV || 0^31 || 1
    let mut j0 = [0u8; 16];
    j0[0..12].copy_from_slice(iv);
    j0[15] = 1;

    // 3. Chiffrer en mode CTR à partir de inc32(J0)
    let mut counter = j0;
    gcm_inc32(&mut counter);
    gcm_ctr_encrypt(&cipher, counter, plaintext);

    // 4. Calculer le tag GHASH
    let tag = compute_gcm_tag(&cipher, h, j0, aad, plaintext);
    tag_out.copy_from_slice(&tag);

    Ok(())
}

/// AES-256-GCM open (verify + decrypt).
///
/// Vérifie le tag GCM, et s'il est valide, déchiffre `ciphertext` en place.
/// Si le tag est invalide, les données ne sont PAS déchiffrées et
/// `Err(AesGcmError::AuthenticationFailed)` est retourné.
///
/// # Arguments
/// * `key`        — Clé AES-256 (32 octets)
/// * `iv`         — Vecteur d'initialisation (12 octets)
/// * `aad`        — Données additionnelles authentifiées
/// * `ciphertext` — Données chiffrées (déchiffrées en place si tag OK)
/// * `tag`        — Tag d'authentification (16 octets)
pub fn aes_gcm_open(
    key: &[u8; AES_KEY_LEN],
    iv: &[u8; AES_GCM_NONCE_LEN],
    aad: &[u8],
    ciphertext: &mut [u8],
    tag: &[u8; AES_GCM_TAG_LEN],
) -> Result<(), AesGcmError> {
    let cipher = Aes256GcmCipher::new(key);

    // 1. Calculer H = AES_K(0^128)
    let mut h_block = [0u8; 16];
    cipher.encrypt_block(&mut h_block);
    let h = bytes_to_gf(&h_block);

    // 2. Construire J0 = IV || 0^31 || 1
    let mut j0 = [0u8; 16];
    j0[0..12].copy_from_slice(iv);
    j0[15] = 1;

    // 3. Vérifier le tag AVANT de déchiffrer
    let expected_tag = compute_gcm_tag(&cipher, h, j0, aad, ciphertext);
    if !bool::from(expected_tag.ct_eq(tag)) {
        for byte in &mut h_block {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        return Err(AesGcmError::AuthenticationFailed);
    }

    // 4. Déchiffrer en mode CTR à partir de inc32(J0)
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

    /// Test : GF(2^128) multiplication par 1 est identité.
    #[test]
    fn gf128_mul_identity() {
        init_crypto_test_env();
        let one: [u64; 2] = [0x8000_0000_0000_0000, 0]; // x^0 = 1 dans la convention GCM
        let a: [u64; 2] = [0x1234_5678_9ABC_DEF0, 0xFEDC_BA98_7654_3210];
        let result = gf128_mul(a, one);
        assert_eq!(result, a, "a * 1 should equal a");
    }

    /// Test : GF(2^128) multiplication par 0 est 0.
    #[test]
    fn gf128_mul_zero() {
        init_crypto_test_env();
        let zero: [u64; 2] = [0, 0];
        let a: [u64; 2] = [0x1234_5678_9ABC_DEF0, 0xFEDC_BA98_7654_3210];
        let result = gf128_mul(a, zero);
        assert_eq!(result, zero, "a * 0 should be 0");
    }

    /// Test : roundtrip seal/open avec AAD vide.
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

    /// Test : tag corrompu est rejeté.
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

    /// Test : ciphertext corrompu est rejeté.
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

    /// Test : AAD différent est rejeté.
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

    /// Test : roundtrip avec AAD non vide.
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

    /// Test : données non alignées sur 16 octets.
    #[test]
    fn roundtrip_unaligned() {
        init_crypto_test_env();
        let key = [0x99u8; AES_KEY_LEN];
        let iv = [0xAAu8; AES_GCM_NONCE_LEN];
        let mut data = *b"short"; // 5 octets
        let original = data;
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"", &mut data, &mut tag).unwrap();
        aes_gcm_open(&key, &iv, b"", &mut data, &tag).unwrap();
        assert_eq!(data, original);
    }

    /// Test : données vides.
    #[test]
    fn roundtrip_empty_plaintext() {
        init_crypto_test_env();
        let key = [0xBBu8; AES_KEY_LEN];
        let iv = [0xCCu8; AES_GCM_NONCE_LEN];
        let mut data = [0u8; 0];
        let mut tag = [0u8; AES_GCM_TAG_LEN];

        aes_gcm_seal(&key, &iv, b"some-aad", &mut data, &mut tag).unwrap();
        aes_gcm_open(&key, &iv, b"some-aad", &mut data, &tag).unwrap();
        // Pas de données à vérifier, mais le tag doit être valide
    }
}
