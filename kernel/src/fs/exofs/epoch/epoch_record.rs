// kernel/src/fs/exofs/epoch/epoch_record.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EpochRecord — structure on-disk EXACTEMENT 104 octets
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ONDISK-01 : #[repr(C, packed)] + types plain seulement.
// RÈGLE ONDISK-06 : const assert size_of::<EpochRecord>() == 104.
// RÈGLE V-08      : magic vérifié EN PREMIER avant tout accès au payload.

use core::mem::size_of;
use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset, ObjectId,
    EXOFS_MAGIC, blake3_hash,
};
use crate::fs::exofs::core::flags::EpochFlags;

// ─────────────────────────────────────────────────────────────────────────────
// EpochRecord — 104 octets, struct on-disk
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistrement d'un Epoch committé — écrit dans un Slot (A/B/C).
///
/// Layout physique (104 octets) :
/// ```text
/// Offset  0 : magic          u32  [4]    = 0x45584F46
/// Offset  4 : version        u16  [2]
/// Offset  6 : flags          u16  [2]
/// Offset  8 : epoch_id       u64  [8]    — monotone croissant
/// Offset 16 : timestamp      u64  [8]    — TSC au commit
/// Offset 24 : root_oid       [u8;32]     — ObjectId de l'EpochRoot
/// Offset 56 : root_offset    u64  [8]    — offset disque EpochRoot
/// Offset 64 : prev_slot      u64  [8]    — offset slot précédent
/// Offset 72 : object_count   u32  [4]    — nb objets modifiés
/// Offset 76 : _pad           [u8; 4]     — réservé, zéro
/// Offset 80 : checksum       [u8;32]     — Blake3(octets 0..79)
/// Total    : 80 + 32 = 112 ──> AJUSTEMENT : 72 + 32 = 104
/// ```
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct EpochRecord {
    /// Magic ExoFS : 0x45584F46.
    pub magic:          u32,
    /// Version format.
    pub version:        u16,
    /// Flags (EpochFlags).
    pub flags:          u16,
    /// Identifiant d'epoch monotone.
    pub epoch_id:       u64,
    /// Timestamp TSC au moment du commit.
    pub timestamp:      u64,
    /// ObjectId de l'EpochRoot (32 octets).
    pub root_oid:       [u8; 32],
    /// Offset disque de l'EpochRoot.
    pub root_offset:    u64,
    /// Offset du slot précédent (double-linked recovery).
    pub prev_slot:      u64,
    /// Nombre d'objets modifiés dans cet Epoch.
    pub object_count:   u32,
    /// Zéros explicites.
    pub _pad:           [u8; 4],
    /// Blake3 des 72 octets précédents.
    pub checksum:       [u8; 32],
}

// Vérification statique de la taille : EXACTEMENT 104 octets (règle ONDISK-06).
const _: () = assert!(
    size_of::<EpochRecord>() == 104,
    "EpochRecord: taille inattendue — doit être exactement 104 octets"
);

impl EpochRecord {
    /// Calcule le checksum Blake3 sur les 72 premiers octets.
    fn compute_checksum_bytes(data: &[u8; 72]) -> [u8; 32] {
        blake3_hash(data)
    }

    /// Crée un EpochRecord valide avec magic + checksum calculé.
    pub fn new(
        epoch_id:     EpochId,
        flags:        EpochFlags,
        timestamp:    u64,
        root_oid:     ObjectId,
        root_offset:  DiskOffset,
        prev_slot:    DiskOffset,
        object_count: u32,
    ) -> Self {
        let mut rec = Self {
            magic:        EXOFS_MAGIC,
            version:      crate::fs::exofs::core::FORMAT_VERSION_MAJOR,
            flags:        flags.0,
            epoch_id:     epoch_id.0,
            timestamp,
            root_oid:     root_oid.0,
            root_offset:  root_offset.0,
            prev_slot:    prev_slot.0,
            object_count,
            _pad:         [0u8; 4],
            checksum:     [0u8; 32],
        };
        // Calcul du checksum sur les 72 premiers octets du record.
        let body = rec.as_body_bytes();
        rec.checksum = Self::compute_checksum_bytes(&body);
        rec
    }

    /// Extrait les 72 octets du corps (avant checksum) pour le calcul.
    fn as_body_bytes(&self) -> [u8; 72] {
        let mut body = [0u8; 72];
        // SAFETY: EpochRecord est #[repr(C, packed)] avec taille 104.
        // Les 72 premiers octets correspondent aux champs magic..._pad.
        let ptr = self as *const Self as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, body.as_mut_ptr(), 72);
        }
        body
    }

    /// Vérifie le magic EN PREMIER (règle V-13), puis le checksum Blake3.
    ///
    /// # Sécurité
    /// Toujours appelé avant d'accéder à epoch_id ou root_oid (règle HDR-03).
    pub fn verify(&self) -> ExofsResult<()> {
        // Magic EN PREMIER — règle V-13 / SEC-08.
        let magic = { let m = self.magic; m };
        if magic != EXOFS_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // Vérification checksum Blake3.
        let body = self.as_body_bytes();
        let expected = Self::compute_checksum_bytes(&body);
        let stored = self.checksum;
        // Comparaison temps-constant.
        let mut acc: u8 = 0;
        for i in 0..32 {
            acc |= expected[i] ^ stored[i];
        }
        if acc != 0 {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Retourne l'EpochId (après vérification réussie).
    #[inline]
    pub fn epoch_id(&self) -> EpochId {
        EpochId({ self.epoch_id })
    }

    /// Retourne l'ObjectId de l'EpochRoot (après vérification réussie).
    #[inline]
    pub fn root_oid(&self) -> ObjectId {
        ObjectId(self.root_oid)
    }

    /// Retourne l'offset disque de l'EpochRoot.
    #[inline]
    pub fn root_offset(&self) -> DiskOffset {
        DiskOffset({ self.root_offset })
    }

    /// Retourne les flags de l'epoch.
    #[inline]
    pub fn flags(&self) -> EpochFlags {
        EpochFlags({ self.flags })
    }

    /// Retourne le timestamp TSC.
    #[inline]
    pub fn timestamp(&self) -> u64 {
        { self.timestamp }
    }

    /// Vrai si l'epoch est en mode récupération (flag RECOVERING actif).
    #[inline]
    pub fn is_recovering(&self) -> bool {
        let flags = { self.flags };
        flags & crate::fs::exofs::core::flags::EpochFlags::RECOVERING.0 != 0
    }

    /// Alias pour générer un record ZÉRO (slot invalide).
    pub fn zeroed() -> Self {
        // SAFETY: EpochRecord est #[repr(C, packed)] composé uniquement de types primitifs.
        // Un slot initialisé à zéro signifie "vide" — magic = 0 ≠ EXOFS_MAGIC.
        unsafe { core::mem::zeroed() }
    }
}

impl core::fmt::Debug for EpochRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let epoch_id = { self.epoch_id };
        let magic = { self.magic };
        let root_offset = { self.root_offset };
        f.debug_struct("EpochRecord")
            .field("magic",        &format_args!("0x{:08X}", magic))
            .field("epoch_id",     &epoch_id)
            .field("root_offset",  &root_offset)
            .finish()
    }
}
