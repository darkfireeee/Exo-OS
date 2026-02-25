// kernel/src/security/crypto/blake3.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BLAKE3 — Fonction de hachage cryptographique (Exo-OS Security · Couche 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation complète de BLAKE3 v1.3.1 (RFC draft 2021).
// BLAKE3 est utilisé pour :
//   • Checksums d'intégrité kernel
//   • PRF dans le KDF (HKDF-BLAKE3)
//   • Vérification de signature de modules
//   • MAC des canaux kernel (combiné avec XChaCha20)
//
// PERFORMANCE : BLAKE3 est conçu pour être très rapide sur des messages larges.
// Cette implémentation séquentielle cible la correction ; une variante AVX-512
// pourrait être ajoutée dans arch/x86_64/crypto/ pour les chemins hot.
//
// RÉFÉRENCE : https://github.com/BLAKE3-team/BLAKE3-specs/blob/master/blake3.pdf
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

// ─────────────────────────────────────────────────────────────────────────────
// Constantes IV (vecteurs d'initialisation) — mêmes que SHA-256
// ─────────────────────────────────────────────────────────────────────────────

const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
    0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

// ─────────────────────────────────────────────────────────────────────────────
// Flags de domaine (domain separation)
// ─────────────────────────────────────────────────────────────────────────────

const CHUNK_START:  u32 = 1 << 0;
const CHUNK_END:    u32 = 1 << 1;
const PARENT:       u32 = 1 << 2;
const ROOT:         u32 = 1 << 3;
const KEYED_HASH:   u32 = 1 << 4;
const DERIVE_KEY_CONTEXT: u32 = 1 << 5;
const DERIVE_KEY_MATERIAL: u32 = 1 << 6;

const BLOCK_LEN: usize  = 64;
const CHUNK_LEN: usize  = 1024;
const OUT_LEN:   usize  = 32;
const KEY_LEN:   usize  = 32;

// ─────────────────────────────────────────────────────────────────────────────
// Permutations de message
// ─────────────────────────────────────────────────────────────────────────────

const MSG_SCHEDULE: [[usize; 16]; 7] = [
    [0,  1,  2,  3,  4,  5,  6,  7,  8,  9,  10, 11, 12, 13, 14, 15],
    [2,  6,  3,  10, 7,  0,  4,  13, 1,  11, 12, 5,  9,  14, 15, 8 ],
    [3,  4,  10, 12, 13, 2,  7,  14, 6,  5,  9,  0,  11, 15, 8,  1 ],
    [10, 7,  12, 9,  14, 3,  13, 15, 4,  0,  11, 2,  5,  8,  1,  6 ],
    [12, 13, 9,  11, 15, 10, 14, 8,  7,  2,  5,  3,  0,  1,  6,  4 ],
    [9,  14, 11, 5,  8,  12, 15, 1,  13, 3,  0,  10, 2,  6,  4,  7 ],
    [11, 15, 5,  0,  1,  9,  8,  6,  14, 10, 2,  12, 3,  4,  7,  13],
];

