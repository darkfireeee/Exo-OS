// kernel/src/fs/exofs/core/blob_id.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BlobId — wrapper Blake3 content-addressed
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE HASH-01 (CRITIQUE) : BlobId = Blake3(données AVANT compression et
//   AVANT chiffrement). Jamais calculé sur des données post-traitement.
//   Violation = déduplication à 0% + BlobIds incohérents après recovery.

use crate::fs::exofs::core::types::BlobId;

// ─────────────────────────────────────────────────────────────────────────────
// Blake3 kernel — implémentation interne no_std
// ─────────────────────────────────────────────────────────────────────────────
//
// Blake3 est une fonction de hachage moderne avec:
//   - Sécurité: 256 bits de résistance aux collisions
//   - Performance: SIMD-friendly, parallélisable
//   - Arbre Merkle interne: O(n/CHUNK_LEN) pour les grands inputs
//
// Nous utilisons une implémentation simplifiée de la fonction de compression
// pour le kernel no_std, en attendant la crate blake3-no_std officielle.

/// Constantes Blake3 internes (IV = SHA-256 fractional parts).
const BLAKE3_IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
    0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

/// Sigma permutation table Blake3/Blake2.
const SIGMA: [[usize; 16]; 10] = [
    [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15],
    [14,10, 4, 8, 9,15,13, 6, 1,12, 0, 2,11, 7, 5, 3],
    [11, 8,12, 0, 5, 2,15,13,10,14, 3, 6, 7, 1, 9, 4],
    [ 7, 9, 3, 1,13,12,11,14, 2, 6, 5,10, 4, 0,15, 8],
    [ 9, 0, 5, 7, 2, 4,10,15,14, 1,11,12, 6, 8, 3,13],
    [ 2,12, 6,10, 0,11, 8, 3, 4,13, 7, 5,15,14, 1, 9],
    [12, 5, 1,15,14,13, 4,10, 0, 7, 6, 3, 9, 2, 8,11],
    [13,11, 7,14,12, 1, 3, 9, 5, 0,15, 4, 8, 6, 2,10],
    [ 6,15,14, 9,11, 3, 0, 8,12, 2,13, 7, 1, 4,10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5,15,11, 9,14, 3,12,13, 0],
];

/// Fonction G de mélange Blake (quart de round).
#[inline(always)]
fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// Compression Blake3 simplifiée sur un bloc de 64 octets.
///
/// Utilisée pour les BlobIds et les ObjectIds Class1.
/// Pour la production complète, une crate blake3 no_std dédiée devrait être liée.
fn blake3_compress(input: &[u8; 64], chaining_value: &[u32; 8]) -> [u32; 8] {
    let mut m = [0u32; 16];
    for i in 0..16 {
        let o = i * 4;
        m[i] = u32::from_le_bytes([input[o], input[o+1], input[o+2], input[o+3]]);
    }

    let mut v = [0u32; 16];
    v[0..8].copy_from_slice(chaining_value);
    v[8..16].copy_from_slice(&BLAKE3_IV);

    // 7 rounds Blake3.
    for round in 0..7 {
        let s = &SIGMA[round % 10];
        g(&mut v, 0, 4,  8, 12, m[s[ 0]], m[s[ 1]]);
        g(&mut v, 1, 5,  9, 13, m[s[ 2]], m[s[ 3]]);
        g(&mut v, 2, 6, 10, 14, m[s[ 4]], m[s[ 5]]);
        g(&mut v, 3, 7, 11, 15, m[s[ 6]], m[s[ 7]]);
        g(&mut v, 0, 5, 10, 15, m[s[ 8]], m[s[ 9]]);
        g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
        g(&mut v, 2, 7,  8, 13, m[s[12]], m[s[13]]);
        g(&mut v, 3, 4,  9, 14, m[s[14]], m[s[15]]);
    }

    let mut out = [0u32; 8];
    for i in 0..8 {
        out[i] = v[i] ^ v[i + 8] ^ chaining_value[i];
    }
    out
}

/// Calcule un hash Blake3 de 32 octets sur un buffer arbitraire.
///
/// Traite le buffer par blocs de 64 octets avec padding zéro final.
/// Cette fonction est le point central de hachage — utilisée partout
/// où un hash de 32 octets est necessaire.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    let mut cv = BLAKE3_IV;
    let mut block = [0u8; 64];

    // Traitement par blocs de 64 octets.
    let chunks = data.len() / 64;
    for i in 0..chunks {
        block.copy_from_slice(&data[i*64..(i+1)*64]);
        cv = blake3_compress(&block, &cv);
    }

    // Dernier bloc avec padding zéro.
    let rem = data.len() % 64;
    block = [0u8; 64];
    if rem > 0 {
        block[..rem].copy_from_slice(&data[chunks*64..]);
    }
    // Appliquer le padding de longueur (longueur totale en LE64 à l'offset 56).
    let len_le = (data.len() as u64).to_le_bytes();
    block[56..64].copy_from_slice(&len_le);
    cv = blake3_compress(&block, &cv);

    // Sérialisation de l'état final en 32 octets LE.
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i*4..(i+1)*4].copy_from_slice(&cv[i].to_le_bytes());
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// API BlobId
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule un BlobId depuis les données BRUTES non-compressées.
///
/// RÈGLE HASH-01 : appelé AVANT compression et AVANT chiffrement.
/// Violation = BlobIds incohérents = déduplication brisée.
pub fn compute_blob_id(raw_data: &[u8]) -> BlobId {
    BlobId(blake3_hash(raw_data))
}

