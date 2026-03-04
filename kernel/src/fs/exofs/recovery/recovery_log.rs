//! recovery_log.rs — Journal circulaire des événements de récupération ExoFS (no_std).
//!
//! Implémente un anneau lock-free de `RECOVERY_LOG_CAPACITY` entrées à accès
//! atomique. Aucune allocation dynamique : toutes les entrées sont stockées dans
//! un tableau statique de taille fixe.
//!
//! # Règles spec appliquées
//! - **OOM-02** : aucune allocation heap ; ring buffer statique.
//! - **ARITH-02** : `checked_add` / `wrapping_add` pour les index.
//! - **ONDISK-03** : pas d'`AtomicU64` dans les structs `repr(C)`.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Capacité du buffer circulaire (puissance de 2 pour masque rapide).
pub const RECOVERY_LOG_CAPACITY: usize = 2048;
const CAPACITY_MASK: usize = RECOVERY_LOG_CAPACITY - 1;

// Vérification statique que la capacité est bien une puissance de 2.
const _: () = assert!(RECOVERY_LOG_CAPACITY.is_power_of_two(), "RECOVERY_LOG_CAPACITY doit être une puissance de 2");

// ── Singleton global ──────────────────────────────────────────────────────────

/// Journal global de récupération, initialisé statiquement.
pub static RECOVERY_LOG: RecoveryLog = RecoveryLog::new_const();

// ── Catégories d'événements ───────────────────────────────────────────────────

/// Catégorie principale d'un événement de récupération.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryLogCategory {
    /// Démarrage de la séquence de boot recovery.
    BootStart      = 0x01,
    /// Sélection du meilleur slot A/B/C.
    SlotSelected   = 0x02,
    /// Début du replay d'epoch.
    ReplayStart    = 0x03,
    /// Fin du replay d'epoch.
    ReplayDone     = 0x04,
    /// Séquence de boot recovery terminée.
    BootDone       = 0x05,
    /// Lancement d'un fsck.
    FsckStarted    = 0x06,
    /// Fin d'un fsck (succès ou partiel).
    FsckDone       = 0x07,
    /// Une action de réparation a été appliquée.
    RepairApplied  = 0x08,
    /// Début d'une phase fsck numérotée.
    PhaseStart     = 0x09,
    /// Fin d'une phase fsck numérotée.
    PhaseDone      = 0x0A,
    /// Checkpoint enregistré.
    CheckpointSaved = 0x0B,
    /// Erreur détectée (non fatale).
    ErrorDetected  = 0x0C,
    /// Avertissement (état dégradé mais récupérable).
    Warning        = 0x0D,
    /// Entrée d'audit externe intégrée.
    AuditIntegration = 0x0E,
    /// Catégorie personnalisée / extension future.
    Custom         = 0xFF,
}

impl RecoveryLogCategory {
    /// Convertit un octet raw en catégorie connue (fallback = `Custom`).
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::BootStart,
            0x02 => Self::SlotSelected,
            0x03 => Self::ReplayStart,
            0x04 => Self::ReplayDone,
            0x05 => Self::BootDone,
            0x06 => Self::FsckStarted,
            0x07 => Self::FsckDone,
            0x08 => Self::RepairApplied,
            0x09 => Self::PhaseStart,
            0x0A => Self::PhaseDone,
            0x0B => Self::CheckpointSaved,
            0x0C => Self::ErrorDetected,
            0x0D => Self::Warning,
            0x0E => Self::AuditIntegration,
            _    => Self::Custom,
        }
    }

    /// Retourne `true` si la catégorie indique une anomalie.
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self, Self::ErrorDetected)
    }

    /// Retourne `true` si la catégorie indique un avertissement ou une erreur.
    #[inline]
    pub fn is_notable(&self) -> bool {
        matches!(self, Self::ErrorDetected | Self::Warning)
    }
}

// ── Entrée de journal ─────────────────────────────────────────────────────────

