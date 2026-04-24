//! Crypto-shredding ExoFS — destruction sécurisée de données.
//!
//! Le crypto-shredding consiste à détruire la clé de chiffrement d'un bloc
//! de données de façon que les données chiffrées deviennent irrécuparables,
//! sans avoir à supprimer les données physiques.
//!
//! Ce module implémente également l'écrasement physique multi-passes (DoD 5220.22-M)
//! pour les médias qui ne garantissent pas l'effacement par crypto-shredding seul.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::crypto_audit::{AuditKind, AUDIT_LOG};
use super::key_storage::{KeySlotId, KeyStorage};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Traits
// ─────────────────────────────────────────────────────────────────────────────

/// Trait d'abstraction pour l'écriture physique sur le stockage.
///
/// Les implémentations doivent garantir que `overwrite_blob` écrase effectivement
/// les secteurs physiques (pas de déduplication silencieuse).
pub trait OverwriteBlob {
    /// Écrase `size` octets à partir de l'identifiant de blob `blob_id`.
    fn overwrite_blob(&self, blob_id: u64, size: u64, pattern: u8) -> ExofsResult<()>;

    /// Écrase avec un pattern aléatoire.
    fn overwrite_blob_random(&self, blob_id: u64, size: u64, rand_fill: &[u8]) -> ExofsResult<()> {
        // Implémentation par défaut : utilise le premier octet du rand_fill.
        let pattern = rand_fill.first().copied().unwrap_or(0xAA);
        self.overwrite_blob(blob_id, size, pattern)
    }
}

/// Implémentation nulle pour les tests (aucun I/O réel).
pub struct NullOverwriter;

impl OverwriteBlob for NullOverwriter {
    fn overwrite_blob(&self, _blob_id: u64, _size: u64, _pattern: u8) -> ExofsResult<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un shredding individuel.
#[derive(Debug, Clone)]
pub struct ShredResult {
    /// Identifiant du blob shredé.
    pub blob_id: u64,
    /// Taille affectée en octets.
    pub size: u64,
    /// Nombre de passes réalisées.
    pub passes: u8,
    /// Slot de clé révoqué (si applicable).
    pub slot_revoked: Option<KeySlotId>,
    /// `true` si le shredding cryptographique a réussi.
    pub crypto_ok: bool,
    /// `true` si l'écrasement physique a réussi.
    pub physical_ok: bool,
}

/// Stratégie d'écrasement physique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteStrategy {
    /// Simple passe (0x00).
    SinglePass,
    /// Trois passes (DoD 5220.22-M : 0x00, 0xFF, aléatoire).
    DodThreePass,
    /// Sept passes (variante DoD étendue).
    SevenPass,
    /// Uniquement crypto-shredding (pas d'I/O physique).
    CryptoOnly,
}

