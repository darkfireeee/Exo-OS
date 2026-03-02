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
    EPOCH_ROOT_MAGIC,
};
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::EpochRootPageHeader;

// ─────────────────────────────────────────────────────────────────────────────
// Checksum d'un EpochRecord
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le checksum Blake3 d'un EpochRecord.
///
/// Seuls les 72 premiers octets sont hachés (les 32 derniers sont le checksum).
/// Layout : bytes[0..72] = corps, bytes[72..104] = checksum.
pub fn compute_epoch_record_checksum(record: &EpochRecord) -> [u8; 32] {
    // SAFETY: EpochRecord est #[repr(C, packed)], Copy, taille 104 octets.
    // slice_from_raw_parts avec len=72 lit les 72 premiers octets uniquement.
    let ptr = record as *const EpochRecord as *const u8;
    let body = unsafe { core::slice::from_raw_parts(ptr, 72) };
    blake3_hash(body)
}

/// Vérifie le checksum d'un EpochRecord.
///
/// Retourne Ok(()) si le checksum est valide.
/// Retourne Err(ChecksumMismatch) sinon.
pub fn verify_epoch_record_checksum(record: &EpochRecord) -> ExofsResult<()> {
    let expected = compute_epoch_record_checksum(record);
    // SAFETY: même garantie que ci-dessus, slice[72..104] = les 32 octets du checksum.
    let ptr = record as *const EpochRecord as *const u8;
    let stored = unsafe { core::slice::from_raw_parts(ptr.add(72), 32) };
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= expected[i] ^ stored[i];
    }
    if diff != 0 { Err(ExofsError::ChecksumMismatch) } else { Ok(()) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Checksum d'une page EpochRoot (header + entries)
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule le checksum Blake3 du corps d'une page EpochRoot.
///
/// Le corps est (page_data.len() - 32) premiers octets.
/// Les 32 derniers octets sont réservés au stockage du checksum.
pub fn compute_epoch_root_page_checksum(page_data: &[u8]) -> ExofsResult<[u8; 32]> {
    let body_len = page_data
        .len()
        .checked_sub(32)
        .ok_or(ExofsError::CorruptedStructure)?;
    Ok(blake3_hash(&page_data[..body_len]))
}

/// Écrit le checksum Blake3 dans les 32 derniers octets d'une page EpochRoot.
pub fn seal_epoch_root_page(page_data: &mut [u8]) -> ExofsResult<()> {
    let body_len = page_data
        .len()
        .checked_sub(32)
        .ok_or(ExofsError::CorruptedStructure)?;
    let checksum = blake3_hash(&page_data[..body_len]);
    page_data[body_len..].copy_from_slice(&checksum);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification combinée magic + checksum
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie magic puis checksum d'une page (generic).
///
/// `magic_expected` : valeur attendue aux octets 0..4 de la page.
/// `page_data`      : contenu brut de la page (body + checksum final 32 octets).
pub fn verify_page_integrity(page_data: &[u8], magic_expected: u32) -> ExofsResult<()> {
    if page_data.len() < 36 {
        return Err(ExofsError::CorruptedStructure);
    }
    // Magic EN PREMIER (règle CHAIN-01).
    let magic = u32::from_le_bytes([page_data[0], page_data[1], page_data[2], page_data[3]]);
    if magic != magic_expected {
        return Err(ExofsError::InvalidMagic);
    }
    // Puis checksum.
    let body_len = page_data.len() - 32;
    let expected = blake3_hash(&page_data[..body_len]);
    let stored = &page_data[body_len..];
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= expected[i] ^ stored[i];
    }
    if diff != 0 {
        Err(ExofsError::ChecksumMismatch)
    } else {
        Ok(())
    }
}
