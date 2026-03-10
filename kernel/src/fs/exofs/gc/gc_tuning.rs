// kernel/src/fs/exofs/gc/gc_tuning.rs
//
// ==============================================================================
// Auto-tuning du Garbage Collector ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ajuste automatiquement les seuils GC selon la charge systeme :
//   - Pression memoire / disque : abaisse les seuils de declenchement
//   - Charge CPU faible         : augmente la frequence GC
//   - GC lag important          : force une passe immediate
//
// Conformite :
//   GC-05 : tuning ne bloque jamais dans le chemin critique d'ecriture
//   ARITH-02 : saturation sur tous les calculs
// ==============================================================================


use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};

use crate::fs::exofs::gc::gc_metrics::GcMetricsSnapshot;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes par defaut
// ==============================================================================

/// Seuil d'espace libre en dessous duquel le GC se declenche (%).
pub const GC_FREE_SPACE_THRESHOLD_DEFAULT: u32 = 20;

/// Intervalle de timer GC en ticks logiques.
pub const GC_TIMER_INTERVAL_DEFAULT_TICKS: u64 = 60_000;

/// Coefficient d'aggressivite minimum (0..=100).
pub const GC_AGGRESSIVENESS_MIN: u32 = 10;

/// Coefficient d'aggressivite maximum (0..=100).
pub const GC_AGGRESSIVENESS_MAX: u32 = 100;

/// Aggressivite par defaut.
pub const GC_AGGRESSIVENESS_DEFAULT: u32 = 50;

/// Lag maximum acceptable en epochs avant forcer une passe.
pub const GC_MAX_LAG_EPOCHS: u64 = 8;

/// Seuil de ratio de collecte en dessous duquel on reduit l'aggressivite.
/// Si < 5% des blobs scannes sont collectes, le GC est inutilement agressif.
pub const GC_MIN_USEFUL_COLLECT_RATIO: u32 = 5;

/// Nombre maximum de passes consecutives en mode urgent.
pub const GC_URGENT_MAX_PASSES: u32 = 16;

// ==============================================================================
// GcTriggerReason — raison du declenchement
// ==============================================================================

/// Raison du declenchement d'une passe GC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GcTriggerReason {
    /// Espace libre trop bas.
    LowFreeSpace  = 0,
    /// Timer periodique.
    PeriodicTimer = 1,
    /// GC lag trop eleve.
    HighLag       = 2,
    /// Demande explicite (syscall SYS_EXOFS_GC_TRIGGER = 514).
    UserRequest   = 3,
    /// Demarrage initial.
    Bootstrap     = 4,
    /// Pression memoire kernel.
    MemoryPressure = 5,
}

impl GcTriggerReason {
    pub fn name(self) -> &'static str {
        match self {
            GcTriggerReason::LowFreeSpace   => "LowFreeSpace",
            GcTriggerReason::PeriodicTimer  => "PeriodicTimer",
            GcTriggerReason::HighLag        => "HighLag",
            GcTriggerReason::UserRequest    => "UserRequest",
            GcTriggerReason::Bootstrap      => "Bootstrap",
            GcTriggerReason::MemoryPressure => "MemoryPressure",
        }
    }

    /// Priorite : higher = plus urgent.
    pub fn priority(self) -> u32 {
        match self {
            GcTriggerReason::MemoryPressure => 5,
            GcTriggerReason::LowFreeSpace   => 4,
            GcTriggerReason::HighLag        => 3,
            GcTriggerReason::UserRequest    => 2,
            GcTriggerReason::PeriodicTimer  => 1,
            GcTriggerReason::Bootstrap      => 0,
        }
    }
}

impl fmt::Display for GcTriggerReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ==============================================================================
// GcTuningParams — parametres ajustables
// ==============================================================================

