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
use core::fmt;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset, ObjectId,
    EXOFS_MAGIC, FORMAT_VERSION_MAJOR, blake3_hash,
};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::epoch::epoch_checksum::{ct_eq_32, EPOCH_RECORD_BODY_LEN};

// =============================================================================
// EpochRecord — structure on-disk EXACTEMENT 104 octets
// =============================================================================
//
// RÈGLE ONDISK-01 : #[repr(C, packed)] + types plain uniquement (pas d'AtomicU64).
// RÈGLE ONDISK-06 : const assert size_of::<EpochRecord>() == 104.
// RÈGLE V-08      : magic vérifié EN PREMIER avant tout accès au payload.
// RÈGLE SEC-08    : comparaison checksum en temps constant.
//
// Layout (104 octets) :
// ┌────────────┬──────┬─────────────────────────────────────────┐
// │ Offset     │ Size │ Champ                                   │
// ├────────────┼──────┼─────────────────────────────────────────┤
// │  0         │  4   │ magic   : u32 = 0x45584F46 ("EXOF")     │
// │  4         │  2   │ version : u16                           │
// │  6         │  2   │ flags   : u16 (EpochFlags)              │
// │  8         │  8   │ epoch_id: u64 (monotone croissant)      │
// │ 16         │  8   │ timestamp: u64 (TSC au commit)          │
// │ 24         │ 32   │ root_oid: [u8;32] (ObjectId EpochRoot)  │
// │ 56         │  8   │ root_offset: u64 (offset disque)        │
// │ 64         │  8   │ prev_slot: u64 (double-link recovery)   │
// │ 72         │  4   │ object_count: u32                       │
// │ 76         │  4   │ _pad: [u8;4] (zéros)                    │
// │ 80 (=body) │ ---  │ ← fin du corps haché (72 octets)        │
// │ 80         │ 32   │ checksum: [u8;32] Blake3(octets 0..72?) │
// └────────────┴──────┴─────────────────────────────────────────┘
// ATTENTION : body = [0..72] ← pas [0..80]. Voir epoch_checksum.rs.
// Total : 80 + 32 = mais spec dit 104 → body = 72 B checksum seul.
// Vérifié : 4+2+2+8+8+32+8+8+4+4 = 80 ; checksum = 32 → total = 112.
// CORRECTION spec : body = 72 ; total = 72 + 4 + 4 + 32 = 112? Non.
// On tient 104 B : body = 72 B (magic..prev_slot), object_count+pad dans checksum? Non.
// => body = magic+ver+flags+epoch_id+ts+root_oid+root_offset+prev_slot = 4+2+2+8+8+32+8+8=72
//    puis object_count(4) + _pad(4) = 8 B non dans le hash?
//    puis checksum = 32 B → total = 72+8+32 = 112? Spec dit 104!
// => Résolution : spec 2.4 dit 104 B. On exclut object_count/pad de la spec,
//    ils sont merged dans le body (72B body inclut rien d'autre que les 72 B).
//    Donc : magic(4)+ver(2)+flags(2)+epoch_id(8)+ts(8)+root_oid(32)+root_offset(8)+prev_slot(8)=72
//    body = 72 B → checksum = Blake3(72 B)
//    Struct finale = body(72) + checksum(32) = 104 B. object_count supprimé.

/// Enregistrement d'un Epoch committé — écrit dans un Slot (A/B/C).
///
/// # Invariant
/// `size_of::<EpochRecord>() == 104` (vérifié statiquement à la compilation).
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct EpochRecord {
    /// Magic ExoFS : 0x45584F46 ("EXOF").
    pub magic:       u32,
    /// Version du format.
    pub version:     u16,
    /// Flags (EpochFlags).
    pub flags:       u16,
    /// Identifiant d'epoch (monotone croissant).
    pub epoch_id:    u64,
    /// Timestamp TSC au moment du commit.
    pub timestamp:   u64,
    /// ObjectId de l'EpochRoot (32 octets).
    pub root_oid:    [u8; 32],
    /// Offset disque de l'EpochRoot.
    pub root_offset: u64,
    /// Offset du slot précédent (double-link pour le recovery chaîné).
    pub prev_slot:   u64,
    /// Blake3 des 72 premiers octets (corps body).
    pub checksum:    [u8; 32],
}

// Vérification statique de la taille : EXACTEMENT 104 octets.
const _: () = assert!(
    size_of::<EpochRecord>() == 104,
    "EpochRecord: la taille doit être exactement 104 octets"
);

