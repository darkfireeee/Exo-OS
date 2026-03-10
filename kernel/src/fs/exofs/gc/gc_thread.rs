// kernel/src/fs/exofs/gc/gc_thread.rs
//
// ==============================================================================
// Thread de Fond du GC ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ce module implemente la boucle principale du thread GC de fond.
// Le thread GC :
//   1. Verifie periodiquement si une passe est necessaire via GC_SCHEDULER
//   2. Lance une passe complète via BLOB_GC si le scheduler le decide
//   3. Met a jour le scheduler apres chaque passe
//   4. Peut etre arrête proprement via le signal SHUTDOWN
//
// Conformite :
//   GC-05 : GC toujours en background, chemin d'ecriture jamais bloque
//   DEAD-01 : jamais acquerir EPOCH_COMMIT_LOCK
//   DAG-01 : pas d'import de arch/, ipc/, process/
// ==============================================================================


use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::fs::exofs::core::EpochId;
use crate::fs::exofs::gc::blob_gc::BLOB_GC;
use crate::fs::exofs::gc::gc_scheduler::{
    ScheduleDecision, ScheduleReason, GC_SCHEDULER,
};
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::gc_tuning::GcSystemState;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre de ticks logiques entre deux interrogations du scheduler.
pub const GC_THREAD_POLL_INTERVAL: u64 = 100;

/// Nombre maximum de passes sans succes avant une pause forcee.
pub const GC_MAX_CONSECUTIVE_FAILURES: u32 = 8;

/// Periode de "cooldown" apres trop d'echecs (en iterations).
pub const GC_FAILURE_COOLDOWN_ITERS: u64 = 50;

// ==============================================================================
// GcThreadStats — statistiques du thread
// ==============================================================================

/// Statistiques du thread GC.
#[derive(Debug, Default, Clone)]
pub struct GcThreadStats {
    /// Iterations totales de la boucle.
    pub iterations:          u64,
    /// Passes lancees.
    pub passes_launched:     u64,
    /// Passes reussies.
    pub passes_succeeded:    u64,
    /// Passes echouees.
    pub passes_failed:       u64,
    /// Conseils "Wait" recus du scheduler.
    pub wait_decisions:      u64,
    /// Conseils "AlreadyRunning".
    pub already_running:     u64,
    /// Ticks logiques depenses en cooldown.
    pub cooldown_iters:      u64,
    /// Ticks totaux traites.
    pub total_ticks:         u64,
    /// Dernier epoch connu.
    pub last_epoch:          EpochId,
}

impl fmt::Display for GcThreadStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcThreadStats[iter={} launched={} ok={} fail={} waits={} epoch={}]",
            self.iterations,
            self.passes_launched,
            self.passes_succeeded,
            self.passes_failed,
            self.wait_decisions,
            self.last_epoch,
        )
    }
}

// ==============================================================================
// GcThreadControl — signaux de controle
// ==============================================================================

/// Signaux de controle du thread GC.
pub struct GcThreadControl {
    /// Demande d'arret propre.
    pub shutdown:     AtomicBool,
    /// Le thread est actif.
    pub running:      AtomicBool,
    /// Nombre consecutif d'echecs.
    pub fail_count:   AtomicU32,
    /// Epoch courante.
    pub current_epoch: AtomicU64,
}

impl GcThreadControl {
    pub const fn new() -> Self {
        Self {
            shutdown:      AtomicBool::new(false),
            running:       AtomicBool::new(false),
            fail_count:    AtomicU32::new(0),
            current_epoch: AtomicU64::new(0),
        }
    }

    /// Signale un arret propre au thread.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Le thread a-t-il recu un ordre d'arret ?
    pub fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Marque le thread comme actif.
    pub fn set_running(&self, v: bool) {
        self.running.store(v, Ordering::Release);
    }

    /// Est actif ?
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Mise a jour de l'epoch.
    pub fn update_epoch(&self, epoch: EpochId) {
        self.current_epoch.store(epoch.0, Ordering::Relaxed);
        GC_SCHEDULER.set_epoch(epoch);
        BLOB_GC.set_epoch(epoch);
    }