/// Parametres de tuning du GC, ajustables dynamiquement.
#[derive(Debug, Clone)]
pub struct GcTuningParams {
    /// Seuil espace libre (%) pour declenchement.
    pub free_space_threshold: u32,
    /// Intervalle timer en ticks logiques.
    pub timer_interval_ticks: u64,
    /// Aggressivite (0..=100) : influence la profondeur de scan.
    pub aggressiveness:       u32,
    /// Lag max acceptable en epochs.
    pub max_lag_epochs:       u64,
    /// Max passes consecutives en mode urgent.
    pub urgent_max_passes:    u32,
    /// true : mode urgence actif (espace critique).
    pub urgent_mode:          bool,
}

impl Default for GcTuningParams {
    fn default() -> Self {
        Self {
            free_space_threshold: GC_FREE_SPACE_THRESHOLD_DEFAULT,
            timer_interval_ticks: GC_TIMER_INTERVAL_DEFAULT_TICKS,
            aggressiveness:       GC_AGGRESSIVENESS_DEFAULT,
            max_lag_epochs:       GC_MAX_LAG_EPOCHS,
            urgent_max_passes:    GC_URGENT_MAX_PASSES,
            urgent_mode:          false,
        }
    }
}

impl GcTuningParams {
    /// Valide que les parametres sont dans des plages acceptables.
    pub fn validate(&self) -> bool {
        self.free_space_threshold <= 100
            && self.aggressiveness >= GC_AGGRESSIVENESS_MIN
            && self.aggressiveness <= GC_AGGRESSIVENESS_MAX
            && self.timer_interval_ticks > 0
            && self.max_lag_epochs > 0
    }
}

// ==============================================================================
// GcSystemState — etat systeme observe par le tuner
// ==============================================================================

/// Etat systeme utilise par l'auto-tuner pour ajuster les parametres.
#[derive(Debug, Clone, Default)]
pub struct GcSystemState {
    /// Pourcentage d'espace disque libre (0..=100).
    pub free_space_pct:    u32,
    /// Lag GC en epochs (current_epoch - oldest_uncollected).
    pub gc_lag_epochs:     u64,
    /// Charge CPU en pourcent (0..=100).
    pub cpu_load_pct:      u32,
    /// Pression memoire : true si l'allocateur signale une pression elevee.
    pub memory_pressure:   bool,
    /// Ticks logiques depuis la derniere passe GC.
    pub ticks_since_pass:  u64,
}

// ==============================================================================
// GcAutoTuner — moteur d'auto-tuning
// ==============================================================================

/// Auto-tuner du GC — ajuste les parametres selon l'etat systeme.
pub struct GcAutoTuner {
    inner: SpinLock<GcAutoTunerInner>,
    /// Compteur de declenchements urgents consecutifs.
    urgent_passes: AtomicU32,
    /// Tick du dernier ajustement.
    last_tune_tick: AtomicU64,
    /// Force un declenchement immediat.
    force_trigger: AtomicBool,
}

struct GcAutoTunerInner {
    params:         GcTuningParams,
    last_reason:    Option<GcTriggerReason>,
    tune_count:     u64,
}