impl EpochRecord {
    // =========================================================================
    // Construction
    // =========================================================================

    /// Crée un EpochRecord valide avec magic, version et checksum calculés.
    ///
    /// # Paramètres
    /// - `epoch_id`    : identifiant de l'epoch committé (doit être > 0).
    /// - `flags`       : flags de l'epoch (voir EpochFlags).
    /// - `timestamp`   : valeur TSC au moment du commit.
    /// - `root_oid`    : ObjectId de l'EpochRoot écrit sur disque.
    /// - `root_offset` : offset disque de l'EpochRoot.
    /// - `prev_slot`   : offset du slot précédent (0 si premier epoch).
    pub fn new(
        epoch_id:    EpochId,
        flags:       EpochFlags,
        timestamp:   u64,
        root_oid:    ObjectId,
        root_offset: DiskOffset,
        prev_slot:   DiskOffset,
    ) -> Self {
        let mut rec = Self {
            magic:       EXOFS_MAGIC,
            version:     FORMAT_VERSION_MAJOR,
            flags:       flags.0,
            epoch_id:    epoch_id.0,
            timestamp,
            root_oid:    root_oid.0,
            root_offset: root_offset.0,
            prev_slot:   prev_slot.0,
            checksum:    [0u8; 32],
        };
        // Calcul du checksum sur les 72 premiers octets (body).
        let body = rec.body_bytes();
        rec.checksum = blake3_hash(&body);
        rec
    }

    /// Crée un record "zéro" (slot vide — magic = 0, checksum invalide).
    ///
    /// Utilisé pour initialiser/effacer un slot.
    pub fn zeroed() -> Self {
        // SAFETY: EpochRecord est #[repr(C, packed)] composé uniquement de types
        // primitifs. Un zéro binaire est un état "vide" valide (magic=0 ≠ EXOFS_MAGIC).
        unsafe { core::mem::zeroed() }
    }

    // =========================================================================
    // Sérialisation / désérialisation
    // =========================================================================

    /// Retourne les 104 octets du record sous forme de tableau.
    pub fn to_bytes(self) -> [u8; 104] {
        let mut buf = [0u8; 104];
        // SAFETY: EpochRecord est Copy + #[repr(C, packed)] de taille 104 B.
        unsafe {
            core::ptr::copy_nonoverlapping(
                &self as *const Self as *const u8,
                buf.as_mut_ptr(),
                104,
            );
        }
        buf
    }

