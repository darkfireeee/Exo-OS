// SPDX-License-Identifier: MIT
// ExoFS NUMA — Politique NUMA adaptative et auto-tuning
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::numa_affinity::MAX_NUMA_NODES;
use super::numa_placement::{PlacementStrategy, NUMA_PLACEMENT};
use super::numa_stats::NUMA_STATS;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─── TuningEvent ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuningEvent {
    /// Stratégie modifiée automatiquement.
    StrategyChanged,
    /// Déséquilibre détecté mais sous le seuil.
    ImbalanceDetected,
    /// Intervention manuelle appliquée.
    ManualOverride,
    /// Pas de changement nécessaire.
    NoChange,
    /// Réinitialisation de la politique.
    Reset,
}

impl TuningEvent {
    pub fn name(self) -> &'static str {
        match self {
            Self::StrategyChanged => "strategy-changed",
            Self::ImbalanceDetected => "imbalance-detected",
            Self::ManualOverride => "manual-override",
            Self::NoChange => "no-change",
            Self::Reset => "reset",
        }
    }
}

// ─── TuningReport ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct TuningReport {
    pub event: TuningEvent,
    pub imbalance_ppt: u64,
    pub threshold_ppt: u64,
    pub old_strategy: PlacementStrategy,
    pub new_strategy: PlacementStrategy,
    pub tick: u64,
    pub evaluations: u64,
    pub changes: u64,
}

impl TuningReport {
    pub fn strategy_changed(self) -> bool {
        self.event == TuningEvent::StrategyChanged
    }
    pub fn is_balanced(self) -> bool {
        self.imbalance_ppt < self.threshold_ppt
    }
}

// ─── PressureZone ─────────────────────────────────────────────────────────────

/// Niveau de pression mémoire sur un nœud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressureZone {
    Normal,
    Warning,
    Critical,
    Emergency,
}

impl PressureZone {
    pub fn from_ppt(usage_ppt: u64) -> Self {
        if usage_ppt >= 950 {
            Self::Emergency
        } else if usage_ppt >= 850 {
            Self::Critical
        } else if usage_ppt >= 700 {
            Self::Warning
        } else {
            Self::Normal
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Critical => "critical",
            Self::Emergency => "emergency",
        }
    }
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Normal)
    }
}

// ─── NumaPolicy ──────────────────────────────────────────────────────────────

/// Politique NUMA adaptative.
pub struct NumaPolicy {
    /// Seuil de déséquilibre en ‰ pour déclencher le changement de stratégie.
    migrate_threshold_ppt: AtomicU64,
    /// Auto-tuning activé.
    auto_tune_enabled: AtomicU8,
    /// Intervalle minimal entre deux évaluations (ticks).
    tune_interval_ticks: AtomicU64,
    /// Dernier tick d'évaluation.
    last_tune_tick: AtomicU64,
    /// Nombre total d'évaluations.
    evaluations: AtomicU64,
    /// Nombre de changements de stratégie.
    strategy_changes: AtomicU64,
    /// Capacité mémoire de référence par nœud en octets.
    node_capacity_bytes: [AtomicU64; MAX_NUMA_NODES],
    /// Stratégie forcée manuellement (u8::MAX = aucune).
    forced_strategy: AtomicU8,
}

macro_rules! atomic8 {
    () => {
        [
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
        ]
    };
}

impl NumaPolicy {
    pub const fn new_const() -> Self {
        Self {
            migrate_threshold_ppt: AtomicU64::new(300), // 30 %
            auto_tune_enabled: AtomicU8::new(0),
            tune_interval_ticks: AtomicU64::new(100_000),
            last_tune_tick: AtomicU64::new(0),
            evaluations: AtomicU64::new(0),
            strategy_changes: AtomicU64::new(0),
            node_capacity_bytes: atomic8!(),
            forced_strategy: AtomicU8::new(u8::MAX),
        }
    }

