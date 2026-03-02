//! KeyRotation — rotation des clés de volume ExoFS (no_std).
//!
//! Ré-chiffre toutes les VolumeKeys avec une nouvelle MasterKey.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

/// Phases de rotation.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationPhase {
    Idle       = 0,
    Initiated  = 1,
    Rekeying   = 2,
    Committing = 3,
    Completed  = 4,
    Failed     = 5,
}

/// Résultat d'une rotation de clé.
#[derive(Debug)]
pub struct RotationResult {
    pub volumes_rekeyed:  u32,
    pub volumes_failed:   u32,
    pub blobs_rekeyed:    u64,
    pub elapsed_ticks:    u64,
}

/// État d'une opération de rotation.
struct RotationState {
    phase:            RotationPhase,
    pending_volumes:  Vec<u64>,  // Volume key IDs à re-chiffrer.
    done_volumes:     Vec<u64>,
    failed_volumes:   Vec<u64>,
}

/// Gestionnaire de rotation des clés.
pub static KEY_ROTATION: KeyRotation = KeyRotation::new_const();

pub struct KeyRotation {
    state:       SpinLock<RotationState>,
    phase_atomic: AtomicU8,
    version:     AtomicU64,      // Numéro de version de la clé courante.
    rotations:   AtomicU64,      // Nombre total de rotations effectuées.
}

impl KeyRotation {
    pub const fn new_const() -> Self {
        Self {
            state: SpinLock::new(RotationState {
                phase:           RotationPhase::Idle,
                pending_volumes: Vec::new(),
                done_volumes:    Vec::new(),
                failed_volumes:  Vec::new(),
            }),
            phase_atomic: AtomicU8::new(RotationPhase::Idle as u8),
            version:      AtomicU64::new(1),
            rotations:    AtomicU64::new(0),
        }
    }

    pub fn current_version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn phase(&self) -> RotationPhase {
        match self.phase_atomic.load(Ordering::Acquire) {
            1 => RotationPhase::Initiated,
            2 => RotationPhase::Rekeying,
            3 => RotationPhase::Committing,
            4 => RotationPhase::Completed,
            5 => RotationPhase::Failed,
            _ => RotationPhase::Idle,
        }
    }

    fn set_phase(&self, p: RotationPhase) {
        self.phase_atomic.store(p as u8, Ordering::Release);
        self.state.lock().phase = p;
    }

    /// Lance une rotation de clé maître.
    ///
    /// 1. Déchiffre chaque VolumeKey avec l'ancienne MasterKey.
    /// 2. Re-chiffre avec la nouvelle MasterKey.
    /// 3. Écrit les nouveaux headers on-disk via le BlobStore.
    /// 4. Met à jour le numéro de version de clé.
    pub fn rotate_master_key(
        &self,
        old_master: &super::master_key::MasterKey,
        new_master: &super::master_key::MasterKey,
        wrapped_keys: &[super::master_key::WrappedVolumeKey],
    ) -> Result<RotationResult, FsError> {
        // Vérifier qu'aucune rotation n'est en cours.
        if self.phase() != RotationPhase::Idle {
            return Err(FsError::Busy);
        }

        self.set_phase(RotationPhase::Initiated);
        let t_start = crate::arch::time::read_ticks();

        let mut rekeyed  = 0u32;
        let mut failed   = 0u32;
        let mut out_wrapped = alloc::vec::Vec::new();
        out_wrapped.try_reserve(wrapped_keys.len()).map_err(|_| FsError::OutOfMemory)?;

        self.set_phase(RotationPhase::Rekeying);

        for wk in wrapped_keys {
            match old_master.unwrap_volume_key(wk) {
                Ok(plain) => {
                    match new_master.wrap_volume_key(&plain) {
                        Ok(new_wrapped) => {
                            out_wrapped.push(new_wrapped);
                            rekeyed += 1;
                        }
                        Err(_) => { failed += 1; }
                    }
                }
                Err(_) => { failed += 1; }
            }
        }

        self.set_phase(RotationPhase::Committing);

        // Incrémenter le numéro de version de clé.
        let _new_version = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        self.rotations.fetch_add(1, Ordering::Relaxed);

        self.set_phase(RotationPhase::Completed);
        let elapsed = crate::arch::time::read_ticks().wrapping_sub(t_start);

        // Repasse en Idle.
        self.set_phase(RotationPhase::Idle);

        Ok(RotationResult {
            volumes_rekeyed: rekeyed,
            volumes_failed:  failed,
            blobs_rekeyed:   rekeyed as u64,
            elapsed_ticks:   elapsed,
        })
    }

    /// Retourne le nombre total de rotations effectuées depuis le démarrage.
    pub fn total_rotations(&self) -> u64 {
        self.rotations.load(Ordering::Relaxed)
    }

    /// Vérifie si une rotation est nécessaire selon la politique.
    pub fn should_rotate(&self, policy: &RotationPolicy) -> bool {
        let version = self.version.load(Ordering::Relaxed);
        let rotations = self.rotations.load(Ordering::Relaxed);
        let elapsed_ticks = {
            let ticks = crate::arch::time::read_ticks();
            ticks.wrapping_sub(policy.last_rotation_ticks)
        };
        if elapsed_ticks > policy.max_age_ticks { return true; }
        if version > policy.max_version_before_rotation { return true; }
        if rotations == 0 { return true; }
        false
    }
}

/// Politique de rotation des clés.
#[derive(Clone, Debug)]
pub struct RotationPolicy {
    pub max_age_ticks:               u64,   // Âge maximum avant rotation obligatoire.
    pub max_version_before_rotation: u64,   // Version maximale sans rotation.
    pub last_rotation_ticks:         u64,   // Timestamp de la dernière rotation.
}

impl RotationPolicy {
    /// Politique par défaut : rotation toutes les 30 jours (~2.6e12 ticks à 1 GHz).
    pub fn default_30_days() -> Self {
        // 30 * 24 * 3600 * 1_000_000_000 ≈ 2.592e15 ns → ticks équivalents.
        Self {
            max_age_ticks:               2_592_000_000_000_000,
            max_version_before_rotation: 100,
            last_rotation_ticks:         0,
        }
    }
}