    /// Parse un EpochRecord depuis 104 octets bruts.
    ///
    /// Étape 1 : lecture du magic (RÈGLE V-08).
    /// Étape 2 : vérification du checksum.
    ///
    /// # Retour
    /// - `Ok(None)` : slot vide (magic = 0x00000000 ou 0xFFFFFFFF).
    /// - `Ok(Some(r))` : record valide.
    /// - `Err(InvalidMagic)` : magic incorrect (slot corrompu).
    /// - `Err(ChecksumMismatch)` : contenu altéré.
    pub fn from_bytes(data: &[u8; 104]) -> ExofsResult<Option<Self>> {
        // RÈGLE V-08 : magic EN PREMIER.
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic == 0x0000_0000 || magic == 0xFFFF_FFFF {
            // Slot vide ou NAND effacé.
            return Ok(None);
        }
        if magic != EXOFS_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        // SAFETY: data est un tableau de 104 octets, EpochRecord est 104 B.
        let record: Self = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Self)
        };
        // Vérification du checksum.
        record.verify_checksum()?;
        Ok(Some(record))
    }

    // =========================================================================
    // Vérification
    // =========================================================================

    /// Vérifie l'intégrité complète du record (magic + checksum).
    ///
    /// # Ordre
    /// 1. Magic EN PREMIER (RÈGLE V-08, SEC-08).
    /// 2. Checksum Blake3 en temps constant (RÈGLE SEC-08).
    pub fn verify(&self) -> ExofsResult<()> {
        // RÈGLE V-08 / CHAIN-01 : magic EN PREMIER.
        let magic = { self.magic }; // Copie locale pour éviter unaligned read.
        if magic != EXOFS_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        self.verify_checksum()
    }

    /// Vérifie uniquement le checksum (après que le magic a été validé).
    fn verify_checksum(&self) -> ExofsResult<()> {
        let body = self.body_bytes();
        let expected = blake3_hash(&body);
        let stored = self.checksum;
        if !ct_eq_32(&expected, &stored) {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Extrait les 72 octets du corps (magic..prev_slot) pour le hash.
    fn body_bytes(&self) -> [u8; EPOCH_RECORD_BODY_LEN] {
        let mut body = [0u8; EPOCH_RECORD_BODY_LEN];
        // SAFETY: self est #[repr(C, packed)], taille 104 B.
        // Les 72 premiers octets sont magic(4)+ver(2)+flags(2)+e(8)+ts(8)+roid(32)+ro(8)+ps(8).
        unsafe {
            core::ptr::copy_nonoverlapping(
                self as *const Self as *const u8,
                body.as_mut_ptr(),
                EPOCH_RECORD_BODY_LEN,
            );
        }
        body
    }

    // =========================================================================
    // Accesseurs (copies locales pour éviter les unaligned reads)
    // =========================================================================

    /// EpochId du record (copie locale — évite unaligned read sur packed struct).
    #[inline]
    pub fn epoch_id(&self) -> EpochId {
        EpochId({ self.epoch_id })
    }

    /// ObjectId de l'EpochRoot.
    #[inline]
    pub fn root_oid(&self) -> ObjectId {
        ObjectId(self.root_oid)
    }

    /// Offset disque de l'EpochRoot.
    #[inline]
    pub fn root_offset(&self) -> DiskOffset {
        DiskOffset({ self.root_offset })
    }

    /// Offset du slot précédent.
    #[inline]
    pub fn prev_slot(&self) -> DiskOffset {
        DiskOffset({ self.prev_slot })
    }

    /// Flags de l'epoch.
    #[inline]
    pub fn flags(&self) -> EpochFlags {
        EpochFlags({ self.flags })
    }

    /// Timestamp TSC.
    #[inline]
    pub fn timestamp(&self) -> u64 {
        { self.timestamp }
    }

    /// Version du format.
    #[inline]
    pub fn version(&self) -> u16 {
        { self.version }
    }

    /// Vrai si l'epoch a le flag RECOVERING (crash précédent non terminé).
    #[inline]
    pub fn is_recovering(&self) -> bool {
        let f = { self.flags };
        EpochFlags(f).contains(EpochFlags::RECOVERING)
    }

    /// Vrai si l'epoch a le flag COMMITTED (commit trois-barrières réussi).
    #[inline]
    pub fn is_committed(&self) -> bool {
        let f = { self.flags };
        EpochFlags(f).contains(EpochFlags::COMMITTED)
    }

    /// Vrai si l'epoch est marqué comme snapshot permanent.
    #[inline]
    pub fn is_snapshot(&self) -> bool {
        let f = { self.flags };
        EpochFlags(f).contains(EpochFlags::SNAPSHOT)
    }

    /// Vrai si l'epoch contient des suppressions d'objets.
    #[inline]
    pub fn has_deletions(&self) -> bool {
        let f = { self.flags };
        EpochFlags(f).contains(EpochFlags::HAS_DELETIONS)
    }

    // =========================================================================
    // Utilitaires de comparaison
    // =========================================================================

    /// Compare deux EpochRecords par epoch_id (pour sélectionner le plus récent).
    pub fn is_newer_than(&self, other: &Self) -> bool {
        let self_id  = { self.epoch_id };
        let other_id = { other.epoch_id };
        self_id > other_id
    }

    /// Retourne le record avec le plus grand epoch_id.
    pub fn newest<'a>(a: &'a Self, b: &'a Self) -> &'a Self {
        if a.is_newer_than(b) { a } else { b }
    }
}

// =============================================================================
// EpochRecordBuilder — pattern Builder pour construire un record de manière sûre
// =============================================================================

/// Builder pour créer un EpochRecord étape par étape.
///
/// Valide les champs obligatoires avant de produire le record final.
pub struct EpochRecordBuilder {
    epoch_id:    Option<EpochId>,
    flags:       EpochFlags,
    timestamp:   u64,
    root_oid:    Option<ObjectId>,
    root_offset: Option<DiskOffset>,
    prev_slot:   DiskOffset,
}

impl EpochRecordBuilder {
    /// Crée un builder vide.
    pub fn new() -> Self {
        EpochRecordBuilder {
            epoch_id:    None,
            flags:       EpochFlags::default(),
            timestamp:   0,
            root_oid:    None,
            root_offset: None,
            prev_slot:   DiskOffset(0),
        }
    }

    /// Définit l'EpochId (obligatoire, doit être > 0).
    pub fn epoch_id(mut self, id: EpochId) -> Self {
        self.epoch_id = Some(id);
        self
    }

    /// Définit les flags de l'epoch.
    pub fn flags(mut self, f: EpochFlags) -> Self {
        self.flags = f;
        self
    }

    /// Définit le timestamp TSC.
    pub fn timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }

    /// Définit l'ObjectId de l'EpochRoot.
    pub fn root_oid(mut self, oid: ObjectId) -> Self {
        self.root_oid = Some(oid);
        self
    }

    /// Définit l'offset disque de l'EpochRoot.
    pub fn root_offset(mut self, off: DiskOffset) -> Self {
        self.root_offset = Some(off);
        self
    }

    /// Définit l'offset du slot précédent.
    pub fn prev_slot(mut self, off: DiskOffset) -> Self {
        self.prev_slot = off;
        self
    }

    /// Marque l'epoch comme committé (ajoute le flag COMMITTED).
    pub fn mark_committed(mut self) -> Self {
        self.flags.set(EpochFlags::COMMITTED);
        self
    }

    /// Construit l'EpochRecord.
    ///
    /// # Erreurs
    /// - `InvalidEpochId` si epoch_id n'est pas défini ou vaut 0.
    /// - `CorruptedStructure` si root_oid ou root_offset manquent.
    pub fn build(self) -> ExofsResult<EpochRecord> {
        let epoch_id = self.epoch_id.ok_or(ExofsError::InvalidEpochId)?;
        if epoch_id.0 == 0 {
            return Err(ExofsError::InvalidEpochId);
        }
        let root_oid    = self.root_oid.ok_or(ExofsError::CorruptedStructure)?;
        let root_offset = self.root_offset.ok_or(ExofsError::CorruptedStructure)?;

        Ok(EpochRecord::new(
            epoch_id,
            self.flags,
            self.timestamp,
            root_oid,
            root_offset,
            self.prev_slot,
        ))
    }
}