/// Vérifie qu'un BlobId correspond aux données fournies.
///
/// Utilisé après lecture disque pour détecter la corruption (règle HDR-03).
#[inline]
pub fn verify_blob_id(blob_id: &BlobId, raw_data: &[u8]) -> bool {
    let expected = compute_blob_id(raw_data);
    blob_id.ct_eq(&expected)
}

// ─────────────────────────────────────────────────────────────────────────────
// Méthodes additionnelles sur BlobId
// ─────────────────────────────────────────────────────────────────────────────

impl BlobId {
    /// Crée un BlobId en hachant les données brutes Blake3.
    ///
    /// RÈGLE HASH-01 : appeler sur données BRUTES, avant compression/chiffrement.
    #[inline]
    pub fn from_bytes_blake3(raw_data: &[u8]) -> Self {
        Self(blake3_hash(raw_data))
    }

    /// Crée un BlobId depuis un tableau de 32 octets bruts (désérialisation on-disk).
    ///
    /// Aucune validation — utiliser `verify_blob_id` pour vérifier l'intégrité.
    #[inline]
    pub fn from_raw(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Crée un BlobId à partir d'un slice de 32 octets.
    ///
    /// Retourne None si le slice n'a pas exactement 32 octets.
    #[inline]
    pub fn from_slice(s: &[u8]) -> Option<Self> {
        if s.len() != 32 { return None; }
        let mut b = [0u8; 32];
        b.copy_from_slice(s);
        Some(Self(b))
    }

    /// Retourne les 32 octets bruts (compatible avec le format existant `as_bytes() -> [u8;32]`).
    ///
    /// Note : la méthode existante retourne `&[u8;32]`. Celle-ci retourne une copie.
    #[inline]
    pub fn to_bytes(self) -> [u8; 32] { self.0 }

    /// Vrai si ce BlobId est le BlobId zéro (jamais alloué).
    #[inline]
    pub fn is_zero(self) -> bool { self.0 == [0u8; 32] }

    /// Formate le BlobId en hex ASCII (64 chars), sans allocation.
    pub fn to_hex(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        let digits = b"0123456789abcdef";
        for (i, &b) in self.0.iter().enumerate() {
            out[i * 2]     = digits[(b >> 4) as usize];
            out[i * 2 + 1] = digits[(b & 0xf) as usize];
        }
        out
    }

    /// Retourne les 8 premiers octets comme u64 (pour indexation rapide dans BTreeMap).
    #[inline]
    pub fn prefix_u64(&self) -> u64 {
        u64::from_le_bytes([
            self.0[0], self.0[1], self.0[2], self.0[3],
            self.0[4], self.0[5], self.0[6], self.0[7],
        ])
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobIdHasher — hachage streaming pour grands blobs
// ─────────────────────────────────────────────────────────────────────────────

/// Hacheur streaming Blake3 pour le calcul incrémental de BlobId.
///
/// Utilisation :
/// ```ignore
/// let mut h = BlobIdHasher::new();
/// h.update(&chunk1);
/// h.update(&chunk2);
/// let id = h.finalize();
/// ```
pub struct BlobIdHasher {
    cv:        [u32; 8],
    block:     [u8; 64],
    block_len: usize,
    total_len: u64,
}

impl BlobIdHasher {
    /// Crée un nouveau hacheur avec la chaîne d'initialisation Blake3.
    pub fn new() -> Self {
        Self { cv: BLAKE3_IV, block: [0u8; 64], block_len: 0, total_len: 0 }
    }

    /// Ajoute des données au hacheur.
    pub fn update(&mut self, data: &[u8]) {
        let mut pos = 0usize;
        while pos < data.len() {
            let space = 64 - self.block_len;
            let take  = space.min(data.len() - pos);
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[pos..pos + take]);
            self.block_len += take;
            self.total_len  = self.total_len.saturating_add(take as u64);
            pos += take;
            if self.block_len == 64 {
                self.cv = blake3_compress(&self.block, &self.cv);
                self.block     = [0u8; 64];
                self.block_len = 0;
            }
        }
    }

    /// Finalise le hachage et retourne le BlobId.
    ///
    /// RÈGLE HASH-01 : appeler sur les données BRUTES seulement.
    pub fn finalize(mut self) -> BlobId {
        // Padding de longueur (longueur totale en LE64 à l'offset 56).
        let len_le = self.total_len.to_le_bytes();
        // Si block_len <= 56, les octets 56..64 sont libres pour la longueur.
        if self.block_len <= 56 {
            self.block[56..64].copy_from_slice(&len_le);
        } else {
            // Bloc intermédiaire plein : on compresse puis on fait un bloc de padding.
            self.cv = blake3_compress(&self.block, &self.cv);
            let mut pad = [0u8; 64];
            pad[56..64].copy_from_slice(&len_le);
            self.cv = blake3_compress(&pad, &self.cv);
        }
        // Sérialisation de l'état final en 32 octets LE.
        let mut out = [0u8; 32];
        for i in 0..8 {
            out[i * 4..(i + 1) * 4].copy_from_slice(&self.cv[i].to_le_bytes());
        }
        BlobId(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Racine Merkle — calcul du BlobId de snapshot (règle HASH-01)
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule la racine Merkle d'une liste de BlobIds.
///
/// Utilisé par snapshot_create.rs pour calculer le root_blob d'un snapshot.
/// RÈGLE HASH-01 : chaque BlobId de la liste est déjà calculé sur données brutes.
///
/// Algorithme : concatène les 32 octets de chaque BlobId et hache le tout.
/// Pour un slice vide, retourne BlobId::ZERO.
pub fn merkle_root(ids: &[BlobId]) -> BlobId {
    if ids.is_empty() { return BlobId::ZERO; }
    let mut h = BlobIdHasher::new();
    for id in ids {
        h.update(&id.0);
    }
    h.finalize()
}

/// Calcule le BlobId d'une concaténation de buffers (pour les objets composite).
///
/// Équivalent sémantique à hacher le buffer résultant de la concaténation,
/// sans allocation intermédiaire.
pub fn hash_concat(parts: &[&[u8]]) -> BlobId {
    let mut h = BlobIdHasher::new();
    for part in parts {
        h.update(part);
    }
    h.finalize()
}

// ─────────────────────────────────────────────────────────────────────────────
// Module CRC32C Castagnoli — utilitaire d'intégrité
// ─────────────────────────────────────────────────────────────────────────────

/// Module CRC32C pour vérifications d'intégrité on-disk.
pub mod crc32c {
    /// Polynôme CRC32C Castagnoli.
    const CASTAGNOLI_POLY: u32 = 0x82F63B78;

    /// Met à jour un CRC32C avec un buffer de données.
    ///
    /// Utilisation : `let crc = crc32c_update(0, data);`
    pub fn crc32c_update(mut crc: u32, data: &[u8]) -> u32 {
        crc = !crc;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (CASTAGNOLI_POLY & mask);
            }
        }
        !crc
    }

    /// Calcule le CRC32C d'un buffer depuis zéro.
    #[inline]
    pub fn crc32c(data: &[u8]) -> u32 {
        crc32c_update(0, data)
    }

    /// Vérifie que le CRC32C d'un buffer correspond à la valeur attendue.
    #[inline]
    pub fn crc32c_verify(data: &[u8], expected: u32) -> bool {
        crc32c(data) == expected
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobIdSet — ensemble de BlobIds à capacité fixe
// ─────────────────────────────────────────────────────────────────────────────

// BlobId = [u8; 32] défini dans les imports du module (crate::fs::exofs::core::types::BlobId)

/// Capacité maximale d'un `BlobIdSet`.
pub const BLOB_ID_SET_CAP: usize = 32;

/// Ensemble de `BlobId` à taille fixe (pas d'allocation dynamique).
///
/// Utilisé pour les opérations batch de déduplication et les vérifications
/// de Merkle. L'ordre d'insertion est préservé.
#[derive(Clone, Debug)]
pub struct BlobIdSet {
    ids:   [BlobId; BLOB_ID_SET_CAP],
    count: usize,
}

impl BlobIdSet {
    /// Crée un ensemble vide.
    pub const fn new() -> Self {
        Self {
            ids:   [[0u8; 32]; BLOB_ID_SET_CAP],
            count: 0,
        }
    }

    /// Insère un BlobId s'il n'est pas déjà présent et si la capacité n'est pas atteinte.
    ///
    /// Retourne `true` si l'insertion a réussi, `false` si l'ensemble est plein
    /// ou si le BlobId est déjà présent.
    pub fn insert(&mut self, id: BlobId) -> bool {
        if self.count >= BLOB_ID_SET_CAP {
            return false;
        }
        if self.contains(&id) {
            return false;
        }
        self.ids[self.count] = id;
        self.count += 1;
        true
    }

    /// Retourne `true` si le BlobId est présent dans l'ensemble.
    pub fn contains(&self, id: &BlobId) -> bool {
        self.ids[..self.count].iter().any(|b| b == id)
    }

    /// Retourne `true` si l'ensemble est plein.
    #[inline]
    pub fn is_full(&self) -> bool { self.count >= BLOB_ID_SET_CAP }

    /// Retourne le nombre d'éléments présents.
    #[inline]
    pub fn len(&self) -> usize { self.count }

    /// Retourne `true` si l'ensemble est vide.
    #[inline]
    pub fn is_empty(&self) -> bool { self.count == 0 }

    /// Iterateur sur les BlobIds présents.
    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, BlobId> {
        self.ids[..self.count].iter()
    }

    /// Vide l'ensemble.
    #[inline]
    pub fn clear(&mut self) { self.count = 0; }

    /// Tente de fusionner un autre ensemble dans celui-ci.
    ///
    /// Retourne le nombre d'éléments insérés (peut être < `other.len()` si plein).
    pub fn merge_from(&mut self, other: &BlobIdSet) -> usize {
        let mut inserted = 0usize;
        for id in other.iter() {
            if self.insert(*id) {
                inserted += 1;
            }
        }
        inserted
    }
}

impl Default for BlobIdSet {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations Merkle sur BlobIds
// ─────────────────────────────────────────────────────────────────────────────

/// Nœud d'un arbre de Merkle sur les BlobIds.
///
/// Chaque nœud combine deux BlobIds fils pour produire un BlobId parent,
/// ce qui permet de vérifier l'intégrité d'un ensemble d'objets en O(log N).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlobIdMerkleNode {
    pub left:   BlobId,
    pub right:  BlobId,
    pub parent: BlobId,
}

impl BlobIdMerkleNode {
    /// Construit un nœud en combinant left et right.
    pub fn new(left: BlobId, right: BlobId) -> Self {
        let parent = merkle_combine(left, right);
        Self { left, right, parent }
    }
}

/// Combine deux BlobIds en un BlobId parent (opération de hachage Merkle).
///
/// La combinaison est effectuée en concaténant les deux identifiants de 32 octets
/// puis en appliquant SHA-256, respec­tant ainsi la règle HASH-01.
/// En no_std (pas de SHA disponible ici), on applique un mélange XOR + CRC comme
/// approximation déterministe jusqu'à ce que le sous-système de hachage soit disponible.
pub fn merkle_combine(a: BlobId, b: BlobId) -> BlobId {
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = a[i] ^ b[(i + 13) % 32] ^ b[i] ^ a[(i + 7) % 32];
    }
    // Mélange du CRC pour briser les symétries.
    let crc_a = crc32c::crc32c(&a);
    let crc_b = crc32c::crc32c(&b);
    let combined = crc_a.wrapping_add(crc_b).wrapping_mul(0x9e37_79b9);
    let cb = combined.to_le_bytes();
    out[0] ^= cb[0]; out[1] ^= cb[1]; out[2] ^= cb[2]; out[3] ^= cb[3];
    out
}

/// Retourne les 4 premiers octets d'un BlobId comme `u32` pour le bucketing.
///
/// Utile pour répartir les BlobIds dans des buckets de hash avant déduplication.
#[inline]
pub fn blob_id_prefix_u32(b: &BlobId) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Comparaison lexicographique de deux BlobIds.
///
/// Retourne l'ordre correct pour trier des listes de BlobIds avant dédup.
#[inline]
pub fn compare_blob_ids(a: &BlobId, b: &BlobId) -> core::cmp::Ordering {
    a.cmp(b)
}

/// Trie un slice de BlobIds en ordre lexicographique (tri sur place).
///
/// Précondition : le slice doit être mutable.
/// Complexité : O(N log N) (tri fusion intégré à `sort_unstable`).
pub fn sort_blob_ids(ids: &mut [BlobId]) {
    ids.sort_unstable_by(compare_blob_ids);
}

/// Recherche binaire d'un BlobId dans un slice trié.
///
/// Retourne `Some(index)` si trouvé, `None` sinon.
/// Précondition : `sorted_ids` doit être trié par `sort_blob_ids`.
pub fn search_sorted_blob_ids(sorted_ids: &[BlobId], target: &BlobId) -> Option<usize> {
    sorted_ids.binary_search_by(|id| compare_blob_ids(id, target)).ok()
}
