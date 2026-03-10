//! recovery_audit.rs — Trail d'audit structuré pour les opérations de récupération ExoFS.
//!
//! Enregistre des événements d'audit détaillés (début/fin de phase, réparations,
//! violations d'intégrité) dans un anneau statique. Distinct de `recovery_log`
//! qui est orienté performance ; `recovery_audit` est orienté conformité et trace.
//!
//! # Règles spec appliquées
//! - **OOM-02** : `try_reserve(1)` avant tout `Vec::push()`.
//! - **ARITH-02** : `checked_add` / `wrapping_add` pour les index.
//! - **ONDISK-03** : pas d'`AtomicU64` dans les structs `repr(C)`.


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult, EpochId};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Capacité du ring buffer d'audit (puissance de 2).
pub const AUDIT_RING_CAPACITY: usize = 512;
const AUDIT_MASK: usize = AUDIT_RING_CAPACITY - 1;

const _: () = assert!(
    AUDIT_RING_CAPACITY.is_power_of_two(),
    "AUDIT_RING_CAPACITY doit être une puissance de 2"
);

// ── Singleton global ──────────────────────────────────────────────────────────

/// Trail d'audit global de récupération.
pub static RECOVERY_AUDIT: RecoveryAudit = RecoveryAudit::new_const();

// ── Types d'événements d'audit ────────────────────────────────────────────────

/// Type d'un événement d'audit de récupération.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditEventKind {
    /// Début de la séquence de récupération.
    RecoveryStarted       = 0x01,
    /// Fin de la séquence de récupération (succès).
    RecoveryCompleted     = 0x02,
    /// Fin de la séquence de récupération (échec).
    RecoveryFailed        = 0x03,
    /// Début d'une phase fsck.
    FsckPhaseStarted      = 0x10,
    /// Fin d'une phase fsck sans erreurs.
    FsckPhaseClean        = 0x11,
    /// Fin d'une phase fsck avec erreurs.
    FsckPhaseErrors       = 0x12,
    /// Réparation de métadonnées appliquée.
    MetadataRepaired      = 0x20,
    /// Blob orphan récupéré vers lost+found.
    OrphanRecovered       = 0x21,
    /// Entrée journal corrompue ignorée.
    CorruptJournalSkipped = 0x22,
    /// Slot sélectionné lors du boot.
    SlotSelected          = 0x30,
    /// Epoch rejoué lors du boot.
    EpochReplayed         = 0x31,
    /// Checkpoint sauvegardé.
    CheckpointSaved       = 0x40,
    /// Checkpoint restauré.
    CheckpointRestored    = 0x41,
    /// Magic invalide détecté sur en-tête.
    InvalidMagicDetected  = 0x50,
    /// Checksum invalide détecté.
    ChecksumInvalid       = 0x51,
    /// Violation d'intégrité structurelle.
    StructureCorrupted    = 0x52,
    /// Accès hors-borne ou offset invalide.
    OffsetOutOfBounds     = 0x53,
    /// Réparation annulée (mode dry-run).
    RepairSkippedDryRun   = 0x60,
    /// Évènement personnalisé / extension.
    Custom                = 0xFF,
}

impl AuditEventKind {
    /// `true` si l'évènement indique une anomalie d'intégrité.
    #[inline]
    pub fn is_integrity_violation(&self) -> bool {
        matches!(
            self,
            Self::InvalidMagicDetected
                | Self::ChecksumInvalid
                | Self::StructureCorrupted
                | Self::OffsetOutOfBounds
        )
    }

    /// `true` si l'évènement indique une réparation effectuée.
    #[inline]
    pub fn is_repair(&self) -> bool {
        matches!(
            self,
            Self::MetadataRepaired | Self::OrphanRecovered | Self::CorruptJournalSkipped
        )
    }

    /// `true` si l'évènement clos la séquence de récupération.
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::RecoveryCompleted | Self::RecoveryFailed)
    }

    /// Conversion depuis u8 (fallback = `Custom`).
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::RecoveryStarted,
            0x02 => Self::RecoveryCompleted,
            0x03 => Self::RecoveryFailed,
            0x10 => Self::FsckPhaseStarted,
            0x11 => Self::FsckPhaseClean,
            0x12 => Self::FsckPhaseErrors,
            0x20 => Self::MetadataRepaired,
            0x21 => Self::OrphanRecovered,
            0x22 => Self::CorruptJournalSkipped,
            0x30 => Self::SlotSelected,
            0x31 => Self::EpochReplayed,
            0x40 => Self::CheckpointSaved,
            0x41 => Self::CheckpointRestored,
            0x50 => Self::InvalidMagicDetected,
            0x51 => Self::ChecksumInvalid,
            0x52 => Self::StructureCorrupted,
            0x53 => Self::OffsetOutOfBounds,
            0x60 => Self::RepairSkippedDryRun,
            _    => Self::Custom,
        }
    }
}