// ─────────────────────────────────────────────────────────────────────────────
// Fonction G — quart de round BLAKE3
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn g(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(mx);
    state[d] = (state[d] ^ state[a]).rotate_right(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(12);
    state[a] = state[a].wrapping_add(state[b]).wrapping_add(my);
    state[d] = (state[d] ^ state[a]).rotate_right(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_right(7);
}

/// Compression BLAKE3 — 7 rounds.
#[inline]
fn compress(
    chaining_value: &[u32; 8],
    block_words:    &[u32; 16],
    counter:        u64,
    block_len:      u32,
    flags:          u32,
) -> [u32; 16] {
    let mut state: [u32; 16] = [
        chaining_value[0], chaining_value[1], chaining_value[2], chaining_value[3],
        chaining_value[4], chaining_value[5], chaining_value[6], chaining_value[7],
        IV[0], IV[1], IV[2], IV[3],
        counter as u32, (counter >> 32) as u32, block_len, flags,
    ];

    for r in 0..7 {
        let s = &MSG_SCHEDULE[r];
        g(&mut state, 0, 4,  8, 12, block_words[s[0]],  block_words[s[1]]);
        g(&mut state, 1, 5,  9, 13, block_words[s[2]],  block_words[s[3]]);
        g(&mut state, 2, 6, 10, 14, block_words[s[4]],  block_words[s[5]]);
        g(&mut state, 3, 7, 11, 15, block_words[s[6]],  block_words[s[7]]);
        g(&mut state, 0, 5, 10, 15, block_words[s[8]],  block_words[s[9]]);
        g(&mut state, 1, 6, 11, 12, block_words[s[10]], block_words[s[11]]);
        g(&mut state, 2, 7,  8, 13, block_words[s[12]], block_words[s[13]]);
        g(&mut state, 3, 4,  9, 14, block_words[s[14]], block_words[s[15]]);
    }

    // XOR les deux moitiés
    for i in 0..8 {
        state[i]   ^= state[i + 8];
        state[i+8] ^= chaining_value[i];
    }
    state
}

/// Convertit un bloc de bytes en 16 mots u32 LE.
#[inline(always)]
fn words_from_le_bytes_64(bytes: &[u8; 64]) -> [u32; 16] {
    let mut words = [0u32; 16];
    for (i, w) in words.iter_mut().enumerate() {
        *w = u32::from_le_bytes(bytes[i*4..i*4+4].try_into().unwrap());
    }
    words
}

/// Extrait les 8 premiers mots d'un output de compression comme chaîning value.
#[inline(always)]
fn first_8_words(output: [u32; 16]) -> [u32; 8] {
    [output[0], output[1], output[2], output[3],
     output[4], output[5], output[6], output[7]]
}

// ─────────────────────────────────────────────────────────────────────────────
// ChunkState — état d'un chunk de 1024 bytes
// ─────────────────────────────────────────────────────────────────────────────

struct ChunkState {
    chaining_value: [u32; 8],
    chunk_counter:  u64,
    block:          [u8; BLOCK_LEN],
    block_len:      u8,
    blocks_compressed: u8,
    flags:          u32,
}

impl ChunkState {
    fn new(key: &[u32; 8], chunk_counter: u64, flags: u32) -> Self {
        Self {
            chaining_value:    *key,
            chunk_counter,
            block:             [0u8; BLOCK_LEN],
            block_len:         0,
            blocks_compressed: 0,
            flags,
        }
    }

    fn len(&self) -> usize {
        BLOCK_LEN * self.blocks_compressed as usize + self.block_len as usize
    }

    fn start_flag(&self) -> u32 {
        if self.blocks_compressed == 0 { CHUNK_START } else { 0 }
    }

    fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            // Compresser le bloc courant si plein
            if self.block_len as usize == BLOCK_LEN {
                let words = words_from_le_bytes_64(&self.block);
                let out = compress(
                    &self.chaining_value,
                    &words,
                    self.chunk_counter,
                    BLOCK_LEN as u32,
                    self.flags | self.start_flag(),
                );
                self.chaining_value = first_8_words(out);
                self.blocks_compressed += 1;
                self.block = [0u8; BLOCK_LEN];
                self.block_len = 0;
            }
            let want = BLOCK_LEN - self.block_len as usize;
            let take = want.min(input.len());
            self.block[self.block_len as usize..self.block_len as usize + take]
                .copy_from_slice(&input[..take]);
            self.block_len += take as u8;
            input = &input[take..];
        }
    }

    fn output(&self) -> Output {
        let mut block_words = [0u32; 16];
        let mut block_padded = [0u8; BLOCK_LEN];
        block_padded[..self.block_len as usize].copy_from_slice(&self.block[..self.block_len as usize]);
        for (i, w) in block_words.iter_mut().enumerate() {
            *w = u32::from_le_bytes(block_padded[i*4..i*4+4].try_into().unwrap());
        }
        Output {
            input_chaining_value: self.chaining_value,
            block_words,
            counter:    self.chunk_counter,
            block_len:  self.block_len as u32,
            flags:      self.flags | self.start_flag() | CHUNK_END,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Output — nœud de sortie BLAKE3
// ─────────────────────────────────────────────────────────────────────────────

struct Output {
    input_chaining_value: [u32; 8],
    block_words:          [u32; 16],
    counter:              u64,
    block_len:            u32,
    flags:                u32,
}

impl Output {
    fn chaining_value(&self) -> [u32; 8] {
        first_8_words(compress(
            &self.input_chaining_value,
            &self.block_words,
            self.counter,
            self.block_len,
            self.flags,
        ))
    }

    fn root_output_bytes(&self, out: &mut [u8]) {
        let mut output_block_counter = 0u64;
        let mut pos = 0;
        while pos < out.len() {
            let words = compress(
                &self.input_chaining_value,
                &self.block_words,
                output_block_counter,
                self.block_len,
                self.flags | ROOT,
            );
            let bytes_to_copy = 64.min(out.len() - pos);
            for (i, word) in words.iter().enumerate() {
                let word_bytes = word.to_le_bytes();
                let word_start = i * 4;
                let word_end   = (word_start + 4).min(out.len() - pos + word_start);
                if word_start < bytes_to_copy {
                    let copy_len = (word_end - word_start).min(4);
                    out[pos + word_start..pos + word_start + copy_len]
                        .copy_from_slice(&word_bytes[..copy_len]);
                }
            }
            pos += bytes_to_copy;
            output_block_counter += 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hasher — interface principale BLAKE3
// ─────────────────────────────────────────────────────────────────────────────

/// Hasher BLAKE3 — API principale.
pub struct Blake3Hasher {
    key:              [u32; 8],
    chunk_state:      ChunkState,
    /// Stack de cv (chaining values) pour la construction de l'arbre Merkle.
    cv_stack:         [[u32; 8]; 54],
    cv_stack_len:     usize,
    flags:            u32,
}

impl Blake3Hasher {
    /// Crée un hasher pour le mode hachage standard.
    pub fn new() -> Self {
        Self::new_internal(IV, 0)
    }

    /// Crée un hasher avec clé (keyed hash, MAC).
    pub fn new_keyed(key: &[u8; KEY_LEN]) -> Self {
        let mut key_words = [0u32; 8];
        for (i, w) in key_words.iter_mut().enumerate() {
            *w = u32::from_le_bytes(key[i*4..i*4+4].try_into().unwrap());
        }
        Self::new_internal(key_words, KEYED_HASH)
    }

    /// Crée un hasher pour la dérivation de clé (KDF).
    pub fn new_derive_key(context: &[u8]) -> Self {
        let context_key = {
            let mut h = Self::new_internal(IV, DERIVE_KEY_CONTEXT);
            h.update(context);
            let mut out = [0u8; 32];
            h.finalize(&mut out);
            let mut words = [0u32; 8];
            for (i, w) in words.iter_mut().enumerate() {
                *w = u32::from_le_bytes(out[i*4..i*4+4].try_into().unwrap());
            }
            words
        };
        Self::new_internal(context_key, DERIVE_KEY_MATERIAL)
    }

    fn new_internal(key: [u32; 8], flags: u32) -> Self {
        Self {
            key,
            chunk_state: ChunkState::new(&key, 0, flags),
            cv_stack:    [[0u32; 8]; 54],
            cv_stack_len: 0,
            flags,
        }
    }

    /// Pousse un chaining value sur la pile.
    fn push_cv(&mut self, cv: [u32; 8]) {
        self.cv_stack[self.cv_stack_len] = cv;
        self.cv_stack_len += 1;
    }

    /// Merge de deux cv parents.
    fn parent_cv(&self, left: [u32; 8], right: [u32; 8]) -> [u32; 8] {
        let mut block_words = [0u32; 16];
        block_words[..8].copy_from_slice(&left);
        block_words[8..].copy_from_slice(&right);
        let out = compress(&self.key, &block_words, 0, BLOCK_LEN as u32, self.flags | PARENT);
        first_8_words(out)
    }

    /// Ajoute des données au hasher.
    pub fn update(&mut self, input: &[u8]) {
        let mut pos = 0;
        while pos < input.len() {
            // Si le chunk courant est complet, le finaliser et stocker son cv
            if self.chunk_state.len() == CHUNK_LEN {
                let chunk_cv = self.chunk_state.output().chaining_value();
                let total_chunks = self.chunk_state.chunk_counter + 1;
                self.push_cv(chunk_cv);
                // Fusionner avec les chunks de même hauteur (total_chunks est power of 2)
                let mut n = total_chunks;
                while n & 1 == 0 {
                    let right = self.cv_stack[self.cv_stack_len - 1];
                    let left  = self.cv_stack[self.cv_stack_len - 2];
                    self.cv_stack_len -= 2;
                    let parent = self.parent_cv(left, right);
                    self.push_cv(parent);
                    n >>= 1;
                }
                self.chunk_state = ChunkState::new(&self.key, total_chunks, self.flags);
            }
            let want = CHUNK_LEN - self.chunk_state.len();
            let take = want.min(input.len() - pos);
            self.chunk_state.update(&input[pos..pos + take]);
            pos += take;
        }
    }

    /// Finalise et écrit la sortie dans `out` (longueur arbitraire ≥ 1 byte).
    pub fn finalize(&self, out: &mut [u8]) {
        let mut current_output = self.chunk_state.output();
        let mut parent_nodes_remaining = self.cv_stack_len;

        while parent_nodes_remaining > 0 {
            parent_nodes_remaining -= 1;
            let left = self.cv_stack[parent_nodes_remaining];
            let right = current_output.chaining_value();
            let mut block_words = [0u32; 16];
            block_words[..8].copy_from_slice(&left);
            block_words[8..].copy_from_slice(&right);
            current_output = Output {
                input_chaining_value: self.key,
                block_words,
                counter: 0,
                block_len: BLOCK_LEN as u32,
                flags: self.flags | PARENT,
            };
        }
        current_output.root_output_bytes(out);
    }

    /// Raccourci : hash un seul message et retourne les 32 premiers bytes.
    pub fn hash(input: &[u8]) -> [u8; OUT_LEN] {
        let mut h = Self::new();
        h.update(input);
        let mut out = [0u8; OUT_LEN];
        h.finalize(&mut out);
        out
    }

    /// Raccourci : MAC d'un message avec une clé.
    pub fn mac(key: &[u8; KEY_LEN], input: &[u8]) -> [u8; OUT_LEN] {
        let mut h = Self::new_keyed(key);
        h.update(input);
        let mut out = [0u8; OUT_LEN];
        h.finalize(&mut out);
        out
    }
}

impl Default for Blake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API simplifiée exportée
// ─────────────────────────────────────────────────────────────────────────────

/// Hash BLAKE3 d'un message — retourne 32 bytes.
#[inline]
pub fn blake3_hash(input: &[u8]) -> [u8; 32] {
    Blake3Hasher::hash(input)
}

/// MAC BLAKE3 (keyed hash) d'un message avec une clé de 32 bytes.
#[inline]
pub fn blake3_mac(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    Blake3Hasher::mac(key, input)
}

/// Dérivation de clé BLAKE3 — contexte de domaine + matériel → clé dérivée.
#[inline]
pub fn blake3_derive_key(context: &[u8], material: &[u8], out: &mut [u8]) {
    let mut h = Blake3Hasher::new_derive_key(context);
    h.update(material);
    h.finalize(out);
}

/// Compare deux digests en temps constant (résistance aux timing attacks).
#[inline]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
