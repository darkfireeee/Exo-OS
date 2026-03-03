// kernel/src/fs/exofs/epoch/epoch_checksum.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Calcul et vérification des checksums structurels de l'epoch manager
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Centralise les appels à blake3_hash pour les structures epoch.
// Chaque utilisation documente PRÉCISÉMENT quels octets sont hachés.

use core::mem::size_of;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, blake3_hash,
    EPOCH_ROOT_MAGIC, EXOFS_MAGIC,
};
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::EpochRootPageHeader;

// =============================================================================
// Checksum d'un EpochRecord (104 octets, body = 72 B, checksum = 32 B)
// =============================================================================

/// Calcule le checksum Blake3 du corps d'un EpochRecord.
///
/// Le corps (body) est constitué des 72 premiers octets du record.
/// Les 32 derniers octets sont réservés au stockage du checksum calculé ici.
///
/// # Layout EpochRecord (104 octets)
/// ```text
/// [0..4]   magic          u32
/// [4..6]   version        u16
/// [6..8]   flags          u16
/// [8..16]  epoch_id       u64
/// [16..24] timestamp      u64
/// [24..56] root_oid       [u8;32]
/// [56..64] root_offset    u64
/// [64..72] prev_slot      u64
/// [72..76] object_count   u32     ← dans le body (72 octets)
/// [76..80] _pad           [u8;4]  ← dans le body
/// [80..104] checksum      [u8;32] ← NON inclus dans le hash (auto-référentiel)
/// ATTENTION : body = [0..72] seulement (pas les 4 bytes object_count/pad
///             qui sont aux offsets 72-80). Voir spec section 2.4.
/// ```
pub const EPOCH_RECORD_BODY_LEN: usize = 72;

/// Corps de l'EpochRecord utilisé pour le checksum.
///
/// Retourne les 72 premiers octets sous forme de tableau.
fn epoch_record_body(record: &EpochRecord) -> [u8; EPOCH_RECORD_BODY_LEN] {
    let mut body = [0u8; EPOCH_RECORD_BODY_LEN];
    // SAFETY: EpochRecord est #[repr(C, packed)], Copy, taille 104 octets.
    // La copie des 72 premiers octets lit magic..prev_slot.
    unsafe {
        core::ptr::copy_nonoverlapping(
            record as *const EpochRecord as *const u8,
            body.as_mut_ptr(),
            EPOCH_RECORD_BODY_LEN,
        );
    }
    body
}

/// Calcule le checksum Blake3 d'un EpochRecord.
///
/// Seuls les `EPOCH_RECORD_BODY_LEN` (72) premiers octets sont hachés.
pub fn compute_epoch_record_checksum(record: &EpochRecord) -> [u8; 32] {
    let body = epoch_record_body(record);
    blake3_hash(&body)
}