// ── Niveau de sévérité ────────────────────────────────────────────────────────

/// Niveau de sévérité d'un événement d'audit.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
    /// Information banale.
    Info     = 0,
    /// Avertissement : état dégradé mais récupérable.
    Warning  = 1,
    /// Erreur : anomalie corrigée ou ignorée.
    Error    = 2,
    /// Critique : données potentiellement perdues.
    Critical = 3,
}

impl AuditSeverity {
    /// Conversion depuis u8 (fallback = `Info`).
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Warning,
            2 => Self::Error,
            3 => Self::Critical,
            _ => Self::Info,
        }
    }
}

// ── Entrée d'audit ─────────────────────────────────────────────────────────────

/// Un événement d'audit de récupération.
///
/// Taille fixe : 64 octets.
#[derive(Clone, Copy, Debug)]
pub struct AuditEntry {
    /// Horodatage TSC.
    pub tick:     u64,
    /// Type de l'événement.
    pub kind:     AuditEventKind,
    /// Niveau de sévérité.
    pub severity: AuditSeverity,
    /// Rembourrage.
    pub _pad:     [u8; 6],
    /// EpochId associée (0 si N/A).
    pub epoch_id: u64,
    /// Identifiant secondaire (slot_id, phase, checkpoint_id, etc.).
    pub target:   u64,
    /// Nombre d'occurrences ou d'erreurs rattachées.
    pub count:    u32,
    /// Code erreur ExofsError (0 = aucun).
    pub err_code: u32,
    /// Informations complémentaires sur 16 octets.
    pub detail:   [u8; 16],
}

impl AuditEntry {
    /// Entrée nulle (valeur d'initialisation).
    pub const fn zeroed() -> Self {
        Self {
            tick:     0,
            kind:     AuditEventKind::Custom,
            severity: AuditSeverity::Info,
            _pad:     [0; 6],
            epoch_id: 0,
            target:   0,
            count:    0,
            err_code: 0,
            detail:   [0; 16],
        }
    }

    /// Construit une entrée d'information simple.
    pub fn info(kind: AuditEventKind, epoch_id: u64, target: u64) -> Self {
        Self {
            tick:     crate::arch::time::read_ticks(),
            kind,
            severity: AuditSeverity::Info,
            _pad:     [0; 6],
            epoch_id,
            target,
            count:    0,
            err_code: 0,
            detail:   [0; 16],
        }
    }

    /// Construit une entrée d'erreur avec code et compteur.
    pub fn error(
        kind:     AuditEventKind,
        severity: AuditSeverity,
        epoch_id: u64,
        target:   u64,
        count:    u32,
        err_code: u32,
    ) -> Self {
        Self {
            tick: crate::arch::time::read_ticks(),
            kind,
            severity,
            _pad: [0; 6],
            epoch_id,
            target,
            count,
            err_code,
            detail: [0; 16],
        }
    }

    /// Construit une entrée avec 16 octets de détail.
    pub fn with_detail(
        kind:     AuditEventKind,
        severity: AuditSeverity,
        epoch_id: u64,
        target:   u64,
        detail:   [u8; 16],
    ) -> Self {
        Self {
            tick: crate::arch::time::read_ticks(),
            kind,
            severity,
            _pad: [0; 6],
            epoch_id,
            target,
            count: 0,
            err_code: 0,
            detail,
        }
    }
}

// ── Slot de ring buffer ───────────────────────────────────────────────────────

struct AuditSlot(core::cell::UnsafeCell<AuditEntry>);

impl AuditSlot {
    const fn new() -> Self {
        Self(core::cell::UnsafeCell::new(AuditEntry::zeroed()))
    }

    /// # Safety : index réservé via `head.fetch_add`.
    unsafe fn write(&self, entry: AuditEntry) {
        self.0.get().write(entry);
    }