    /// Epoch courante.
    pub fn epoch(&self) -> EpochId {
        EpochId(self.current_epoch.load(Ordering::Relaxed))
    }
}

// ==============================================================================
// GcThread — structure du thread
// ==============================================================================

/// Etat du thread GC de fond.
pub struct GcThread {
    pub control: GcThreadControl,
    // Stats internes (non proteges par spinlock — lecture atomique ok).
    iterations:      AtomicU64,
    passes_launched: AtomicU64,
    passes_ok:       AtomicU64,
    passes_fail:     AtomicU64,
    wait_count:      AtomicU64,
}

impl GcThread {
    pub const fn new() -> Self {
        Self {
            control:         GcThreadControl::new(),
            iterations:      AtomicU64::new(0),
            passes_launched: AtomicU64::new(0),
            passes_ok:       AtomicU64::new(0),
            passes_fail:     AtomicU64::new(0),
            wait_count:      AtomicU64::new(0),
        }
    }

    // ── Boucle principale ────────────────────────────────────────────────────

    /// Boucle principale du thread GC.
    ///
    /// Cette fonction est appelee par le kernel au demarrage du thread GC.
    /// Elle tourne indefiniment jusqu'a reception du signal SHUTDOWN.
    ///
    /// GC-05 : non-bloquante hors des passes GC elles-memes.
    pub fn run(&self) {
        self.control.set_running(true);
        let mut cooldown: u64 = 0;

        // Boucle principale.
        loop {
            // Test d'arrêt.
            if self.control.should_shutdown() {
                break;
            }

            self.iterations.fetch_add(1, Ordering::Relaxed);

            // Cooldown apres trop d'échecs.
            if cooldown > 0 {
                cooldown = cooldown.saturating_sub(1);
                self.spin_yield();
                continue;
            }

            // Etat systeme courant pour le scheduler.
            let system_state = self.build_system_state();

            // Interroger le scheduler.
            let decision = GC_SCHEDULER.check(&system_state);

            match decision {
                ScheduleDecision::RunNow { reason } => {
                    self.run_one_pass(reason);

                    let fail = self.control.fail_count.load(Ordering::Relaxed);
                    if fail >= GC_MAX_CONSECUTIVE_FAILURES {
                        // Trop d'echecs : entrer en cooldown.
                        cooldown = GC_FAILURE_COOLDOWN_ITERS;
                        self.control.fail_count.store(0, Ordering::Relaxed);
                    }
                }

                ScheduleDecision::Wait { .. } => {
                    self.wait_count.fetch_add(1, Ordering::Relaxed);
                    self.spin_yield();
                }

                ScheduleDecision::AlreadyRunning => {
                    self.spin_yield();
                }

                ScheduleDecision::Disabled => {
                    self.spin_yield();
                }
            }
        }

        self.control.set_running(false);
    }

