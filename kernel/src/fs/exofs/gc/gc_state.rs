//! État et phase courante du Garbage Collector ExoFS.
//!
//! La machine à états est : Idle → Scanning → Marking → Sweeping → Idle.
//! RÈGLE 13 : GC n'acquiert jamais EPOCH_COMMIT_LOCK.

use core::sync::atomic::{AtomicU8, Ordering};
use crate::fs::exofs::core::EpochId;

/// Phases du cycle GC.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum GcPhase {
    /// Aucune passe en cours.
    Idle     = 0,
    /// Scan de l'epoch root chain pour construire le graphe live.
    Scanning = 1,
    /// Propagation tricolore gris→noir.
    Marking  = 2,
    /// Libération des blobs blancs et mise à jour des structures.
    Sweeping = 3,
    /// Finalisation : compaction du heap, mise à jour des métriques.
    Finalizing = 4,
}

impl GcPhase {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Scanning,
            2 => Self::Marking,
            3 => Self::Sweeping,
            4 => Self::Finalizing,
            _ => Self::Idle,
        }
    }
}

/// État global du GC, partagé entre le thread GC et les observateurs.
pub struct GcState {
    /// Phase courante (AtomicU8 pour lecture sans lock).
    phase: AtomicU8,
    /// EpochId de la passe en cours (0 = aucune).
    current_epoch: core::sync::atomic::AtomicU64,
    /// Nombre de blobs récupérés lors de la dernière passe.
    last_reclaimed: core::sync::atomic::AtomicU64,
    /// Nombre de bytes libérés lors de la dernière passe.
    last_freed_bytes: core::sync::atomic::AtomicU64,
    /// Timestamp (ticks) du début de la passe courante.
    pass_start_tick: core::sync::atomic::AtomicU64,
}

impl GcState {
    pub const fn new() -> Self {
        Self {
            phase: AtomicU8::new(0),
            current_epoch: core::sync::atomic::AtomicU64::new(0),
            last_reclaimed: core::sync::atomic::AtomicU64::new(0),
            last_freed_bytes: core::sync::atomic::AtomicU64::new(0),
            pass_start_tick: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Retourne la phase courante.
    #[inline]
    pub fn phase(&self) -> GcPhase {
        GcPhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    /// Passe à une nouvelle phase.
    pub fn set_phase(&self, p: GcPhase) {
        self.phase.store(p as u8, Ordering::Release);
    }

    /// Retourne `true` si une passe GC est active.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.phase() != GcPhase::Idle
    }

    /// Démarre une nouvelle passe pour l'epoch donnée.
    pub fn begin_pass(&self, epoch: EpochId, start_tick: u64) {
        self.current_epoch.store(epoch.0, Ordering::Release);
        self.pass_start_tick.store(start_tick, Ordering::Release);
        self.set_phase(GcPhase::Scanning);
    }

    /// Termine la passe en enregistrant les métriques de résultat.
    pub fn end_pass(&self, reclaimed: u64, freed_bytes: u64) {
        self.last_reclaimed.store(reclaimed, Ordering::Release);
        self.last_freed_bytes.store(freed_bytes, Ordering::Release);
        self.set_phase(GcPhase::Idle);
        self.current_epoch.store(0, Ordering::Release);
    }

    /// Epoch en cours de traitement (0 si inactif).
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch.load(Ordering::Acquire)
    }

    /// Blobs récupérés lors de la dernière passe terminée.
    pub fn last_reclaimed(&self) -> u64 {
        self.last_reclaimed.load(Ordering::Acquire)
    }

    /// Bytes libérés lors de la dernière passe terminée.
    pub fn last_freed_bytes(&self) -> u64 {
        self.last_freed_bytes.load(Ordering::Acquire)
    }
}

/// Singleton GC state, accessible depuis tout le module fs.
pub static GC_STATE: GcState = GcState::new();