// =============================================================================
// Debug / Display
// =============================================================================

impl fmt::Debug for EpochRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let epoch_id    = { self.epoch_id };
        let magic       = { self.magic };
        let root_offset = { self.root_offset };
        let flags       = { self.flags };
        let timestamp   = { self.timestamp };
        f.debug_struct("EpochRecord")
            .field("magic",       &format_args!("0x{:08X}", magic))
            .field("epoch_id",    &epoch_id)
            .field("flags",       &format_args!("0x{:04X}", flags))
            .field("timestamp",   &timestamp)
            .field("root_offset", &root_offset)
            .finish()
    }
}

impl fmt::Display for EpochRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let epoch_id = { self.epoch_id };
        let committed = if self.is_committed() { "C" } else { "-" };
        let recovering = if self.is_recovering() { "R" } else { "-" };
        let snapshot = if self.is_snapshot() { "S" } else { "-" };
        write!(f, "Epoch[{}] flags=[{}{}{}]", epoch_id, committed, recovering, snapshot)
    }
}

// =============================================================================
// Utilitaires de manipulation des slots (3 records à gérer ensemble)
// =============================================================================

/// Sélectionne le meilleur parmi jusqu'à 3 EpochRecords optionnels.
///
/// Critère : max(epoch_id) parmi les Some(r) où r.verify() == Ok.
/// Retourne None si tous sont None ou invalides.
pub fn select_best_record(
    a: Option<&EpochRecord>,
    b: Option<&EpochRecord>,
    c: Option<&EpochRecord>,
) -> Option<EpochRecord> {
    let candidates = [a, b, c];
    let mut best: Option<EpochRecord> = None;

    for opt in &candidates {
        if let Some(rec) = opt {
            if rec.verify().is_ok() {
                match &best {
                    None => best = Some(**rec),
                    Some(b) if rec.is_newer_than(b) => best = Some(**rec),
                    _ => {}
                }
            }
        }
    }
    best
}

/// Compte combien de records parmi les 3 sont valides (magic + checksum OK).
pub fn count_valid_records(
    a: Option<&EpochRecord>,
    b: Option<&EpochRecord>,
    c: Option<&EpochRecord>,
) -> u8 {
    [a, b, c]
        .iter()
        .filter(|opt| opt.map(|r| r.verify().is_ok()).unwrap_or(false))
        .count() as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// EpochRecord — 104 octets, struct on-disk
// ─────────────────────────────────────────────────────────────────────────────

// Enregistrement d'un Epoch committé — écrit dans un Slot (A/B/C). [TODO: struct tronquée]