/// Une entrée immuable dans le journal circulaire.
///
/// Taille fixe : 32 octets (pas de `repr(C)` on-disk — usage runtime uniquement).
#[derive(Clone, Copy, Debug)]
pub struct RecoveryLogEntry {
    /// Horodatage TSC (CPU ticks) de l'événement.
    pub tick:     u64,
    /// Catégorie de l'événement.
    pub category: RecoveryLogCategory,
    /// Sous-code numérique (interprétation dépend de `category`).
    pub code:     u8,
    /// Rembourrage explicite pour alignement.
    pub _pad:     [u8; 6],
    /// Donnée u64 associée (slot_id, epoch_id, count, etc.).
    pub data:     u64,
    /// Données optionnelles complémentaires (ex. blob_id partiel).
    pub extra:    [u8; 8],
}

impl RecoveryLogEntry {
    /// Entrée nulle (valeur d'initialisation dans le ring buffer).
    pub const fn zeroed() -> Self {
        Self {
            tick:     0,
            category: RecoveryLogCategory::Custom,
            code:     0,
            _pad:     [0; 6],
            data:     0,
            extra:    [0; 8],
        }
    }

    /// Construit une entrée simple sans données extra.
    pub fn new(tick: u64, category: RecoveryLogCategory, data: u64) -> Self {
        Self {
            tick,
            category,
            code:  0,
            _pad:  [0; 6],
            data,
            extra: [0; 8],
        }
    }

    /// Construit une entrée avec sous-code et extra.
    pub fn with_extra(
        tick:     u64,
        category: RecoveryLogCategory,
        code:     u8,
        data:     u64,
        extra:    [u8; 8],
    ) -> Self {
        Self { tick, category, code, _pad: [0; 6], data, extra }
    }
}

// ── Slot de ring buffer (UnsafeCell pour mutation statique) ──────────────────

/// Cellule unitaire du ring buffer.
struct LogSlot(core::cell::UnsafeCell<RecoveryLogEntry>);

impl LogSlot {
    const fn new() -> Self {
        Self(core::cell::UnsafeCell::new(RecoveryLogEntry::zeroed()))
    }

    /// Écriture atomique (le séquencement est assuré par `head` fetch_add).
    ///
    /// # Safety
    /// L'appelant doit avoir réservé le slot via `head.fetch_add`.
    unsafe fn write(&self, entry: RecoveryLogEntry) {
        self.0.get().write(entry);
    }

    /// Lecture diagnostique (pas de garantie de cohérence totale en SMP).
    unsafe fn read(&self) -> RecoveryLogEntry {
        self.0.get().read()
    }
}

// SAFETY : accès contrôlé par l'index atomique `head` (séquence monotone).
unsafe impl Sync for LogSlot {}

// ── Structure principale ──────────────────────────────────────────────────────

/// Journal circulaire lock-free des événements de récupération.
///
/// - Capacité fixe : `RECOVERY_LOG_CAPACITY` entrées.
/// - Écriture lock-free via `head.fetch_add`.
/// - Lecture diagnostique sans verrouillage.
pub struct RecoveryLog {
    /// Tableau statique de slots.
    ring:       [LogSlot; RECOVERY_LOG_CAPACITY],
    /// Index d'écriture qui avance monotoniquement.
    head:       AtomicU64,
    /// Compteur total d'entrées écrites (saturations incluses).
    total:      AtomicU64,
    /// Compteur d'entrées de catégorie `ErrorDetected`.
    error_count: AtomicUsize,
    /// Compteur d'entrées notables (erreurs + avertissements).
    notable_count: AtomicUsize,
}

impl RecoveryLog {
    /// Construit un journal vide (utilisable dans un `static`).
    pub const fn new_const() -> Self {
        // Initialise le tableau de slots à zéro.
        const SLOT: LogSlot = LogSlot::new();
        Self {
            ring:          [SLOT; RECOVERY_LOG_CAPACITY],
            head:          AtomicU64::new(0),
            total:         AtomicU64::new(0),
            error_count:   AtomicUsize::new(0),
            notable_count: AtomicUsize::new(0),
        }
    }

    // ── Écriture ──────────────────────────────────────────────────────────────

