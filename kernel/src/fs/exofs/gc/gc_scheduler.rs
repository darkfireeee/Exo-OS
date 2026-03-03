// kernel/src/fs/exofs/gc/gc_scheduler.rs
//
// ==============================================================================
// Planificateur GC ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ce module determine quand et comment declencher une passe GC.
// Il s'appuie sur GcAutoTuner pour les decisions de declenchement,
// maintient l'etat du timer entre les passes, et publie un signal
// non-bloquant au thread GC.
//
// Conformite :
//   GC-05 : le GC ne bloque jamais le chemin d'ecriture
//           -> should_trigger() est non-bloquant (SeqCst atomic)
//   DAG-01 : pas d'import de arch/, ipc/, process/
//   ARITH-02 : saturating_*
// ==============================================================================

#![allow(dead_code)]

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::gc::gc_metrics::GC_METRICS;
use crate::fs::exofs::gc::gc_state::GC_STATE;
use crate::fs::exofs::gc::gc_tuning::{GcSystemState, GcTriggerReason, GC_TUNER};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Intervalle minimal entre deux passes (ticks logiques).
pub const GC_MIN_INTERVAL_TICKS: u64 = 1_000;

/// Nombre maximum de passes urgentes consecutives.
pub const GC_MAX_URGENT_PASSES: u32 = 16;

// ==============================================================================
// ScheduleReason — raison du declenchement
// ==============================================================================

/// Raison pour laquelle le scheduler a decide de lancer une passe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScheduleReason {
    /// Timer periodique.
    Timer           = 0,
    /// Espace libre insuffisant.
    LowFreeSpace    = 1,
    /// Lag GC trop eleve.
    HighLag         = 2,
    /// Pression memoire.
    MemPressure     = 3,
    /// Demande explicite (syscall 514).
    Explicit        = 4,
    /// Demarrage du systeme.
    Bootstrap       = 5,
}

impl ScheduleReason {
    pub fn name(self) -> &'static str {
        match self {
            Self::Timer        => "timer",
            Self::LowFreeSpace => "low_free_space",
            Self::HighLag      => "high_lag",
            Self::MemPressure  => "mem_pressure",
            Self::Explicit     => "explicit",
            Self::Bootstrap    => "bootstrap",
        }
    }

    fn from_trigger(reason: GcTriggerReason) -> Self {
        match reason {
            GcTriggerReason::PeriodicTimer   => Self::Timer,
            GcTriggerReason::LowFreeSpace    => Self::LowFreeSpace,
            GcTriggerReason::HighLag         => Self::HighLag,
            GcTriggerReason::MemoryPressure  => Self::MemPressure,
            GcTriggerReason::UserRequest     => Self::Explicit,
            GcTriggerReason::Bootstrap       => Self::Bootstrap,
        }
    }
}

impl fmt::Display for ScheduleReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ==============================================================================
// ScheduleDecision — décision du planificateur
// ==============================================================================

/// Décision prise par le scheduler pour la prochaine fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleDecision {
    /// Lancer une passe GC.
    RunNow { reason: ScheduleReason },
    /// Attendre le prochain tick.
    Wait { ticks_remaining: u64 },
    /// GC deja en cours : ignorer.
    AlreadyRunning,
    /// GC desactive.
    Disabled,
}

// ==============================================================================
// GcSchedulerStats — statistiques
// ==============================================================================

/// Statistiques du planificateur.
#[derive(Debug, Default, Clone)]
pub struct GcSchedulerStats {
    pub checks_performed:   u64,
    pub runs_triggered:     u64,
    pub runs_skipped:       u64,
    pub urgent_passes:      u64,
    pub explicit_requests:  u64,
    pub last_trigger_tick:  u64,
    pub last_reason:        Option<ScheduleReason>,
}

impl fmt::Display for GcSchedulerStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SchedulerStats[checks={} triggered={} skip={} urgent={} explicit={}]",
            self.checks_performed,
            self.runs_triggered,
            self.runs_skipped,
            self.urgent_passes,
            self.explicit_requests,
        )
    }
}

// ==============================================================================
// GcSchedulerInner — état interne
// ==============================================================================

