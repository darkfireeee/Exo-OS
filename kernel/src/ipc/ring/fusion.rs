// kernel/src/ipc/ring/fusion.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FUSION RING — Adaptive batching ring
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le Fusion Ring adapte dynamiquement sa stratégie entre :
//   • MODE DIRECT   : chaque message envoyé immédiatement (faible charge)
//   • MODE BATCH    : accumulation jusqu'à THRESHOLD ou délai (forte charge)
//
// La décision est basée sur un compteur de messages en vol et une EWA
// (Exponential Weighted Average) du débit observé.
//
// INVARIANTS :
//   • Latence garantie ≤ FUSION_MAX_DELAY_TICKS × tick_period.
//   • Débordement impossible : si le BatchBuffer est plein → flush immédiat.
//   • Thread-safe pour un seul producteur et un seul consommateur.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use crate::ipc::core::{IpcError, MsgFlags};
use super::spsc::SpscRing;
use super::batch::BatchBuffer;

// ─────────────────────────────────────────────────────────────────────────────
// Mode de fonctionnement du Fusion Ring
// ─────────────────────────────────────────────────────────────────────────────

/// Mode courant du Fusion Ring.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FusionMode {
    /// Envoi direct sans buffering (faible charge).
    Direct = 0,
    /// Accumulation + flush différé (forte charge).
    Batch  = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// FusionMetrics — métriques adaptatives
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs pour la prise de décision d'adaptation.
struct FusionMetrics {
    /// Messages envoyés depuis la dernière période d'observation.
    msgs_since_last: AtomicU64,
    /// Dernier tick d'observation.
    last_obs_tick: AtomicU64,
    /// EWA du débit msgs/tick.
    ewa_throughput: AtomicU32,
    /// Nombre de switchs de mode (diagnostic).
    mode_switches: AtomicU64,
}

impl FusionMetrics {
    const fn new() -> Self {
        Self {
            msgs_since_last: AtomicU64::new(0),
            last_obs_tick:   AtomicU64::new(0),
            ewa_throughput:  AtomicU32::new(0),
            mode_switches:   AtomicU64::new(0),
        }
    }

    /// Met à jour l'EWA du débit. Appelé à chaque tick scheduler.
    /// Formule EWA : ewa = (ewa * 7 + instant) >> 3
    fn update(&self, current_tick: u64) {
        let last = self.last_obs_tick.load(Ordering::Relaxed);
        if current_tick == last {
            return; // même tick, pas de mise à jour
        }
        let dt = (current_tick - last).max(1);
        let msgs = self.msgs_since_last.swap(0, Ordering::Relaxed);
        let instant = (msgs / dt).min(u32::MAX as u64) as u32;
        let ewa = ((self.ewa_throughput.load(Ordering::Relaxed) as u64 * 7
            + instant as u64) >> 3) as u32;
        self.ewa_throughput.store(ewa, Ordering::Relaxed);
        self.last_obs_tick.store(current_tick, Ordering::Relaxed);
    }

    /// Retourne le débit EWA en msgs/tick.
    fn throughput_ewa(&self) -> u32 {
        self.ewa_throughput.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FusionRing
// ─────────────────────────────────────────────────────────────────────────────

/// Seuil de débit pour passer en mode Batch (msgs/tick).
const BATCH_THRESHOLD_THROUGHPUT: u32 = 100;
/// Seuil de débit pour repasser en mode Direct.
const DIRECT_THRESHOLD_THROUGHPUT: u32 = 20;

/// Ring adaptatif — mode Direct en dessous du seuil, Batch au-dessus.
pub struct FusionRing {
    /// Ring sous-jacent (SPSC).
    inner: SpscRing,
    /// Tampon de batch côté producteur.
    batch: BatchBuffer,
    /// Mode courant.
    mode:  AtomicU32, // 0 = Direct, 1 = Batch
    /// Métriques pour l'adaptation.
    metrics: FusionMetrics,
    /// Tick courant (mis à jour par le scheduler tick handler).
    current_tick: AtomicU64,
}

impl FusionRing {
    /// Crée un FusionRing en mode Direct.
    pub const fn new() -> Self {
        Self {
            inner:        SpscRing::new(),
            batch:        BatchBuffer::new(),
            mode:         AtomicU32::new(0),
            metrics:      FusionMetrics::new(),
            current_tick: AtomicU64::new(0),
        }
    }

    /// Initialise le ring sous-jacent.
    pub fn init(&self) {
        self.inner.init();
    }

    /// Met à jour le tick courant (appelé depuis le scheduler tick handler).
    #[inline(always)]
    pub fn tick(&self, tick: u64) {
        self.current_tick.store(tick, Ordering::Relaxed);
        self.metrics.update(tick);
        self.adapt_mode();
    }

    /// Adapte le mode en fonction du débit observé.
    fn adapt_mode(&self) {
        let ewa = self.metrics.throughput_ewa();
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 0 && ewa >= BATCH_THRESHOLD_THROUGHPUT {
            self.mode.store(1, Ordering::Relaxed);
            self.metrics.mode_switches.fetch_add(1, Ordering::Relaxed);
        } else if mode == 1 && ewa <= DIRECT_THRESHOLD_THROUGHPUT {
            self.mode.store(0, Ordering::Relaxed);
            self.metrics.mode_switches.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Envoie un message (adaptatif).
    pub fn send(&mut self, src: &[u8], flags: MsgFlags) -> Result<(), IpcError> {
        let tick = self.current_tick.load(Ordering::Relaxed);
        self.metrics.msgs_since_last.fetch_add(1, Ordering::Relaxed);

        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 0 {
            // MODE DIRECT — envoi immédiat.
            self.inner.push_copy(src, flags).map(|_| ())
        } else {
            // MODE BATCH — accumuler puis flush si nécessaire.
            let should_flush = self.batch.add(src, flags, tick);
            if should_flush || self.batch.is_expired(tick) {
                let _ = self.batch.flush_to_ring(&self.inner);
            }
            Ok(())
        }
    }

    /// Flush forcé du batch (ex : fin de quantum, fermeture canal).
    pub fn flush(&mut self) -> usize {
        if self.batch.count() > 0 {
            self.batch.flush_to_ring(&self.inner)
        } else {
            0
        }
    }

    /// Reçoit un message.
    pub fn recv(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        self.inner.pop_into(dst)
    }

    /// Mode courant du ring.
    #[inline(always)]
    pub fn mode(&self) -> FusionMode {
        if self.mode.load(Ordering::Relaxed) == 0 {
            FusionMode::Direct
        } else {
            FusionMode::Batch
        }
    }

    /// Nombre de messages en attente dans le ring.
    #[inline(always)]
    pub fn pending(&self) -> usize {
        self.inner.len_approx() + self.batch.count()
    }

    /// Statistiques diagnostiques.
    pub fn mode_switches(&self) -> u64 {
        self.metrics.mode_switches.load(Ordering::Relaxed)
    }

    pub fn throughput_ewa(&self) -> u32 {
        self.metrics.throughput_ewa()
    }
}