    /// Configure la politique.
    pub fn configure(
        &self,
        threshold_ppt: u64,
        auto: bool,
        interval_ticks: u64,
    ) -> ExofsResult<()> {
        if threshold_ppt > 1000 {
            return Err(ExofsError::InvalidArgument);
        }
        if interval_ticks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        self.migrate_threshold_ppt
            .store(threshold_ppt, Ordering::Relaxed);
        self.auto_tune_enabled
            .store(u8::from(auto), Ordering::Relaxed);
        self.tune_interval_ticks
            .store(interval_ticks, Ordering::Relaxed);
        Ok(())
    }

    /// Définit la capacité mémoire d'un nœud.
    pub fn set_node_capacity(&self, node: usize, capacity_bytes: u64) -> ExofsResult<()> {
        if node >= MAX_NUMA_NODES {
            return Err(ExofsError::InvalidArgument);
        }
        self.node_capacity_bytes[node].store(capacity_bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Force une stratégie (u8::MAX = libère la contrainte).
    pub fn force_strategy(&self, strategy: Option<PlacementStrategy>) {
        let v = strategy.map(|s| s as u8).unwrap_or(u8::MAX);
        self.forced_strategy.store(v, Ordering::Relaxed);
    }

    pub fn threshold_ppt(&self) -> u64 {
        self.migrate_threshold_ppt.load(Ordering::Relaxed)
    }
    pub fn auto_enabled(&self) -> bool {
        self.auto_tune_enabled.load(Ordering::Relaxed) != 0
    }
    pub fn evaluations(&self) -> u64 {
        self.evaluations.load(Ordering::Relaxed)
    }
    pub fn strategy_changes(&self) -> u64 {
        self.strategy_changes.load(Ordering::Relaxed)
    }

    // ── Évaluation ────────────────────────────────────────────────────────────

    /// Évalue la charge des nœuds et ajuste la stratégie si nécessaire.
    pub fn evaluate(&self, current_tick: u64) -> TuningReport {
        let old_strategy = NUMA_PLACEMENT.strategy();

        // Vérifier actif et intervalle
        if !self.auto_enabled() {
            return self._make_report(
                TuningEvent::NoChange,
                0,
                old_strategy,
                old_strategy,
                current_tick,
            );
        }
        let last = self.last_tune_tick.load(Ordering::Relaxed);
        let interval = self.tune_interval_ticks.load(Ordering::Relaxed);
        if current_tick.saturating_sub(last) < interval {
            return self._make_report(
                TuningEvent::NoChange,
                0,
                old_strategy,
                old_strategy,
                current_tick,
            );
        }
        self.last_tune_tick.store(current_tick, Ordering::Relaxed);
        self.evaluations.fetch_add(1, Ordering::Relaxed);

        // Stratégie forcée manuelle
        let forced = self.forced_strategy.load(Ordering::Relaxed);
        if forced != u8::MAX {
            let new_strat = PlacementStrategy::from_u8(forced);
            let n = NUMA_PLACEMENT.n_nodes() as u8;
            let _ = NUMA_PLACEMENT.init(n, new_strat);
            self.strategy_changes.fetch_add(1, Ordering::Relaxed);
            return self._make_report(
                TuningEvent::ManualOverride,
                0,
                old_strategy,
                new_strat,
                current_tick,
            );
        }

        // Calcul du déséquilibre (ARITH-02 : saturating, checked_div, RECUR-01 : while)
        let imbalance = self._compute_imbalance_ppt();
        let threshold = self.migrate_threshold_ppt.load(Ordering::Relaxed);

        let new_strategy = if imbalance > threshold {
            PlacementStrategy::LeastUsed
        } else {
            PlacementStrategy::LocalFirst
        };

        // Appliquer si différent
        let event = if new_strategy != old_strategy {
            let n = NUMA_PLACEMENT.n_nodes() as u8;
            let _ = NUMA_PLACEMENT.init(n, new_strategy);
            self.strategy_changes.fetch_add(1, Ordering::Relaxed);
            TuningEvent::StrategyChanged
        } else if imbalance > 0 {
            TuningEvent::ImbalanceDetected
        } else {
            TuningEvent::NoChange
        };

        self._make_report(event, imbalance, old_strategy, new_strategy, current_tick)
    }

    /// Calcule le déséquilibre en ‰ entre nœuds (RECUR-01 : while, ARITH-02 : saturating).
    fn _compute_imbalance_ppt(&self) -> u64 {
        let mut max_score = 0u64;
        let mut min_score = u64::MAX;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            let capacity = self.node_capacity_bytes[i].load(Ordering::Relaxed);
            if capacity == 0 {
                i = i.wrapping_add(1);
                continue;
            }
            let stats = NUMA_STATS.node_stats(i);
            let ppt = stats.usage_ppt(capacity);
            if ppt > max_score {
                max_score = ppt;
            }
            if ppt < min_score {
                min_score = ppt;
            }
            i = i.wrapping_add(1);
        }
        if max_score == 0 || min_score == u64::MAX {
            return 0;
        }
        // ARITH-02 : saturating_sub + checked_div
        let diff = max_score.saturating_sub(min_score);
        diff.saturating_mul(1000)
            .checked_div(max_score.max(1))
            .unwrap_or(0)
    }

    fn _make_report(
        &self,
        event: TuningEvent,
        imbalance: u64,
        old_strategy: PlacementStrategy,
        new_strategy: PlacementStrategy,
        tick: u64,
    ) -> TuningReport {
        TuningReport {
            event,
            imbalance_ppt: imbalance,
            threshold_ppt: self.migrate_threshold_ppt.load(Ordering::Relaxed),
            old_strategy,
            new_strategy,
            tick,
            evaluations: self.evaluations.load(Ordering::Relaxed),
            changes: self.strategy_changes.load(Ordering::Relaxed),
        }
    }

    // ── Pression par nœud ─────────────────────────────────────────────────────

    /// Niveau de pression courant d'un nœud (ARITH-02).
    pub fn pressure_zone(&self, node: usize) -> PressureZone {
        if node >= MAX_NUMA_NODES {
            return PressureZone::Normal;
        }
        let capacity = self.node_capacity_bytes[node].load(Ordering::Relaxed);
        if capacity == 0 {
            return PressureZone::Normal;
        }
        let ppt = NUMA_STATS.node_stats(node).usage_ppt(capacity);
        PressureZone::from_ppt(ppt)
    }

    /// Nœud en plus haute pression (RECUR-01 : while).
    pub fn highest_pressure_node(&self) -> usize {
        let mut best = 0usize;
        let mut best_ppt = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            let capacity = self.node_capacity_bytes[i].load(Ordering::Relaxed);
            if capacity > 0 {
                let ppt = NUMA_STATS.node_stats(i).usage_ppt(capacity);
                if ppt > best_ppt {
                    best_ppt = ppt;
                    best = i;
                }
            }
            i = i.wrapping_add(1);
        }
        best
    }