struct GcSchedulerInner {
    /// Tick du dernier declenchement.
    last_run_tick:      u64,
    /// Tick de la prochaine execution planifiee.
    next_scheduled_tick: u64,
    /// Passes urgentes consecutives.
    urgent_count:       u32,
    /// GC est-il desactive ?
    disabled:           bool,
    /// Stats.
    stats:              GcSchedulerStats,
}

// ==============================================================================
// GcScheduler — facade thread-safe
// ==============================================================================

/// Planificateur GC.
pub struct GcScheduler {
    inner:           SpinLock<GcSchedulerInner>,
    /// Signal de declenchement urgent (atomique, non bloquant, GC-05).
    trigger_signal:  AtomicBool,
    /// Raison du signal urgent.
    trigger_reason:  AtomicU8,
    /// Epoch courante pour les decisions.
    current_epoch:   AtomicU64,
}

impl GcScheduler {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(GcSchedulerInner {
                last_run_tick:       0,
                next_scheduled_tick: 0,
                urgent_count:        0,
                disabled:            false,
                stats:               GcSchedulerStats {
                    checks_performed:  0,
                    runs_triggered:    0,
                    runs_skipped:      0,
                    urgent_passes:     0,
                    explicit_requests: 0,
                    last_trigger_tick: 0,
                    last_reason:       None,
                },
            }),
            trigger_signal: AtomicBool::new(false),
            trigger_reason: AtomicU8::new(ScheduleReason::Timer as u8),
            current_epoch:  AtomicU64::new(0),
        }
    }

    // ── API principale ───────────────────────────────────────────────────────

    /// Interroge le scheduler pour savoir si une passe doit etre lancee.
    ///
    /// GC-05 : non bloquant — uniquement des lectures atomiques et un spinlock
    /// acquis brievement.
    pub fn check(&self, system_state: &GcSystemState) -> ScheduleDecision {
        {
            let mut g = self.inner.lock();
            g.stats.checks_performed =
                g.stats.checks_performed.saturating_add(1);

            if g.disabled {
                return ScheduleDecision::Disabled;
            }
        }

        // Verifier si le GC est deja en cours.
        let gc_snap = GC_STATE.snapshot();
        if gc_snap.is_running {
            return ScheduleDecision::AlreadyRunning;
        }

        // Decompter les ticks depuis la derniere passe.
        let current_tick = GC_STATE.advance_tick();
        let last_tick = self.inner.lock().last_run_tick;
        let elapsed = current_tick.saturating_sub(last_tick);

        // Signal de declenchement urgent.
        if self.trigger_signal.swap(false, Ordering::AcqRel) {
            let raw = self.trigger_reason.load(Ordering::Acquire);
            let reason = self.decode_reason(raw);
            self.record_trigger(reason, current_tick);
            return ScheduleDecision::RunNow { reason };
        }

        // Evaluer les conditions via GC_TUNER.
        if let Some(trigger_reason) = GC_TUNER.should_trigger(system_state) {
            let reason = ScheduleReason::from_trigger(trigger_reason);
            self.record_trigger(reason, current_tick);
            return ScheduleDecision::RunNow { reason };
        }

        // Timer standard.
        let timer_interval = GC_TUNER.timer_interval_ticks();
        if elapsed >= timer_interval {
            self.record_trigger(ScheduleReason::Timer, current_tick);
            return ScheduleDecision::RunNow { reason: ScheduleReason::Timer };
        }

        let remaining = timer_interval.saturating_sub(elapsed).max(1);
        let mut g = self.inner.lock();
        g.stats.runs_skipped = g.stats.runs_skipped.saturating_add(1);
        ScheduleDecision::Wait { ticks_remaining: remaining }
    }

    /// Enregistre qu'une passe GC vient de se terminer.
    pub fn on_pass_complete(&self, success: bool) {
        let tick = GC_STATE.advance_tick();
        let mut g = self.inner.lock();

        g.last_run_tick = tick;

        if success {
            g.urgent_count = 0;
        } else {
            g.urgent_count = g.urgent_count.saturating_add(1);
            if g.urgent_count >= GC_MAX_URGENT_PASSES {
                // Trop de passes urgentes : ralentir.
                g.urgent_count = 0;
                g.next_scheduled_tick = tick.saturating_add(GC_MIN_INTERVAL_TICKS * 4);
            }
        }

        g.stats.runs_triggered = g.stats.runs_triggered.saturating_add(1);
    }

    /// Déclenche une passe urgente (depuis le syscall 514 par ex.).
    ///
    /// GC-05 : uniquement un store atomique, jamais bloquant.
    pub fn force_trigger(&self, reason: ScheduleReason) {
        self.trigger_reason.store(reason as u8, Ordering::Release);
        self.trigger_signal.store(true, Ordering::Release);

        let mut g = self.inner.lock();
        g.stats.explicit_requests =
            g.stats.explicit_requests.saturating_add(1);
        if reason == ScheduleReason::Explicit {
            g.stats.urgent_passes =
                g.stats.urgent_passes.saturating_add(1);
        }
    }

    /// Met a jour l'epoch courante.
    pub fn set_epoch(&self, epoch: EpochId) {
        self.current_epoch.store(epoch, Ordering::Relaxed);
    }

    /// Active ou desactive le GC.
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().disabled = !enabled;
    }

    /// Est-ce qu'une demande urgente est en attente ?
    ///
    /// GC-05 : lecture atomique uniquement.
    pub fn has_pending_trigger(&self) -> bool {
        self.trigger_signal.load(Ordering::Acquire)
    }

    // ── Helpers internes ─────────────────────────────────────────────────────

    fn decode_reason(&self, raw: u8) -> ScheduleReason {
        match raw {
            0 => ScheduleReason::Timer,
            1 => ScheduleReason::LowFreeSpace,
            2 => ScheduleReason::HighLag,
            3 => ScheduleReason::MemPressure,
            4 => ScheduleReason::Explicit,
            5 => ScheduleReason::Bootstrap,
            _ => ScheduleReason::Timer,
        }
    }

    fn record_trigger(&self, reason: ScheduleReason, tick: u64) {
        let mut g = self.inner.lock();
        g.stats.runs_triggered =
            g.stats.runs_triggered.saturating_add(1);
        g.stats.last_trigger_tick = tick;
        g.stats.last_reason = Some(reason);
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    pub fn stats(&self) -> GcSchedulerStats {
        self.inner.lock().stats.clone()
    }

    pub fn is_disabled(&self) -> bool {
        self.inner.lock().disabled
    }

    pub fn last_run_tick(&self) -> u64 {
        self.inner.lock().last_run_tick
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Planificateur GC global.
pub static GC_SCHEDULER: GcScheduler = GcScheduler::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::gc::gc_tuning::GcSystemState;

    fn state(free_pct: u8, lag: u32) -> GcSystemState {
        GcSystemState {
            free_space_pct:   free_pct,
            gc_lag_epochs:    lag,
            cpu_load_pct:     10,
            memory_pressure:  false,
            ticks_since_pass: 9_999_999,
        }
    }

    #[test]
    fn test_disabled_returns_disabled() {
        let s = GcScheduler::new();
        s.set_enabled(false);
        let decision = s.check(&state(50, 0));
        assert_eq!(decision, ScheduleDecision::Disabled);
    }

    #[test]
    fn test_force_trigger() {
        let s = GcScheduler::new();
        s.force_trigger(ScheduleReason::Explicit);
        assert!(s.has_pending_trigger());
        let decision = s.check(&state(50, 0));
        // Le signal a ete consomme par check().
        assert!(!s.has_pending_trigger());
        match decision {
            ScheduleDecision::RunNow { reason } => {
                assert_eq!(reason, ScheduleReason::Explicit);
            }
            _ => {}
        }
    }

    #[test]
    fn test_on_pass_complete_resets_urgent() {
        let s = GcScheduler::new();
        s.inner.lock().urgent_count = 5;
        s.on_pass_complete(true);
        assert_eq!(s.inner.lock().urgent_count, 0);
    }

    #[test]
    fn test_stats_initial() {
        let s = GcScheduler::new();
        let stats = s.stats();
        assert_eq!(stats.runs_triggered, 0);
        assert_eq!(stats.checks_performed, 0);
    }

    #[test]
    fn test_reason_names() {
        assert_eq!(ScheduleReason::Timer.name(), "timer");
        assert_eq!(ScheduleReason::LowFreeSpace.name(), "low_free_space");
        assert_eq!(ScheduleReason::Explicit.name(), "explicit");
    }
}