    /// # Safety : lecture diagnostique sans synchronisation supplémentaire.
    unsafe fn read(&self) -> AuditEntry {
        self.0.get().read()
    }
}

unsafe impl Sync for AuditSlot {}

// ── Structure principale ──────────────────────────────────────────────────────

/// Trail d'audit circulaire lock-free pour la récupération ExoFS.
pub struct RecoveryAudit {
    ring:             [AuditSlot; AUDIT_RING_CAPACITY],
    head:             AtomicU64,
    total:            AtomicU64,
    violation_count:  AtomicUsize,
    repair_count:     AtomicUsize,
    critical_count:   AtomicUsize,
}

impl RecoveryAudit {
    /// Construit un trail vide (utilisable dans un `static`).
    pub const fn new_const() -> Self {
        const SLOT: AuditSlot = AuditSlot::new();
        Self {
            ring:            [SLOT; AUDIT_RING_CAPACITY],
            head:            AtomicU64::new(0),
            total:           AtomicU64::new(0),
            violation_count: AtomicUsize::new(0),
            repair_count:    AtomicUsize::new(0),
            critical_count:  AtomicUsize::new(0),
        }
    }

    // ── Enregistrement ────────────────────────────────────────────────────────

    /// Enregistre un événement d'audit dans le ring buffer.
    pub fn record(&self, entry: AuditEntry) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize & AUDIT_MASK;
        // SAFETY: idx est dans [0, AUDIT_MASK].
        unsafe { self.ring[idx].write(entry); }
        self.total.fetch_add(1, Ordering::Relaxed);