    /// Écrit une entrée dans le journal circulaire.
    ///
    /// Thread-safe, lock-free. L'entrée la plus ancienne est écrasée si le
    /// buffer est plein (comportement anneau).
    pub fn push(&self, entry: RecoveryLogEntry) {
        // Réserver un slot via l'index monotone.
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize & CAPACITY_MASK;

        // SAFETY : idx est dans [0, CAPACITY_MASK] par masquage.
        unsafe { self.ring[idx].write(entry); }

        self.total.fetch_add(1, Ordering::Relaxed);

        if entry.category.is_error() {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }
        if entry.category.is_notable() {
            self.notable_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Raccourci : enregistre un événement simple avec tick courant.
    pub fn log(&self, category: RecoveryLogCategory, data: u64) {
        let tick = crate::arch::time::read_ticks();
        self.push(RecoveryLogEntry::new(tick, category, data));
    }

    /// Raccourci : enregistre boot start.
    pub fn log_boot_start(&self) {
        self.log(RecoveryLogCategory::BootStart, 0);
    }

    /// Raccourci : enregistre boot done.
    pub fn log_boot_done(&self) {
        self.log(RecoveryLogCategory::BootDone, 0);
    }

    /// Raccourci : enregistre la sélection d'un slot.
    pub fn log_slot_selected(&self, slot_id: u8) {
        self.log(RecoveryLogCategory::SlotSelected, slot_id as u64);
    }

    /// Raccourci : enregistre le début d'un replay.
    pub fn log_replay_start(&self, epoch_id: u64) {
        self.log(RecoveryLogCategory::ReplayStart, epoch_id);
    }

    /// Raccourci : enregistre la fin d'un replay.
    pub fn log_replay_done(&self, n_replayed: u32) {
        self.log(RecoveryLogCategory::ReplayDone, n_replayed as u64);
    }

    /// Raccourci : enregistre le lancement d'un fsck.
    pub fn log_fsck_started(&self) {
        self.log(RecoveryLogCategory::FsckStarted, 0);
    }

    /// Raccourci : enregistre la fin d'un fsck.
    pub fn log_fsck_done(&self, total_errors: u32) {
        self.log(RecoveryLogCategory::FsckDone, total_errors as u64);
    }

    /// Raccourci : enregistre le début d'une phase fsck.
    pub fn log_phase_start(&self, phase: u8) {
        self.log(RecoveryLogCategory::PhaseStart, phase as u64);
    }

    /// Raccourci : enregistre la fin d'une phase fsck.
    pub fn log_phase_done(&self, phase: u8, errors: u32) {
        let tick = crate::arch::time::read_ticks();
        let entry = RecoveryLogEntry::with_extra(
            tick,
            RecoveryLogCategory::PhaseDone,
            phase,
            errors as u64,
            [0; 8],
        );
        self.push(entry);
    }

    /// Raccourci : enregistre une réparation appliquée.
    pub fn log_repair_applied(&self, repair_code: u32, target_id: u64) {
        let tick = crate::arch::time::read_ticks();
        let entry = RecoveryLogEntry::with_extra(
            tick,
            RecoveryLogCategory::RepairApplied,
            0,
            target_id,
            {
                let mut e = [0u8; 8];
                e[0..4].copy_from_slice(&repair_code.to_le_bytes());
                e
            },
        );
        self.push(entry);
    }

    /// Raccourci : enregistre une erreur détectée.
    pub fn log_error(&self, error_code: u8, context: u64) {
        let tick = crate::arch::time::read_ticks();
        let entry = RecoveryLogEntry::with_extra(
            tick,
            RecoveryLogCategory::ErrorDetected,
            error_code,
            context,
            [0; 8],
        );
        self.push(entry);
    }

    /// Raccourci : enregistre un avertissement.
    pub fn log_warning(&self, code: u8, context: u64) {
        let tick = crate::arch::time::read_ticks();
        let entry = RecoveryLogEntry::with_extra(
            tick,
            RecoveryLogCategory::Warning,
            code,
            context,
            [0; 8],
        );
        self.push(entry);
    }

    /// Raccourci : enregistre la sauvegarde d'un checkpoint.
    pub fn log_checkpoint_saved(&self, checkpoint_id: u64) {
        self.log(RecoveryLogCategory::CheckpointSaved, checkpoint_id);
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Retourne le nombre total d'entrées écrites (peut dépasser la capacité).
    #[inline]
    pub fn total_written(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Retourne le nombre d'entrées actuellement dans le ring (≤ capacité).
    #[inline]
    pub fn len(&self) -> usize {
        let t = self.total.load(Ordering::Relaxed) as usize;
        t.min(RECOVERY_LOG_CAPACITY)
    }

    /// Retourne `true` si aucune entrée n'a été écrite.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total.load(Ordering::Relaxed) == 0
    }

    /// Nombre d'entrées `ErrorDetected` enregistrées.
    #[inline]
    pub fn error_count(&self) -> usize {
        self.error_count.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées notables (erreurs + avertissements).
    #[inline]
    pub fn notable_count(&self) -> usize {
        self.notable_count.load(Ordering::Relaxed)
    }

    /// Lit les `n` entrées les plus récentes.
    ///
    /// # Règle OOM-02
    /// `try_reserve` avant chaque push dans le `Vec` résultat.
    ///
    /// # Retourne
    /// `Vec` d'entrées du plus ancien au plus récent parmi les `n` dernières.
    /// Retourne `ExofsError::NoMemory` si l'allocation échoue.
    pub fn read_recent(&self, n: usize) -> ExofsResult<Vec<RecoveryLogEntry>> {
        let n = n.min(RECOVERY_LOG_CAPACITY);
        let total = self.total.load(Ordering::Relaxed) as usize;
        let n = n.min(total);

        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;

        let head = self.head.load(Ordering::Relaxed) as usize;

        for i in 0..n {
            // Index du slot : on remonte depuis head - n + i.
            let slot_idx = head
                .wrapping_sub(n)
                .wrapping_add(i)
                & CAPACITY_MASK;
            // SAFETY : slot_idx est dans [0, CAPACITY_MASK].
            let entry = unsafe { self.ring[slot_idx].read() };
            out.push(entry);
        }

        Ok(out)
    }

    /// Lit toutes les entrées présentes dans le ring (contiguïté pas garantie).
    ///
    /// Retourne `ExofsError::NoMemory` si l'allocation échoue.
    pub fn read_all(&self) -> ExofsResult<Vec<RecoveryLogEntry>> {
        self.read_recent(RECOVERY_LOG_CAPACITY)
    }

    /// Lit les entrées de la catégorie spécifiée parmi les `n` dernières.
    ///
    /// Retourne `ExofsError::NoMemory` si l'allocation échoue.
    pub fn read_by_category(
        &self,
        category: RecoveryLogCategory,
        n: usize,
    ) -> ExofsResult<Vec<RecoveryLogEntry>> {
        let all = self.read_recent(n)?;
        let mut filtered = Vec::new();
        for entry in &all {
            if entry.category as u8 == category as u8 {
                filtered.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                filtered.push(*entry);
            }
        }
        Ok(filtered)
    }

    /// Lit les `n` dernières entrées notables (erreurs + avertissements).
    ///
    /// Retourne `ExofsError::NoMemory` si l'allocation échoue.
    pub fn read_notable(&self, n: usize) -> ExofsResult<Vec<RecoveryLogEntry>> {
        let all = self.read_recent(n.min(RECOVERY_LOG_CAPACITY))?;
        let mut notable = Vec::new();
        for entry in &all {
            if entry.category.is_notable() {
                notable.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                notable.push(*entry);
            }
        }
        Ok(notable)
    }

    // ── Remise à zéro ─────────────────────────────────────────────────────────

    /// Remet le journal à zéro (efface les compteurs ; les données du ring
    /// restent en mémoire mais ne seront plus accessibles via `read_recent`).
    ///
    /// # Safety
    /// Doit être appelé uniquement pendant une phase d'init exclusive.
    pub fn clear(&self) {
        self.head.store(0, Ordering::SeqCst);
        self.total.store(0, Ordering::SeqCst);
        self.error_count.store(0, Ordering::SeqCst);
        self.notable_count.store(0, Ordering::SeqCst);
    }

    // ── Diagnostic ────────────────────────────────────────────────────────────

    /// Retourne un snapshot diagnostique du journal.
    pub fn diagnostic(&self) -> RecoveryLogDiagnostic {
        RecoveryLogDiagnostic {
            capacity:      RECOVERY_LOG_CAPACITY,
            total_written: self.total_written(),
            current_len:   self.len(),
            error_count:   self.error_count(),
            notable_count: self.notable_count(),
        }
    }
}

// ── Snapshot diagnostique ─────────────────────────────────────────────────────

/// Vue diagnostique du journal de récupération.
#[derive(Clone, Copy, Debug)]
pub struct RecoveryLogDiagnostic {
    /// Capacité maximale du ring buffer.
    pub capacity:      usize,
    /// Nombre total d'entrées écrites (peut dépasser `capacity`).
    pub total_written: u64,
    /// Nombre d'entrées actuellement accessibles dans le ring.
    pub current_len:   usize,
    /// Nombre d'entrées de catégorie `ErrorDetected`.
    pub error_count:   usize,
    /// Nombre d'entrées notables (erreurs + warnings).
    pub notable_count: usize,
}

// ── Compat : wrapper legacy log_event ─────────────────────────────────────────

/// Événements legacy utilisés par boot_recovery (compatibilité).
#[derive(Clone, Copy, Debug)]
pub enum RecoveryEvent {
    BootStart,
    SlotSelected(crate::fs::exofs::recovery::slot_recovery::SlotId),
    ReplayStart,
    ReplayDone,
    BootDone,
    FsckStarted,
    FsckDone,
    RepairApplied(u32),
    RecoveryModuleLoaded,
    RecoveryModuleUnloaded,
    RepairStarted,
}

impl RecoveryLog {
    /// Compatibilité legacy : traduit `RecoveryEvent` en entrée structurée.
    pub fn log_event(&self, event: RecoveryEvent) {
        match event {
            RecoveryEvent::BootStart           => self.log_boot_start(),
            RecoveryEvent::SlotSelected(s)     => self.log_slot_selected(s.0),
            RecoveryEvent::ReplayStart         => self.log(RecoveryLogCategory::ReplayStart, 0),
            RecoveryEvent::ReplayDone          => self.log(RecoveryLogCategory::ReplayDone, 0),
            RecoveryEvent::BootDone            => self.log_boot_done(),
            RecoveryEvent::FsckStarted         => self.log_fsck_started(),
            RecoveryEvent::FsckDone            => self.log_fsck_done(0),
            RecoveryEvent::RepairApplied(n)    => self.log_repair_applied(n, 0),
            RecoveryEvent::RecoveryModuleLoaded   => self.log(RecoveryLogCategory::BootStart, 0),
            RecoveryEvent::RecoveryModuleUnloaded => self.log(RecoveryLogCategory::BootDone, 0),
            RecoveryEvent::RepairStarted          => self.log(RecoveryLogCategory::RepairApplied, 0),
        }
    }
}

// ── Tests unitaires (cfg(test)) ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_push_and_read() {
        let log = RecoveryLog::new_const();
        assert!(log.is_empty());
        log.log(RecoveryLogCategory::BootStart, 42);
        assert_eq!(log.len(), 1);
        assert_eq!(log.total_written(), 1);
        let entries = log.read_recent(10).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].category as u8, RecoveryLogCategory::BootStart as u8);
        assert_eq!(entries[0].data, 42);
    }

    #[test]
    fn test_error_counting() {
        let log = RecoveryLog::new_const();
        log.log_error(0x01, 0xDEAD);
        assert_eq!(log.error_count(), 1);
        log.log_warning(0x02, 0);
        assert_eq!(log.notable_count(), 2);
    }

    #[test]
    fn test_ring_wrap() {
        let log = RecoveryLog::new_const();
        for i in 0..(RECOVERY_LOG_CAPACITY + 10) {
            log.log(RecoveryLogCategory::Custom, i as u64);
        }
        assert_eq!(log.len(), RECOVERY_LOG_CAPACITY);
        assert_eq!(log.total_written(), (RECOVERY_LOG_CAPACITY + 10) as u64);
    }

    #[test]
    fn test_clear() {
        let log = RecoveryLog::new_const();
        log.log(RecoveryLogCategory::BootStart, 0);
        log.clear();
        assert!(log.is_empty());
    }
}