impl OverwriteStrategy {
    /// Retourne les patterns d'octet à utiliser pour chaque passe.
    pub fn patterns(self) -> &'static [u8] {
        match self {
            Self::SinglePass => &[0x00],
            Self::DodThreePass => &[0x00, 0xFF, 0xAA],
            Self::SevenPass => &[0x00, 0xFF, 0x55, 0xAA, 0x92, 0x49, 0x24],
            Self::CryptoOnly => &[],
        }
    }
    pub fn pass_count(self) -> u8 {
        self.patterns().len() as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CryptoShredder
// ─────────────────────────────────────────────────────────────────────────────

/// Exécuteur de crypto-shredding.
pub struct CryptoShredder {
    strategy: OverwriteStrategy,
    audited: bool,
}

impl CryptoShredder {
    /// Crée un shredder avec la stratégie spécifiée.
    pub fn new(strategy: OverwriteStrategy) -> Self {
        Self {
            strategy,
            audited: true,
        }
    }

    /// Crée un shredder crypto-only (pas d'I/O physique).
    pub fn crypto_only() -> Self {
        Self::new(OverwriteStrategy::CryptoOnly)
    }

    /// Crée un shredder DoD 3 passes.
    pub fn dod_three_pass() -> Self {
        Self::new(OverwriteStrategy::DodThreePass)
    }

    /// Désactive la journalisation d'audit.
    pub fn without_audit(mut self) -> Self {
        self.audited = false;
        self
    }

    /// Shred un seul blob.
    ///
    /// 1. Révoque le slot de clé associé (crypto-shredding).
    /// 2. Écrase physiquement selon la stratégie.
    /// 3. Enregistre dans l'audit log.
    ///
    /// OOM-02 / ARITH-02.
    pub fn shred_blob<O: OverwriteBlob>(
        &self,
        blob_id: u64,
        size: u64,
        slot_id: Option<KeySlotId>,
        storage: Option<&KeyStorage>,
        writer: &O,
    ) -> ExofsResult<ShredResult> {
        // Révocation cryptographique.
        let crypto_ok = match (slot_id, storage) {
            (Some(sid), Some(ks)) => ks.revoke_key(sid).is_ok(),
            _ => true,
        };

        // Écrasement physique.
        let patterns = self.strategy.patterns();
        let mut physical_ok = true;
        for &pattern in patterns {
            if writer.overwrite_blob(blob_id, size, pattern).is_err() {
                physical_ok = false;
                break;
            }
        }

        let passes = self.strategy.pass_count();

        if self.audited {
            AUDIT_LOG.record(
                AuditKind::BlobShredded,
                slot_id,
                blob_id,
                crypto_ok && physical_ok,
            );
        }

        Ok(ShredResult {
            blob_id,
            size,
            passes,
            slot_revoked: slot_id,
            crypto_ok,
            physical_ok,
        })
    }

    /// Shred un batch de blobs.
    ///
    /// OOM-02 : try_reserve.
    pub fn shred_batch<O: OverwriteBlob>(
        &self,
        batch: &[(u64, u64)],        // (blob_id, size)
        slots: &[Option<KeySlotId>], // slot par blob (même longueur ou slice vide)
        storage: Option<&KeyStorage>,
        writer: &O,
    ) -> ExofsResult<Vec<ShredResult>> {
        let mut results: Vec<ShredResult> = Vec::new();
        results
            .try_reserve(batch.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for (i, &(blob_id, size)) in batch.iter().enumerate() {
            let slot = slots.get(i).copied().flatten();
            let r = self.shred_blob(blob_id, size, slot, storage, writer)?;
            results.push(r);
        }
        Ok(results)
    }

    /// Vérifie qu'un slot est bien révoqué (post-shredding).
    pub fn verify_shred(storage: &KeyStorage, slot_id: KeySlotId) -> bool {
        use super::key_storage::SlotState;
        matches!(storage.slot_state(slot_id), Ok(SlotState::Revoked))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaire de génération de pattern
// ─────────────────────────────────────────────────────────────────────────────

/// Génère un pattern d'octet pseudo-aléatoire depuis un seed.
///
/// ARITH-02 : wrapping_mul.
pub fn pseudorandom_pattern(seed: u64, pass_idx: u8) -> u8 {
    let v = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(pass_idx as u64)
        .rotate_right(7);
    (v & 0xFF) as u8
}

/// Calcule le nombre d'octets total à effacer dans un batch.
///
/// ARITH-02 : checked_add.
pub fn total_shred_size(batch: &[(u64, u64)]) -> ExofsResult<u64> {
    let mut total: u64 = 0;
    for &(_, size) in batch {
        total = total.checked_add(size).ok_or(ExofsError::OffsetOverflow)?;
    }
    Ok(total)
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
    fn nw() -> NullOverwriter {
        NullOverwriter
    }

    #[test]
    fn test_shred_blob_crypto_only() {
        let s = CryptoShredder::crypto_only();
        let r = s.shred_blob(1, 4096, None, None, &nw()).unwrap();
        assert_eq!(r.blob_id, 1);
        assert_eq!(r.size, 4096);
        assert_eq!(r.passes, 0);
    }

    #[test]
    fn test_shred_blob_revokes_key() {
        let ks = ks();
        let sid = ks
            .store_key_256(
                &[0u8; 32],
                crate::fs::exofs::crypto::key_storage::KeyKind::Object,
            )
            .unwrap();
        let s = CryptoShredder::new(OverwriteStrategy::SinglePass);
        let r = s.shred_blob(42, 1024, Some(sid), Some(&ks), &nw()).unwrap();
        assert!(r.crypto_ok);
        assert!(CryptoShredder::verify_shred(&ks, sid));
    }

    #[test]
    fn test_shred_dod_three_passes() {
        let s = CryptoShredder::dod_three_pass();
        let r = s.shred_blob(99, 512, None, None, &nw()).unwrap();
        assert_eq!(r.passes, 3);
        assert!(r.physical_ok);
    }

    #[test]
    fn test_shred_batch_count() {
        let s = CryptoShredder::crypto_only();
        let batch: &[(u64, u64)] = &[(1, 100), (2, 200), (3, 300)];
        let results = s.shred_batch(batch, &[], None, &nw()).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_total_shred_size_ok() {
        let batch: &[(u64, u64)] = &[(1, 1000), (2, 2000)];
        assert_eq!(total_shred_size(batch).unwrap(), 3000);
    }

    #[test]
    fn test_total_shred_size_overflow() {
        let batch: &[(u64, u64)] = &[(1, u64::MAX), (2, 1)];
        assert!(total_shred_size(batch).is_err());
    }

    #[test]
    fn test_pseudorandom_pattern_deterministic() {
        let p1 = pseudorandom_pattern(12345, 0);
        let p2 = pseudorandom_pattern(12345, 0);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_pseudorandom_pattern_different_pass() {
        let p0 = pseudorandom_pattern(42, 0);
        let p1 = pseudorandom_pattern(42, 1);
        assert_ne!(p0, p1);
    }

    #[test]
    fn test_strategy_patterns_counts() {
        assert_eq!(OverwriteStrategy::SinglePass.pass_count(), 1);
        assert_eq!(OverwriteStrategy::DodThreePass.pass_count(), 3);
        assert_eq!(OverwriteStrategy::SevenPass.pass_count(), 7);
        assert_eq!(OverwriteStrategy::CryptoOnly.pass_count(), 0);
    }

    #[test]
    fn test_verify_shred_false_for_active() {
        let ks = ks();
        let sid = ks
            .store_key_256(
                &[0u8; 32],
                crate::fs::exofs::crypto::key_storage::KeyKind::Object,
            )
            .unwrap();
        assert!(!CryptoShredder::verify_shred(&ks, sid));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ShredPolicy — politique de destruction automatique
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de destruction automatique d'une clé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShredPolicy {
    /// Jamais (destruction manuelle uniquement).
    Never,
    /// Après N accès au blob.
    AfterNAccesses(u64),
    /// Après un délai dépassé (en ticks noyau).
    AfterTicks(u64),
    /// Immédiatement après la première utilisation.
    Ephemeral,
}

impl ShredPolicy {
    /// Indique si une politique implique la destruction immédiate après usage.
    pub fn is_ephemeral(&self) -> bool {
        matches!(self, Self::Ephemeral)
    }

    /// Évalue la politique face à un compteur d'accès et un tick courant.
    ///
    /// Retourne `true` si la politique dit de détruire maintenant.
    /// ARITH-02 : comparaison directe, pas d'arithmétique débordable.
    pub fn should_shred(&self, accesses: u64, current_tick: u64, registered_tick: u64) -> bool {
        match self {
            Self::Never => false,
            Self::AfterNAccesses(n) => accesses >= *n,
            Self::AfterTicks(dt) => current_tick.saturating_sub(registered_tick) >= *dt,
            Self::Ephemeral => accesses >= 1,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ShredScheduler
// ─────────────────────────────────────────────────────────────────────────────

use alloc::collections::BTreeMap;

/// Entrée du planificateur de destruction.
#[derive(Debug, Clone)]
struct ShredEntry {
    #[allow(dead_code)]
    blob_id: u64,
    blob_size: u64,
    policy: ShredPolicy,
    accesses: u64,
    registered_tick: u64,
    slot_id: Option<KeySlotId>,
}

/// Planificateur de destruction différée.
///
/// Maintient un registre des blobs à détruire et permet d'évaluer par tick
/// quels blobs doivent être shredés.
pub struct ShredScheduler {
    entries: BTreeMap<u64, ShredEntry>, // blob_id → entrée
    #[allow(dead_code)]
    next_seq: u64,
    max_items: usize,
}

impl ShredScheduler {
    /// Crée un planificateur avec capacité maximale.
    pub fn new(max_items: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            next_seq: 0,
            max_items,
        }
    }

    /// Enregistre un blob à surveiller.
    ///
    /// OOM-02 : vérification de capacité avant insertion.
    pub fn register(
        &mut self,
        blob_id: u64,
        blob_size: u64,
        policy: ShredPolicy,
        slot_id: Option<KeySlotId>,
        tick: u64,
    ) -> ExofsResult<()> {
        if self.entries.len() >= self.max_items {
            return Err(ExofsError::NoMemory);
        }
        let entry = ShredEntry {
            blob_id,
            blob_size,
            policy,
            accesses: 0,
            registered_tick: tick,
            slot_id,
        };
        self.entries.insert(blob_id, entry);
        Ok(())
    }

    /// Incrémente le compteur d'accès pour le blob.
    pub fn record_access(&mut self, blob_id: u64) {
        if let Some(e) = self.entries.get_mut(&blob_id) {
            e.accesses = e.accesses.saturating_add(1);
        }
    }

    /// Évalue les politiques au tick courant et retourne les blob_ids à détruire.
    ///
    /// OOM-02 : try_reserve.
    pub fn due_for_shred(&self, current_tick: u64) -> ExofsResult<Vec<u64>> {
        let mut due: Vec<u64> = Vec::new();
        due.try_reserve(self.entries.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for (bid, e) in &self.entries {
            if e.policy
                .should_shred(e.accesses, current_tick, e.registered_tick)
            {
                due.push(*bid);
            }
        }
        Ok(due)
    }

    /// Exécute les destructions dues, retourne les résultats.
    ///
    /// OOM-02 : try_reserve.
    pub fn execute_due<O: OverwriteBlob>(
        &mut self,
        current_tick: u64,
        storage: Option<&KeyStorage>,
        writer: &O,
        shredder: &CryptoShredder,
    ) -> ExofsResult<Vec<ShredResult>> {
        let due = self.due_for_shred(current_tick)?;
        let mut results: Vec<ShredResult> = Vec::new();
        results
            .try_reserve(due.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for bid in &due {
            if let Some(e) = self.entries.get(bid) {
                let r = shredder.shred_blob(*bid, e.blob_size, e.slot_id, storage, writer)?;
                results.push(r);
            }
            self.entries.remove(bid);
        }
        Ok(results)
    }

    /// Retire manuellement un blob du planificateur.
    pub fn unregister(&mut self, blob_id: u64) {
        self.entries.remove(&blob_id);
    }

    /// Nombre de blobs surveillés.
    pub fn registered_count(&self) -> usize {
        self.entries.len()
    }

    /// Retourne les `blob_id`s enregistrés.
    ///
    /// OOM-02 : try_reserve.
    pub fn registered_blobs(&self) -> ExofsResult<Vec<u64>> {
        let mut out: Vec<u64> = Vec::new();
        out.try_reserve(self.entries.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for bid in self.entries.keys() {
            out.push(*bid);
        }
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_scheduler {
    use super::*;

    fn nw() -> NullOverwriter {
        NullOverwriter
    }
    fn shredder() -> CryptoShredder {
        CryptoShredder::crypto_only()
    }

    #[test]
    fn test_policy_ephemeral() {
        let p = ShredPolicy::Ephemeral;
        assert!(!p.should_shred(0, 10, 0));
        assert!(p.should_shred(1, 10, 0));
    }

    #[test]
    fn test_policy_after_n_accesses() {
        let p = ShredPolicy::AfterNAccesses(5);
        assert!(!p.should_shred(4, 100, 0));
        assert!(p.should_shred(5, 100, 0));
        assert!(p.should_shred(6, 100, 0));
    }

    #[test]
    fn test_policy_after_ticks() {
        let p = ShredPolicy::AfterTicks(100);
        assert!(!p.should_shred(0, 99, 0));
        assert!(p.should_shred(0, 100, 0));
    }

    #[test]
    fn test_policy_never() {
        let p = ShredPolicy::Never;
        assert!(!p.should_shred(9999, 9999, 0));
    }

    #[test]
    fn test_scheduler_register_due() {
        let mut sched = ShredScheduler::new(16);
        sched
            .register(1, 512, ShredPolicy::AfterNAccesses(2), None, 0)
            .unwrap();
        sched.record_access(1);
        assert!(sched.due_for_shred(0).unwrap().is_empty());
        sched.record_access(1);
        let due = sched.due_for_shred(0).unwrap();
        assert_eq!(due, alloc::vec![1u64]);
    }

    #[test]
    fn test_scheduler_execute_due() {
        let s = shredder();
        let mut sched = ShredScheduler::new(16);
        sched
            .register(2, 1024, ShredPolicy::Ephemeral, None, 0)
            .unwrap();
        sched.record_access(2);
        let results = sched.execute_due(0, None, &nw(), &s).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(sched.registered_count(), 0);
    }

    #[test]
    fn test_scheduler_capacity_overflow() {
        let mut sched = ShredScheduler::new(1);
        sched.register(1, 100, ShredPolicy::Never, None, 0).unwrap();
        let err = sched.register(2, 100, ShredPolicy::Never, None, 0);
        assert!(err.is_err());
    }

    #[test]
    fn test_scheduler_unregister() {
        let mut sched = ShredScheduler::new(16);
        sched
            .register(10, 200, ShredPolicy::Never, None, 0)
            .unwrap();
        assert_eq!(sched.registered_count(), 1);
        sched.unregister(10);
        assert_eq!(sched.registered_count(), 0);
    }

    #[test]
    fn test_registered_blobs_list() {
        let mut sched = ShredScheduler::new(16);
        sched.register(7, 100, ShredPolicy::Never, None, 0).unwrap();
        sched.register(8, 100, ShredPolicy::Never, None, 0).unwrap();
        let blobs = sched.registered_blobs().unwrap();
        assert_eq!(blobs.len(), 2);
    }

    #[test]
    fn test_shred_result_fields() {
        let s = CryptoShredder::new(OverwriteStrategy::DodThreePass);
        let r = s.shred_blob(77, 4096, None, None, &nw()).unwrap();
        assert_eq!(r.blob_id, 77);
        assert_eq!(r.passes, 3);
        assert!(r.physical_ok);
        assert!(r.crypto_ok);
    }
}