        if entry.kind.is_integrity_violation() {
            self.violation_count.fetch_add(1, Ordering::Relaxed);
        }
        if entry.kind.is_repair() {
            self.repair_count.fetch_add(1, Ordering::Relaxed);
        }
        if entry.severity == AuditSeverity::Critical {
            self.critical_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Enregistre le début d'une séquence de récupération.
    /// Enregistre l'initialisation du module de recovery.
    pub fn record_init(&self) {
        use crate::fs::exofs::core::types::EpochId;
        self.record_recovery_started(EpochId(0));
    }

    pub fn record_recovery_started(&self, epoch_id: EpochId) {
        self.record(AuditEntry::info(
            AuditEventKind::RecoveryStarted,
            epoch_id.0,
            0,
        ));
    }

    /// Enregistre la fin réussie d'une séquence de récupération.
    pub fn record_recovery_completed(&self, epoch_id: EpochId, repairs: u32) {
        self.record(AuditEntry::error(
            AuditEventKind::RecoveryCompleted,
            AuditSeverity::Info,
            epoch_id.0,
            0,
            repairs,
            0,
        ));
    }

    /// Enregistre un échec de récupération.
    pub fn record_recovery_failed(&self, epoch_id: EpochId, err_code: u32) {
        self.record(AuditEntry::error(
            AuditEventKind::RecoveryFailed,
            AuditSeverity::Critical,
            epoch_id.0,
            0,
            0,
            err_code,
        ));
    }

    /// Enregistre le début d'une phase fsck.
    pub fn record_phase_started(&self, phase: u8) {
        self.record(AuditEntry::info(
            AuditEventKind::FsckPhaseStarted,
            0,
            phase as u64,
        ));
    }

    /// Enregistre la fin d'une phase fsck.
    pub fn record_phase_done(&self, phase: u8, errors: u32) {
        let kind = if errors == 0 {
            AuditEventKind::FsckPhaseClean
        } else {
            AuditEventKind::FsckPhaseErrors
        };
        let sev = if errors == 0 { AuditSeverity::Info } else { AuditSeverity::Warning };
        self.record(AuditEntry::error(kind, sev, 0, phase as u64, errors, 0));
    }

    /// Enregistre une réparation de métadonnées.
    pub fn record_metadata_repaired(&self, target_offset: u64, detail: [u8; 16]) {
        self.record(AuditEntry::with_detail(
            AuditEventKind::MetadataRepaired,
            AuditSeverity::Warning,
            0,
            target_offset,
            detail,
        ));
    }

    /// Enregistre la récupération d'un blob orphan.
    pub fn record_orphan_recovered(&self, blob_id_prefix: [u8; 16]) {
        self.record(AuditEntry::with_detail(
            AuditEventKind::OrphanRecovered,
            AuditSeverity::Warning,
            0,
            0,
            blob_id_prefix,
        ));
    }

    /// Enregistre un magic invalide.
    pub fn record_invalid_magic(&self, offset: u64, got: u64) {
        let mut detail = [0u8; 16];
        detail[0..8].copy_from_slice(&got.to_le_bytes());
        self.record(AuditEntry::with_detail(
            AuditEventKind::InvalidMagicDetected,
            AuditSeverity::Error,
            0,
            offset,
            detail,
        ));
    }

    /// Enregistre un checksum invalide.
    pub fn record_checksum_invalid(&self, offset: u64, expected: u32, got: u32) {
        let mut detail = [0u8; 16];
        detail[0..4].copy_from_slice(&expected.to_le_bytes());
        detail[4..8].copy_from_slice(&got.to_le_bytes());
        self.record(AuditEntry::error(
            AuditEventKind::ChecksumInvalid,
            AuditSeverity::Error,
            0,
            offset,
            0,
            got,
        ));
    }

    /// Enregistre la sélection d'un slot.
    pub fn record_slot_selected(&self, slot_id: u8, epoch_id: u64) {
        self.record(AuditEntry::info(
            AuditEventKind::SlotSelected,
            epoch_id,
            slot_id as u64,
        ));
    }

    /// Enregistre le replay d'une epoch.
    pub fn record_epoch_replayed(&self, epoch_id: EpochId, n_replayed: u32) {
        self.record(AuditEntry::error(
            AuditEventKind::EpochReplayed,
            AuditSeverity::Info,
            epoch_id.0,
            0,
            n_replayed,
            0,
        ));
    }

    /// Enregistre la sauvegarde d'un checkpoint.
    pub fn record_checkpoint_saved(&self, checkpoint_id: u64, phase: u8) {
        let mut e = AuditEntry::info(AuditEventKind::CheckpointSaved, 0, checkpoint_id);
        e.err_code = phase as u32;
        self.record(e);
    }

    /// Enregistre une structure corrompue.
    pub fn record_structure_corrupted(&self, offset: u64, err_code: u32) {
        self.record(AuditEntry::error(
            AuditEventKind::StructureCorrupted,
            AuditSeverity::Error,
            0,
            offset,
            0,
            err_code,
        ));
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Nombre total d'événements enregistrés.
    #[inline]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées actuellement dans le ring.
    #[inline]
    pub fn len(&self) -> usize {
        (self.total.load(Ordering::Relaxed) as usize).min(AUDIT_RING_CAPACITY)
    }

    /// `true` si aucun événement n'a été enregistré.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total.load(Ordering::Relaxed) == 0
    }

    /// Nombre de violations d'intégrité enregistrées.
    #[inline]
    pub fn violation_count(&self) -> usize {
        self.violation_count.load(Ordering::Relaxed)
    }

    /// Nombre de réparations enregistrées.
    #[inline]
    pub fn repair_count(&self) -> usize {
        self.repair_count.load(Ordering::Relaxed)
    }

    /// Nombre d'événements critiques enregistrés.
    #[inline]
    pub fn critical_count(&self) -> usize {
        self.critical_count.load(Ordering::Relaxed)
    }

    /// Lit les `n` événements les plus récents.
    ///
    /// # Règle OOM-02
    /// `try_reserve` avant chaque `push`.
    pub fn read_recent(&self, n: usize) -> ExofsResult<Vec<AuditEntry>> {
        let n = n.min(AUDIT_RING_CAPACITY);
        let total = self.total.load(Ordering::Relaxed) as usize;
        let n = n.min(total);

        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;

        let head = self.head.load(Ordering::Relaxed) as usize;
        for i in 0..n {
            let slot_idx = head.wrapping_sub(n).wrapping_add(i) & AUDIT_MASK;
            // SAFETY: slot_idx dans [0, AUDIT_MASK].
            let entry = unsafe { self.ring[slot_idx].read() };
            out.push(entry);
        }

        Ok(out)
    }

    /// Lit tous les événements du ring.
    pub fn read_all(&self) -> ExofsResult<Vec<AuditEntry>> {
        self.read_recent(AUDIT_RING_CAPACITY)
    }

    /// Lit les événements d'un type donné parmi les `n` derniers.
    ///
    /// # Règle OOM-02
    /// `try_reserve(1)` avant chaque `push`.
    pub fn read_by_kind(
        &self,
        kind: AuditEventKind,
        n: usize,
    ) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.read_recent(n)?;
        let mut out = Vec::new();
        for entry in &all {
            if entry.kind as u8 == kind as u8 {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*entry);
            }
        }
        Ok(out)
    }

    /// Lit tous les événements de sévérité ≥ `min_sev` parmi les `n` derniers.
    ///
    /// # Règle OOM-02
    /// `try_reserve(1)` avant chaque `push`.
    pub fn read_by_severity(
        &self,
        min_sev: AuditSeverity,
        n: usize,
    ) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.read_recent(n)?;
        let mut out = Vec::new();
        for entry in &all {
            if entry.severity >= min_sev {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*entry);
            }
        }
        Ok(out)
    }

    /// Lit tous les événements de violations d'intégrité parmi les `n` derniers.
    pub fn read_violations(&self, n: usize) -> ExofsResult<Vec<AuditEntry>> {
        let all = self.read_recent(n)?;
        let mut out = Vec::new();
        for entry in &all {
            if entry.kind.is_integrity_violation() {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*entry);
            }
        }
        Ok(out)
    }

    // ── Remise à zéro ─────────────────────────────────────────────────────────

    /// Remet le trail à zéro.
    ///
    /// # Safety
    /// Appelable uniquement pendant une phase d'init exclusive.
    pub fn clear(&self) {
        self.head.store(0, Ordering::SeqCst);
        self.total.store(0, Ordering::SeqCst);
        self.violation_count.store(0, Ordering::SeqCst);
        self.repair_count.store(0, Ordering::SeqCst);
        self.critical_count.store(0, Ordering::SeqCst);
    }

    // ── Vérification de santé ─────────────────────────────────────────────────

    /// `true` si aucune violation d'intégrité et aucun événement critique.
    #[inline]
    pub fn is_clean(&self) -> bool {
        self.violation_count() == 0 && self.critical_count() == 0
    }

    /// Retourne un snapshot diagnostique.
    pub fn diagnostic(&self) -> AuditDiagnostic {
        AuditDiagnostic {
            capacity:        AUDIT_RING_CAPACITY,
            total_events:    self.total(),
            current_len:     self.len(),
            violations:      self.violation_count(),
            repairs:         self.repair_count(),
            critical:        self.critical_count(),
            is_clean:        self.is_clean(),
        }
    }
}