impl GcAutoTuner {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(GcAutoTunerInner {
                params:      GcTuningParams {
                    free_space_threshold: GC_FREE_SPACE_THRESHOLD_DEFAULT,
                    timer_interval_ticks: GC_TIMER_INTERVAL_DEFAULT_TICKS,
                    aggressiveness:       GC_AGGRESSIVENESS_DEFAULT,
                    max_lag_epochs:       GC_MAX_LAG_EPOCHS,
                    urgent_max_passes:    GC_URGENT_MAX_PASSES,
                    urgent_mode:          false,
                },
                last_reason: None,
                tune_count:  0,
            }),
            urgent_passes:  AtomicU32::new(0),
            last_tune_tick: AtomicU64::new(0),
            force_trigger:  AtomicBool::new(false),
        }
    }

    // ── Parametres courants ──────────────────────────────────────────────────

    /// Retourne une copie des parametres courants.
    pub fn params(&self) -> GcTuningParams {
        self.inner.lock().params.clone()
    }

    // ── Ajustement automatique ───────────────────────────────────────────────

    /// Met a jour les parametres selon l'etat systeme observe.
    ///
    /// GC-05 : cette methode ne bloque jamais dans le chemin critique d'ecriture.
    pub fn tune(&self, state: &GcSystemState, metrics: &GcMetricsSnapshot, tick: u64) {
        let mut g = self.inner.lock();
        g.tune_count = g.tune_count.saturating_add(1);
        self.last_tune_tick.store(tick, Ordering::Relaxed);

        // ── Mode urgence ─────────────────────────────────────────────────────
        let urgent = state.free_space_pct < 10 || state.memory_pressure;
        g.params.urgent_mode = urgent;

        if urgent {
            // Mode urgence : aggressivite maximale, seuil abaisse.
            g.params.aggressiveness = GC_AGGRESSIVENESS_MAX;
            g.params.free_space_threshold = 40;
            g.params.timer_interval_ticks =
                GC_TIMER_INTERVAL_DEFAULT_TICKS / 4;
        } else if state.free_space_pct < g.params.free_space_threshold {
            // Espace faible mais pas critique.
            g.params.aggressiveness = g.params.aggressiveness
                .saturating_add(10)
                .min(GC_AGGRESSIVENESS_MAX);
            g.params.timer_interval_ticks =
                GC_TIMER_INTERVAL_DEFAULT_TICKS / 2;
        } else {
            // Etat normal : potentiellement reduire l'aggressivite.
            let ratio = metrics.collect_ratio_x100();
            if ratio < GC_MIN_USEFUL_COLLECT_RATIO as u64 {
                // GC peu utile : espacer les passes.
                g.params.aggressiveness = g.params.aggressiveness
                    .saturating_sub(5)
                    .max(GC_AGGRESSIVENESS_MIN);
                g.params.timer_interval_ticks = g.params.timer_interval_ticks
                    .saturating_add(GC_TIMER_INTERVAL_DEFAULT_TICKS / 4)
                    .min(GC_TIMER_INTERVAL_DEFAULT_TICKS * 4);
            } else {
                // Restaurer les valeurs par defaut progressivement.
                g.params.aggressiveness = GC_AGGRESSIVENESS_DEFAULT;
                g.params.timer_interval_ticks = GC_TIMER_INTERVAL_DEFAULT_TICKS;
                g.params.free_space_threshold = GC_FREE_SPACE_THRESHOLD_DEFAULT;
            }
        }
    }

    // ── Decisions de declenchement ───────────────────────────────────────────

    /// Determine si une passe GC doit etre declenchee.
    ///
    /// GC-05 : non bloquant, lecture atomique des parametres.
    pub fn should_trigger(
        &self,
        state:       &GcSystemState,
        current_tick: u64,
    ) -> Option<GcTriggerReason> {
        // Declenchement force explicite.
        if self.force_trigger.swap(false, Ordering::AcqRel) {
            return Some(GcTriggerReason::UserRequest);
        }

        let g = self.inner.lock();
        let params = &g.params;

        // Pression memoire.
        if state.memory_pressure {
            return Some(GcTriggerReason::MemoryPressure);
        }

        // Espace libre trop bas.
        if state.free_space_pct < params.free_space_threshold {
            return Some(GcTriggerReason::LowFreeSpace);
        }

        // GC lag trop eleve.
        if state.gc_lag_epochs > params.max_lag_epochs {
            return Some(GcTriggerReason::HighLag);
        }

        // Timer periodique.
        let last = self.last_tune_tick.load(Ordering::Relaxed);
        let elapsed = current_tick.saturating_sub(last);
        if elapsed >= params.timer_interval_ticks {
            return Some(GcTriggerReason::PeriodicTimer);
        }

        None
    }

    /// Force le declenchement d'une passe GC au prochain cycle.
    pub fn force_trigger(&self) {
        self.force_trigger.store(true, Ordering::Release);
    }

    /// Enregistre qu'une passe urgente s'est terminee.
    pub fn record_urgent_pass(&self) {
        self.urgent_passes.fetch_add(1, Ordering::Relaxed);
    }

    /// Nombre de passes urgentes consecutives.
    pub fn urgent_pass_count(&self) -> u32 {
        self.urgent_passes.load(Ordering::Relaxed)
    }

    /// Reinitialise le compteur urgent.
    pub fn reset_urgent_count(&self) {
        self.urgent_passes.store(0, Ordering::Relaxed);
    }

    /// Retourne la derniere raison de declenchement.
    pub fn last_reason(&self) -> Option<GcTriggerReason> {
        self.inner.lock().last_reason
    }

    /// Enregistre la raison courante.
    pub fn record_trigger_reason(&self, reason: GcTriggerReason) {
        self.inner.lock().last_reason = Some(reason);
    }

    /// Nombre total d'ajustements effectues.
    pub fn tune_count(&self) -> u64 {
        self.inner.lock().tune_count
    }

    /// Valide les paramètres actuels.
    pub fn validate_params(&self) -> Result<(), ()> {
        if self.inner.lock().params.validate() { Ok(()) } else { Err(()) }
    }

    /// Intervalle du timer en ticks.
    pub fn timer_interval_ticks(&self) -> u64 {
        self.inner.lock().params.timer_interval_ticks
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Auto-tuner GC global.
pub static GC_TUNER: GcAutoTuner = GcAutoTuner::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> GcSystemState {
        GcSystemState {
            free_space_pct:   50,
            gc_lag_epochs:    1,
            cpu_load_pct:     20,
            memory_pressure:  false,
            ticks_since_pass: 1000,
        }
    }

    #[test]
    fn test_default_params_valid() {
        let p = GcTuningParams::default();
        assert!(p.validate());
    }

    #[test]
    fn test_no_trigger_normal() {
        let tuner = GcAutoTuner::new();
        let state = default_state();
        // tick = 0, timer interval = 60000, elapsed = 0
        let r = tuner.should_trigger(&state, 0);
        assert!(r.is_none());
    }

    #[test]
    fn test_trigger_low_free_space() {
        let tuner = GcAutoTuner::new();
        let mut state = default_state();
        state.free_space_pct = 10; // < 20 (seuil default)
        let r = tuner.should_trigger(&state, 0);
        assert_eq!(r, Some(GcTriggerReason::LowFreeSpace));
    }

    #[test]
    fn test_trigger_memory_pressure() {
        let tuner = GcAutoTuner::new();
        let mut state = default_state();
        state.memory_pressure = true;
        let r = tuner.should_trigger(&state, 0);
        assert_eq!(r, Some(GcTriggerReason::MemoryPressure));
    }

    #[test]
    fn test_trigger_high_lag() {
        let tuner = GcAutoTuner::new();
        let mut state = default_state();
        state.gc_lag_epochs = 10; // > 8 (seuil default)
        let r = tuner.should_trigger(&state, 0);
        assert_eq!(r, Some(GcTriggerReason::HighLag));
    }

    #[test]
    fn test_force_trigger() {
        let tuner = GcAutoTuner::new();
        let state = default_state();
        tuner.force_trigger();
        let r = tuner.should_trigger(&state, 0);
        assert_eq!(r, Some(GcTriggerReason::UserRequest));
        // Deuxieme appel : plus de force_trigger
        let r2 = tuner.should_trigger(&state, 0);
        assert_ne!(r2, Some(GcTriggerReason::UserRequest));
    }

    #[test]
    fn test_tune_urgent_mode() {
        let tuner = GcAutoTuner::new();
        let mut state = default_state();
        state.free_space_pct = 5; // < 10 -> urgent
        let snap = crate::fs::exofs::gc::gc_metrics::GcMetricsSnapshot::default();
        tuner.tune(&state, &snap, 1000);
        let params = tuner.params();
        assert!(params.urgent_mode);
        assert_eq!(params.aggressiveness, GC_AGGRESSIVENESS_MAX);
    }

    #[test]
    fn test_trigger_reason_priority() {
        assert!(GcTriggerReason::MemoryPressure.priority() > GcTriggerReason::LowFreeSpace.priority());
        assert!(GcTriggerReason::LowFreeSpace.priority() > GcTriggerReason::PeriodicTimer.priority());
    }
}