    /// Lance une seule passe GC.
    fn run_one_pass(&self, _reason: ScheduleReason) {
        self.passes_launched.fetch_add(1, Ordering::Relaxed);

        let epoch = self.control.epoch();
        BLOB_GC.set_epoch(epoch);

        // GC pass avec aucune EpochRoot fournie directement.
        // Dans un systeme reel, les EpochRoots seraient lues depuis le
        // stockage persistant via un handle de disque. Ici l'API est
        // correcte structurellement : on passe [None, None, None].
        let result = BLOB_GC.run_pass(&[None, None, None]);

        let success = result.success;
        GC_SCHEDULER.on_pass_complete(success);

        if success {
            self.passes_ok.fetch_add(1, Ordering::Relaxed);
            self.control.fail_count.store(0, Ordering::Relaxed);
        } else {
            self.passes_fail.fetch_add(1, Ordering::Relaxed);
            self.control.fail_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Etat systeme courant pour le tuner.
    fn build_system_state(&self) -> GcSystemState {
        // Les valeurs reelles proviendraient de capteurs systeme.
        // Ici on uses des valeurs conservatrices sures pour le no_std.
        // NB: pas d'import arch/ (DAG-01).
        GcSystemState {
            free_space_pct:   50,
            gc_lag_epochs:    0,
            cpu_load_pct:     0,
            memory_pressure:  false,
            ticks_since_pass: GC_STATE.advance_tick(),
        }
    }

    /// Pause non-bloquante (spin hint).
    ///
    /// Equivalent d'un hint de mise en attente sans appel systeme.
    /// Conforme DAG-01 (pas de process::/arch::).
    #[inline(always)]
    fn spin_yield(&self) {
        // Boucle de spin courte (pause CPU hint).
        for _ in 0..GC_THREAD_POLL_INTERVAL {
            core::hint::spin_loop();
        }
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Statistiques du thread.
    pub fn stats(&self) -> GcThreadStats {
        GcThreadStats {
            iterations:       self.iterations.load(Ordering::Relaxed),
            passes_launched:  self.passes_launched.load(Ordering::Relaxed),
            passes_succeeded: self.passes_ok.load(Ordering::Relaxed),
            passes_failed:    self.passes_fail.load(Ordering::Relaxed),
            wait_decisions:   self.wait_count.load(Ordering::Relaxed),
            already_running:  0,
            cooldown_iters:   0,
            total_ticks:      GC_STATE.snapshot().logical_tick,
            last_epoch:       self.control.epoch(),
        }
    }

    /// Demande un arret propre du thread.
    pub fn shutdown(&self) {
        self.control.request_shutdown();
    }

    /// Met a jour l'epoch courante.
    pub fn set_epoch(&self, epoch: EpochId) {
        self.control.update_epoch(epoch);
    }

    /// Demande une passe urgente.
    ///
    /// GC-05 : non-bloquant — communique via AtomicBool.
    pub fn trigger_urgent(&self) {
        GC_SCHEDULER.force_trigger(ScheduleReason::Explicit);
    }

    /// Active ou desactive le GC.
    pub fn set_enabled(&self, enabled: bool) {
        GC_SCHEDULER.set_enabled(enabled);
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Thread GC global.
pub static GC_THREAD: GcThread = GcThread::new();

/// Point d'entree du thread GC pour le kernel.
///
/// Cette fonction est passée au scheduler kernel pour etre executee
/// dans le thread GC de fond. Elle ne retourne jamais en fonctionnement normal.
pub fn gc_thread_entry() -> ! {
    GC_THREAD.run();
    // Si run() se termine (shutdown), boucler proprement.
    loop {
        core::hint::spin_loop();
    }
}

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_shutdown() {
        let ctrl = GcThreadControl::new();
        assert!(!ctrl.should_shutdown());
        ctrl.request_shutdown();
        assert!(ctrl.should_shutdown());
    }

    #[test]
    fn test_control_running() {
        let ctrl = GcThreadControl::new();
        assert!(!ctrl.is_running());
        ctrl.set_running(true);
        assert!(ctrl.is_running());
    }

    #[test]
    fn test_control_epoch() {
        let ctrl = GcThreadControl::new();
        ctrl.update_epoch(42);
        assert_eq!(ctrl.epoch(), 42);
    }

    #[test]
    fn test_thread_stats_initial() {
        let t = GcThread::new();
        let s = t.stats();
        assert_eq!(s.iterations, 0);
        assert_eq!(s.passes_launched, 0);
    }

    #[test]
    fn test_shutdown_does_not_panic() {
        let t = GcThread::new();
        t.shutdown();
        assert!(t.control.should_shutdown());
    }

    #[test]
    fn test_trigger_urgent_no_block() {
        let t = GcThread::new();
        // Doit etre un store atomique seulement — pas de blocage.
        t.trigger_urgent();
        assert!(GC_SCHEDULER.has_pending_trigger());
        // Nettoyer pour les autres tests.
        GC_SCHEDULER.force_trigger(ScheduleReason::Timer);
    }

    #[test]
    fn test_set_epoch() {
        let t = GcThread::new();
        t.set_epoch(99);
        assert_eq!(t.control.epoch(), 99);
    }
}