    /// Réinitialise le state de la politique.
    pub fn reset(&self) {
        self.last_tune_tick.store(0, Ordering::Relaxed);
        self.evaluations.store(0, Ordering::Relaxed);
        self.strategy_changes.store(0, Ordering::Relaxed);
        self.forced_strategy.store(u8::MAX, Ordering::Relaxed);
    }
}

/// Singleton global de la politique NUMA.
pub static NUMA_POLICY: NumaPolicy = NumaPolicy::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_policy() -> NumaPolicy {
        NumaPolicy::new_const()
    }

    #[test]
    fn test_configure_ok() {
        let p = fresh_policy();
        p.configure(200, true, 50_000).unwrap();
        assert_eq!(p.threshold_ppt(), 200);
        assert!(p.auto_enabled());
    }

    #[test]
    fn test_configure_invalid_threshold() {
        let p = fresh_policy();
        assert!(p.configure(1001, false, 1000).is_err());
    }

    #[test]
    fn test_configure_zero_interval_error() {
        let p = fresh_policy();
        assert!(p.configure(200, true, 0).is_err());
    }

    #[test]
    fn test_evaluate_disabled_returns_no_change() {
        let p = fresh_policy();
        p.configure(200, false, 1000).unwrap();
        let r = p.evaluate(100_000);
        assert_eq!(r.event, TuningEvent::NoChange);
    }

    #[test]
    fn test_evaluate_before_interval_no_change() {
        let p = fresh_policy();
        p.configure(200, true, 100_000).unwrap();
        // Premier appel OK
        p.evaluate(100_000);
        // Deuxième appel trop tôt
        let r = p.evaluate(100_001);
        assert_eq!(r.event, TuningEvent::NoChange);
    }

    #[test]
    fn test_evaluate_after_interval_triggers() {
        let p = fresh_policy();
        p.configure(200, true, 1_000).unwrap();
        let r = p.evaluate(1_000);
        // Doit avoir été évalué (pas NoChange si intervalle atteint)
        // Mais sans données de charge il ne change rien
        let _ = r;
        assert!(p.evaluations() >= 1);
    }

    #[test]
    fn test_force_strategy_override() {
        let p = fresh_policy();
        p.configure(200, true, 1).unwrap();
        p.force_strategy(Some(PlacementStrategy::RoundRobin));
        let r = p.evaluate(1);
        assert_eq!(r.event, TuningEvent::ManualOverride);
    }

    #[test]
    fn test_force_strategy_clear() {
        let p = fresh_policy();
        p.force_strategy(None);
        // Doit être u8::MAX
        assert_eq!(p.forced_strategy.load(Ordering::Relaxed), u8::MAX);
    }

    #[test]
    fn test_set_node_capacity_ok() {
        let p = fresh_policy();
        p.set_node_capacity(0, 8_000_000_000).unwrap();
        assert_eq!(
            p.node_capacity_bytes[0].load(Ordering::Relaxed),
            8_000_000_000
        );
    }

    #[test]
    fn test_set_node_capacity_invalid() {
        let p = fresh_policy();
        assert!(p.set_node_capacity(8, 1000).is_err());
    }

    #[test]
    fn test_pressure_zone_empty_node() {
        let p = fresh_policy();
        // Nœud sans capacité → Normal
        assert_eq!(p.pressure_zone(0), PressureZone::Normal);
    }

    #[test]
    fn test_pressure_zone_categorization() {
        assert_eq!(PressureZone::from_ppt(100), PressureZone::Normal);
        assert_eq!(PressureZone::from_ppt(720), PressureZone::Warning);
        assert_eq!(PressureZone::from_ppt(860), PressureZone::Critical);
        assert_eq!(PressureZone::from_ppt(960), PressureZone::Emergency);
    }

    #[test]
    fn test_imbalance_no_capacity_is_zero() {
        let p = fresh_policy();
        // Aucune capacité définie → imbalance = 0
        let imb = p._compute_imbalance_ppt();
        assert_eq!(imb, 0);
    }

    #[test]
    fn test_reset() {
        let p = fresh_policy();
        p.configure(200, true, 1).unwrap();
        p.evaluate(1);
        p.reset();
        assert_eq!(p.evaluations(), 0);
        assert_eq!(p.strategy_changes(), 0);
    }

    #[test]
    fn test_tuning_event_names() {
        assert_eq!(TuningEvent::StrategyChanged.name(), "strategy-changed");
        assert_eq!(TuningEvent::ImbalanceDetected.name(), "imbalance-detected");
        assert_eq!(TuningEvent::NoChange.name(), "no-change");
    }

    #[test]
    fn test_report_is_balanced() {
        let p = fresh_policy();
        p.configure(300, true, 1).unwrap();
        // Déséquilibre 0 < threshold 300 → balanced
        let r = p._make_report(
            TuningEvent::NoChange,
            0,
            PlacementStrategy::RoundRobin,
            PlacementStrategy::RoundRobin,
            0,
        );
        assert!(r.is_balanced());
    }
}