/// Vérifie le checksum Blake3 d'un EpochRecord (temps constant).
///
/// # Règles
/// - `V-13 / SEC-08` : comparaison en temps constant (XOR fold).
/// - `HDR-03` : appelé APRÈS vérification du magic.
///
/// # Retour
/// - `Ok(())` si le checksum est valide.
/// - `Err(ChecksumMismatch)` si altéré.
pub fn verify_epoch_record_checksum(record: &EpochRecord) -> ExofsResult<()> {
    let expected = compute_epoch_record_checksum(record);
    // Lecture du checksum stocké (octets 72..104 du record).
    let ptr = record as *const EpochRecord as *const u8;
    let stored: [u8; 32] = unsafe {
        let mut arr = [0u8; 32];
        core::ptr::copy_nonoverlapping(ptr.add(EPOCH_RECORD_BODY_LEN), arr.as_mut_ptr(), 32);
        arr
    };
    if !ct_eq_32(&expected, &stored) {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(())
}

// =============================================================================
// Checksum d'une page EpochRoot (layout : body_len - 32 B + checksum 32 B)
// =============================================================================

/// Calcule le checksum Blake3 du corps d'une page EpochRoot.
///
/// La page a le format : [body: page.len()-32 octets][checksum: 32 octets].
/// Le checksum couvre TOUS les octets du corps, y compris le champ `next_page`
/// de l'en-tête (RÈGLE CHAIN-01 : next_page inclus dans le checksum).
///
/// # Erreurs
/// - `CorruptedStructure` si la page est trop courte (<= 32 octets).
pub fn compute_epoch_root_page_checksum(page_data: &[u8]) -> ExofsResult<[u8; 32]> {
    let body_len = page_data
        .len()
        .checked_sub(32)
        .ok_or(ExofsError::CorruptedStructure)?;
    if body_len == 0 {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(blake3_hash(&page_data[..body_len]))
}

/// Scelle une page EpochRoot en écrivant le checksum Blake3 dans les 32 derniers octets.
///
/// Doit être appelé après que toutes les entrées ont été écrites dans la page,
/// et APRÈS avoir fixé le champ `next_page` (RÈGLE CHAIN-01).
///
/// # Erreurs
/// - `CorruptedStructure` si `page_data.len() <= 32`.
pub fn seal_epoch_root_page(page_data: &mut [u8]) -> ExofsResult<()> {
    let body_len = page_data
        .len()
        .checked_sub(32)
        .ok_or(ExofsError::CorruptedStructure)?;
    if body_len == 0 {
        return Err(ExofsError::CorruptedStructure);
    }
    let checksum = blake3_hash(&page_data[..body_len]);
    page_data[body_len..].copy_from_slice(&checksum);
    Ok(())
}

/// Vérifie le checksum d'une page EpochRoot lue depuis disque.
///
/// Retourne `Ok(())` si valide, `Err(ChecksumMismatch)` sinon.
pub fn verify_epoch_root_page_checksum(page_data: &[u8]) -> ExofsResult<()> {
    let body_len = page_data
        .len()
        .checked_sub(32)
        .ok_or(ExofsError::CorruptedStructure)?;
    let expected = blake3_hash(&page_data[..body_len]);
    let stored: [u8; 32] = {
        let slice = &page_data[body_len..];
        if slice.len() != 32 {
            return Err(ExofsError::CorruptedStructure);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        arr
    };
    if !ct_eq_32(&expected, &stored) {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(())
}

// =============================================================================
// Vérification combinée magic + checksum (generic)
// =============================================================================

/// Vérifie magic puis checksum d'une page quelconque.
///
/// Ordre de vérification (RÈGLE V-13 / CHAIN-01) :
/// 1. Magic EN PREMIER (4 octets LE à l'offset 0).
/// 2. Checksum Blake3 si magic valide.
///
/// # Paramètres
/// - `page_data`      : contenu brut de la page (body + checksum final 32 B).
/// - `magic_expected` : valeur attendue aux octets 0..4 (little-endian).
///
/// # Erreurs
/// - `CorruptedStructure` : page trop courte.
/// - `InvalidMagic`       : magic incorrect.
/// - `ChecksumMismatch`   : contenu altéré.
pub fn verify_page_integrity(page_data: &[u8], magic_expected: u32) -> ExofsResult<()> {
    if page_data.len() < 36 {
        return Err(ExofsError::CorruptedStructure);
    }
    // RÈGLE V-13 : magic EN PREMIER.
    let magic = u32::from_le_bytes([
        page_data[0], page_data[1], page_data[2], page_data[3],
    ]);
    if magic != magic_expected {
        return Err(ExofsError::InvalidMagic);
    }
    // Checksum sur le corps (tout sauf les 32 derniers octets).
    verify_epoch_root_page_checksum(page_data)
}

/// Variante pour les EpochRootPages (magic = EPOCH_ROOT_MAGIC).
#[inline]
pub fn verify_epoch_root_page_integrity(page_data: &[u8]) -> ExofsResult<()> {
    verify_page_integrity(page_data, EPOCH_ROOT_MAGIC)
}

/// Variante pour les pages superblock (magic = EXOFS_MAGIC).
#[inline]
pub fn verify_superblock_page_integrity(page_data: &[u8]) -> ExofsResult<()> {
    verify_page_integrity(page_data, EXOFS_MAGIC)
}

// =============================================================================
// Checksum incrémental (accumulateur Blake3 simplifié)
// =============================================================================

/// Contexte de checksum incrémental pour les structures variables.
///
/// Permet de calculer un Blake3 sur plusieurs slices discontiguës
/// sans copie intermédiaire.
///
/// Implémentation : accumulation dans un buffer de 4 KB, puis hash final.
/// Pour les structures > 4 KB, le hash est chaîné (hash des hashes).
pub struct IncrementalChecksum {
    /// Buffer d'accumulation.
    buf: [u8; 4096],
    /// Nombre d'octets accumulés dans `buf`.
    buf_pos: usize,
    /// Hash intermédiaire si le buffer a déjà été flushed.
    intermediate: Option<[u8; 32]>,
    /// Total d'octets traités.
    total_bytes: u64,
}

impl IncrementalChecksum {
    /// Crée un nouveau contexte de checksum incrémental.
    pub fn new() -> Self {
        IncrementalChecksum {
            buf:          [0u8; 4096],
            buf_pos:      0,
            intermediate: None,
            total_bytes:  0,
        }
    }

    /// Ajoute des données au contexte.
    pub fn update(&mut self, data: &[u8]) {
        let mut remaining = data;
        while !remaining.is_empty() {
            let space = 4096 - self.buf_pos;
            let to_copy = remaining.len().min(space);
            self.buf[self.buf_pos..self.buf_pos + to_copy]
                .copy_from_slice(&remaining[..to_copy]);
            self.buf_pos += to_copy;
            self.total_bytes = self.total_bytes.saturating_add(to_copy as u64);
            remaining = &remaining[to_copy..];

            if self.buf_pos == 4096 {
                self.flush_buffer();
            }
        }
    }

    /// Ajoute un u32 LE au contexte.
    pub fn update_u32(&mut self, val: u32) {
        self.update(&val.to_le_bytes());
    }

    /// Ajoute un u64 LE au contexte.
    pub fn update_u64(&mut self, val: u64) {
        self.update(&val.to_le_bytes());
    }

    /// Finalise le calcul et retourne le hash Blake3 de 32 octets.
    pub fn finalize(mut self) -> [u8; 32] {
        match self.intermediate {
            None => {
                // Tout tient dans le buffer.
                blake3_hash(&self.buf[..self.buf_pos])
            }
            Some(prev_hash) => {
                // Chaînage : hash(prev_hash || buf_tail).
                let mut combined = [0u8; 32 + 4096];
                combined[..32].copy_from_slice(&prev_hash);
                combined[32..32 + self.buf_pos]
                    .copy_from_slice(&self.buf[..self.buf_pos]);
                blake3_hash(&combined[..32 + self.buf_pos])
            }
        }
    }

    /// Flush le buffer interne et met à jour le hash intermédiaire.
    fn flush_buffer(&mut self) {
        let hash = blake3_hash(&self.buf[..self.buf_pos]);
        self.intermediate = Some(match self.intermediate {
            None => hash,
            Some(prev) => {
                let mut combined = [0u8; 64];
                combined[..32].copy_from_slice(&prev);
                combined[32..].copy_from_slice(&hash);
                blake3_hash(&combined)
            }
        });
        self.buf_pos = 0;
    }
}

// =============================================================================
// Utilitaires de comparaison temps constant
// =============================================================================

/// Comparaison en temps constant de deux tableaux de 32 octets.
///
/// RÈGLE SEC-08 : jamais de short-circuit sur les checksums.
/// Retourne `true` si les tableaux sont identiques.
#[inline]
pub fn ct_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut acc: u8 = 0;
    for i in 0..32 {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

/// Comparaison en temps constant de deux slices quelconques.
///
/// Retourne `false` immédiatement si les longueurs diffèrent (OK car la
/// longueur n'est pas secrète).
#[inline]
pub fn ct_eq_slice(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc: u8 = 0;
    for i in 0..a.len() {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

// =============================================================================
// Checksum d'un objet arbitraire (pour les structures on-disk)
// =============================================================================

/// Calcule le checksum Blake3 du contenu brut d'un objet on-disk.
///
/// `data` : slice des octets de l'objet AVANT le champ checksum.
/// Retourne le hash 32 octets à écrire dans le champ checksum de l'objet.
#[inline]
pub fn compute_struct_checksum(data: &[u8]) -> [u8; 32] {
    blake3_hash(data)
}

/// Vérifie le checksum d'une structure on-disk de format générique.
///
/// `data` : slice contenant le corps + les 32 derniers octets de checksum.
/// `body_len` : nombre d'octets du corps (doit être `data.len() - 32`).
pub fn verify_struct_checksum(data: &[u8], body_len: usize) -> ExofsResult<()> {
    if data.len() != body_len.saturating_add(32) {
        return Err(ExofsError::CorruptedStructure);
    }
    let expected = blake3_hash(&data[..body_len]);
    let stored: [u8; 32] = {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&data[body_len..]);
        arr
    };
    if !ct_eq_32(&expected, &stored) {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(())
}
