//! Rotation de clés ExoFS — politique et exécution de la rotation.
//!
//! Permet de renouveler les clés stockées dans `KeyStorage` selon des critères
//! temporels ou d'usage, avec journalisation des rotations effectuées.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::entropy::ENTROPY_POOL;
use super::key_storage::{KeyKind, KeySlotId, KeyStorage};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Raison du déclenchement de la rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationReason {
    /// Planifiée (politique périodique).
    Scheduled,
    /// Dépassement du nombre d'utilisations.
    UsageLimitReached,
    /// Compromission suspectée.
    SecurityAlert,
    /// Demande manuelle.
    Manual,
    /// Rotation initiale à la création.
    Initial,
}

impl core::fmt::Display for RotationReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Scheduled => write!(f, "Scheduled"),
            Self::UsageLimitReached => write!(f, "UsageLimitReached"),
            Self::SecurityAlert => write!(f, "SecurityAlert"),
            Self::Manual => write!(f, "Manual"),
            Self::Initial => write!(f, "Initial"),
        }
    }
}

/// Entrée de suivi de rotation pour un slot.
#[derive(Debug, Clone)]
pub struct RotationEntry {
    pub slot_id: KeySlotId,
    pub rotation_count: u64,
    pub last_reason: RotationReason,
    pub last_ts: u64,
    pub new_slot_id: Option<KeySlotId>,
}

/// Résultat d'une rotation de slot.
#[derive(Debug, Clone)]
pub struct RotationResult {
    pub old_slot: KeySlotId,
    pub new_slot: KeySlotId,
    pub kind: KeyKind,
    pub reason: RotationReason,
}

/// Politique de rotation d'un slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationSchedule {
    /// Rotation manuelle uniquement.
    OnDemand,
    /// Après N utilisations.
    AfterNUses(u64),
    /// Après N rotations d'autres slots (co-rotation).
    CoRotation,
}

// ─────────────────────────────────────────────────────────────────────────────
// KeyRotation
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de rotation de clés.
pub struct KeyRotation {
    /// Entrées de suivi par slot.
    entries: BTreeMap<KeySlotId, RotationEntry>,
    /// Politiques par slot.
    policies: BTreeMap<KeySlotId, RotationSchedule>,
    /// Historique compact (ring buffer).
    history: Vec<RotationResult>,
    /// Taille maximale de l'historique.
    hist_cap: usize,
}