// ── Snapshot diagnostique ─────────────────────────────────────────────────────

/// Vue diagnostique du trail d'audit.
#[derive(Clone, Copy, Debug)]
pub struct AuditDiagnostic {
    /// Capacité maximale du ring.
    pub capacity:     usize,
    /// Total d'événements enregistrés.
    pub total_events: u64,
    /// Nombre courant dans le ring.
    pub current_len:  usize,
    /// Violations d'intégrité.
    pub violations:   usize,
    /// Réparations effectuées.
    pub repairs:      usize,
    /// Événements critiques.
    pub critical:     usize,
    /// `true` si aucune violation ni événement critique.
    pub is_clean:     bool,
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_read() {
        let audit = RecoveryAudit::new_const();
        assert!(audit.is_empty());
        audit.record_recovery_started(EpochId(1));
        assert_eq!(audit.total(), 1);
        let entries = audit.read_recent(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind as u8, AuditEventKind::RecoveryStarted as u8);
    }

    #[test]
    fn test_violation_counting() {
        let audit = RecoveryAudit::new_const();
        audit.record_invalid_magic(0x1000, 0xDEAD);
        audit.record_checksum_invalid(0x2000, 0xAA, 0xBB);
        assert_eq!(audit.violation_count(), 2);
    }

    #[test]
    fn test_ring_wrap() {
        let audit = RecoveryAudit::new_const();
        for i in 0..(AUDIT_RING_CAPACITY + 5) {
            audit.record(AuditEntry::info(AuditEventKind::Custom, 0, i as u64));
        }
        assert_eq!(audit.len(), AUDIT_RING_CAPACITY);
    }

    #[test]
    fn test_is_clean() {
        let audit = RecoveryAudit::new_const();
        audit.record_recovery_started(EpochId(1));
        assert!(audit.is_clean());
        audit.record_invalid_magic(0, 0);
        assert!(!audit.is_clean());
    }

    #[test]
    fn test_clear() {
        let audit = RecoveryAudit::new_const();
        audit.record_recovery_started(EpochId(42));
        audit.clear();
        assert!(audit.is_empty());
        assert_eq!(audit.violation_count(), 0);
    }
}