impl KeyRotation {
    /// Crée un gestionnaire de rotation.
    pub fn new(hist_cap: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            policies: BTreeMap::new(),
            history: Vec::new(),
            hist_cap: hist_cap.max(16),
        }
    }

    /// Enregistre un slot pour la rotation avec une politique donnée.
    pub fn register(&mut self, slot: KeySlotId, schedule: RotationSchedule) -> ExofsResult<()> {
        self.entries.insert(
            slot,
            RotationEntry {
                slot_id: slot,
                rotation_count: 0,
                last_reason: RotationReason::Initial,
                last_ts: ENTROPY_POOL.random_u64(),
                new_slot_id: None,
            },
        );
        self.policies.insert(slot, schedule);
        Ok(())
    }

    /// Désinscrit un slot.
    pub fn unregister(&mut self, slot: KeySlotId) {
        self.entries.remove(&slot);
        self.policies.remove(&slot);
    }

    /// Effectue la rotation d'un seul slot.
    ///
    /// Génère un nouveau matériel de clé, stocke dans un nouveau slot,
    /// révoque l'ancien slot.
    ///
    /// OOM-02.
    pub fn rotate_one(
        &mut self,
        old_slot: KeySlotId,
        kind: KeyKind,
        reason: RotationReason,
        storage: &KeyStorage,
    ) -> ExofsResult<RotationResult> {
        // Génération d'un nouveau matériel.
        let raw_vec = ENTROPY_POOL.random_bytes(32)?;
        let mut new_key = [0u8; 32];
        new_key.copy_from_slice(&raw_vec);

        // Stockage du nouveau slot.
        let new_slot = storage.store_key_256(&new_key, kind)?;

        // Zeroize le nouveau matériel temporaire.
        new_key.iter_mut().for_each(|b| *b = 0);

        // Révocation de l'ancien slot.
        let _ = storage.revoke_key(old_slot);

        // Mise à jour de l'entrée de suivi.
        if let Some(entry) = self.entries.get_mut(&old_slot) {
            entry.rotation_count = entry.rotation_count.saturating_add(1);
            entry.last_reason = reason;
            entry.last_ts = ENTROPY_POOL.random_u64();
            entry.new_slot_id = Some(new_slot);
        }

        // Nouvel enregistrement du slot.
        let schedule = self
            .policies
            .get(&old_slot)
            .copied()
            .unwrap_or(RotationSchedule::OnDemand);
        self.register(new_slot, schedule)?;

        let result = RotationResult {
            old_slot,
            new_slot,
            kind,
            reason,
        };

        // Historique.
        if self.history.len() >= self.hist_cap {
            self.history.remove(0);
        }
        self.history
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.history.push(result.clone());

        Ok(result)
    }

    /// Rotation en lot d'un ensemble de slots.
    ///
    /// OOM-02.
    pub fn rotate_batch(
        &mut self,
        batch: &[(KeySlotId, KeyKind)],
        reason: RotationReason,
        storage: &KeyStorage,
    ) -> ExofsResult<Vec<RotationResult>> {
        let mut results: Vec<RotationResult> = Vec::new();
        results
            .try_reserve(batch.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &(slot, kind) in batch {
            match self.rotate_one(slot, kind, reason, storage) {
                Ok(r) => results.push(r),
                Err(e) => return Err(e),
            }
        }
        Ok(results)
    }

    /// Identifie les slots qui nécessitent une rotation selon leur politique d'usage.
    ///
    /// OOM-02.
    pub fn due_for_rotation(&self, storage: &KeyStorage) -> ExofsResult<Vec<KeySlotId>> {
        let mut due: Vec<KeySlotId> = Vec::new();
        for (&slot, policy) in &self.policies {
            if let RotationSchedule::AfterNUses(n) = *policy {
                if let Ok(count) = storage.access_count(slot) {
                    if count >= n {
                        due.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        due.push(slot);
                    }
                }
            }
        }
        Ok(due)
    }

    /// Retourne l'historique des N dernières rotations.
    ///
    /// OOM-02.
    pub fn recent_history(&self, n: usize) -> ExofsResult<Vec<RotationResult>> {
        let start = self.history.len().saturating_sub(n);
        let mut out: Vec<RotationResult> = Vec::new();
        out.try_reserve(n.min(self.history.len()))
            .map_err(|_| ExofsError::NoMemory)?;
        out.extend_from_slice(&self.history[start..]);
        Ok(out)
    }

    /// Nombre de rotations enregistrées pour un slot.
    pub fn rotation_count(&self, slot: KeySlotId) -> u64 {
        self.entries
            .get(&slot)
            .map(|e| e.rotation_count)
            .unwrap_or(0)
    }

    /// Nombre total de slots enregistrés.
    pub fn registered_count(&self) -> usize {
        self.entries.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::key_storage::KeyStorage;
    use super::*;

    fn ks() -> KeyStorage {
        KeyStorage::new_const()
    }

    #[test]
    fn test_rotate_one_ok() {
        let ks = ks();
        let old = ks.store_key_256(&[1u8; 32], KeyKind::Volume).unwrap();
        let mut r = KeyRotation::new(8);
        r.register(old, RotationSchedule::OnDemand).unwrap();
        let res = r
            .rotate_one(old, KeyKind::Volume, RotationReason::Manual, &ks)
            .unwrap();
        assert_ne!(res.old_slot, res.new_slot);
        assert_eq!(res.kind, KeyKind::Volume);
    }

    #[test]
    fn test_old_slot_revoked_after_rotation() {
        let ks = ks();
        let old = ks.store_key_256(&[2u8; 32], KeyKind::Master).unwrap();
        let mut r = KeyRotation::new(8);
        r.register(old, RotationSchedule::OnDemand).unwrap();
        r.rotate_one(old, KeyKind::Master, RotationReason::Scheduled, &ks)
            .unwrap();
        assert!(ks.load_key_256(old).is_err());
    }

    #[test]
    fn test_rotate_batch_count() {
        let ks = ks();
        let s1 = ks.store_key_256(&[1u8; 32], KeyKind::Object).unwrap();
        let s2 = ks.store_key_256(&[2u8; 32], KeyKind::Object).unwrap();
        let mut r = KeyRotation::new(16);
        r.register(s1, RotationSchedule::OnDemand).unwrap();
        r.register(s2, RotationSchedule::OnDemand).unwrap();
        let results = r
            .rotate_batch(
                &[(s1, KeyKind::Object), (s2, KeyKind::Object)],
                RotationReason::SecurityAlert,
                &ks,
            )
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_rotation_count_increments() {
        let ks = ks();
        let s = ks.store_key_256(&[0u8; 32], KeyKind::Derived).unwrap();
        let mut r = KeyRotation::new(8);
        r.register(s, RotationSchedule::OnDemand).unwrap();
        r.rotate_one(s, KeyKind::Derived, RotationReason::Manual, &ks)
            .unwrap();
        assert_eq!(r.rotation_count(s), 1);
    }

    #[test]
    fn test_recent_history_limited() {
        let ks = ks();
        let mut r = KeyRotation::new(4);
        for i in 0u8..6 {
            let s = ks.store_key_256(&[i; 32], KeyKind::Session).unwrap();
            r.register(s, RotationSchedule::OnDemand).unwrap();
            r.rotate_one(s, KeyKind::Session, RotationReason::Scheduled, &ks)
                .unwrap();
        }
        let hist = r.recent_history(3).unwrap();
        assert_eq!(hist.len(), 3);
    }

    #[test]
    fn test_due_for_rotation_after_uses() {
        let ks = ks();
        let s = ks.store_key_256(&[0u8; 32], KeyKind::Volume).unwrap();
        // Simuler 5 accès.
        for _ in 0..5 {
            ks.load_key_256(s).unwrap();
        }
        let mut r = KeyRotation::new(8);
        r.register(s, RotationSchedule::AfterNUses(5)).unwrap();
        let due = r.due_for_rotation(&ks).unwrap();
        assert!(due.contains(&s));
    }

    #[test]
    fn test_unregister_removes() {
        let ks = ks();
        let s = ks.store_key_256(&[0u8; 32], KeyKind::Master).unwrap();
        let mut r = KeyRotation::new(8);
        r.register(s, RotationSchedule::OnDemand).unwrap();
        assert_eq!(r.registered_count(), 1);
        r.unregister(s);
        assert_eq!(r.registered_count(), 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Politique de co-rotation et alert
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau d'alerte de sécurité déclenchant une co-rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityLevel {
    Normal,
    Elevated,
    Critical,
}

/// Gestionnaire d'alerte déclenchant des rotations forcées.
pub struct SecurityRotationTrigger {
    level: SecurityLevel,
    manager: KeyRotation,
}

impl SecurityRotationTrigger {
    pub fn new(hist_cap: usize) -> Self {
        Self {
            level: SecurityLevel::Normal,
            manager: KeyRotation::new(hist_cap),
        }
    }

    /// Élève le niveau de sécurité.
    pub fn elevate(&mut self, new_level: SecurityLevel) {
        self.level = new_level;
    }

    /// Retourne `true` si une rotation forcée est nécessaire.
    pub fn rotation_required(&self) -> bool {
        self.level >= SecurityLevel::Elevated
    }

    /// Force la rotation de tous les slots enregistrés selon le niveau d'alerte.
    ///
    /// OOM-02.
    pub fn force_rotate_all(
        &mut self,
        storage: &KeyStorage,
        kinds: &[(KeySlotId, KeyKind)],
    ) -> ExofsResult<Vec<RotationResult>> {
        if !self.rotation_required() {
            return Ok(Vec::new());
        }
        let reason = if self.level >= SecurityLevel::Critical {
            RotationReason::SecurityAlert
        } else {
            RotationReason::Scheduled
        };
        self.manager.rotate_batch(kinds, reason, storage)
    }

    /// Accès au manager interne.
    pub fn manager_ref(&self) -> &KeyRotation {
        &self.manager
    }
    pub fn manager_mut(&mut self) -> &mut KeyRotation {
        &mut self.manager
    }
}

/// Planificateur périodique de rotation (basé sur un compteur d'époques).
pub struct EpochRotationScheduler {
    epoch: u64,
    epoch_period: u64,
    last_rotation: u64,
}

impl EpochRotationScheduler {
    pub fn new(period: u64) -> Self {
        Self {
            epoch: 0,
            epoch_period: period.max(1),
            last_rotation: 0,
        }
    }

    /// Avance l'epoch d'une unité.
    ///
    /// ARITH-02 : wrapping_add.
    pub fn tick(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Retourne `true` si la période de rotation est écoulée.
    pub fn is_due(&self) -> bool {
        self.epoch.wrapping_sub(self.last_rotation) >= self.epoch_period
    }

    /// Marque la rotation comme effectuée à l'epoch courante.
    pub fn mark_done(&mut self) {
        self.last_rotation = self.epoch;
    }

    pub fn current_epoch(&self) -> u64 {
        self.epoch
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::super::key_storage::KeyStorage;
    use super::*;

    #[test]
    fn test_security_trigger_normal_no_rotation() {
        let ks = KeyStorage::new_const();
        let mut t = SecurityRotationTrigger::new(8);
        let r = t.force_rotate_all(&ks, &[]).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn test_security_trigger_elevated_rotation() {
        let ks = KeyStorage::new_const();
        let s = ks.store_key_256(&[0u8; 32], KeyKind::Volume).unwrap();
        let mut t = SecurityRotationTrigger::new(8);
        t.manager_mut()
            .register(s, RotationSchedule::OnDemand)
            .unwrap();
        t.elevate(SecurityLevel::Elevated);
        assert!(t.rotation_required());
        let r = t.force_rotate_all(&ks, &[(s, KeyKind::Volume)]).unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn test_epoch_scheduler_due() {
        let mut sched = EpochRotationScheduler::new(10);
        for _ in 0..10 {
            sched.tick();
        }
        assert!(sched.is_due());
        sched.mark_done();
        assert!(!sched.is_due());
    }

    #[test]
    fn test_epoch_tick_wrapping() {
        let mut sched = EpochRotationScheduler::new(1);
        sched.tick();
        assert!(sched.is_due());
    }
}
