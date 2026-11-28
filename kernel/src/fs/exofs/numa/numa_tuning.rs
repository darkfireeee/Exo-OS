// SPDX-License-Identifier: GPL-3.0
// ExoFS NUMA — Adaptive NUMA policy and auto-tuning
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::numa_affinity::MAX_NUMA_NODES;
use super::numa_placement::{PlacementStrategy, NUMA_PLACEMENT};
use super::numa_stats::NUMA_STATS;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─── TuningEvent ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuningEvent {
    /// Strategy modified automatically.
    StrategyChanged,
    /// Imbalance detected but below threshold.
    ImbalanceDetected,
    /// Manual override applied.
    ManualOverride,
    /// No change needed.
    NoChange,
    /// Policy reset.
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

/// Memory pressure level on a node.
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

/// Adaptive NUMA policy.
pub struct NumaPolicy {
    /// Imbalance threshold in ‰ to trigger strategy change.
    migrate_threshold_ppt: AtomicU64,
    /// Auto-tuning enabled.
    auto_tune_enabled: AtomicU8,
    /// Minimum interval between two evaluations (ticks).
    tune_interval_ticks: AtomicU64,
    /// Last evaluation tick.
    last_tune_tick: AtomicU64,
    /// Total number of evaluations.
    evaluations: AtomicU64,
    /// Number of strategy changes.
    strategy_changes: AtomicU64,
    /// Reference memory capacity per node in bytes.
    node_capacity_bytes: [AtomicU64; MAX_NUMA_NODES],
    /// Manually forced strategy (u8::MAX = none).
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

    /// Configures the policy.
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

    /// Sets a node's memory capacity.
    pub fn set_node_capacity(&self, node: usize, capacity_bytes: u64) -> ExofsResult<()> {
        if node >= MAX_NUMA_NODES {
            return Err(ExofsError::InvalidArgument);
        }
        self.node_capacity_bytes[node].store(capacity_bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Forces a strategy (u8::MAX = releases constraint).
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

    // ── Evaluation ────────────────────────────────────────────────────────────

    /// Evaluates node load and adjusts strategy if needed.
    pub fn evaluate(&self, current_tick: u64) -> TuningReport {
        let old_strategy = NUMA_PLACEMENT.strategy();

        // Check active and interval
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

        // Manually forced strategy
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

        // Imbalance computation (ARITH-02: saturating, checked_div, RECUR-01: while)
        let imbalance = self._compute_imbalance_ppt();
        let threshold = self.migrate_threshold_ppt.load(Ordering::Relaxed);

        let new_strategy = if imbalance > threshold {
            PlacementStrategy::LeastUsed
        } else {
            PlacementStrategy::LocalFirst
        };

        // Apply if different
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

    /// Computes imbalance in ‰ between nodes (RECUR-01: while, ARITH-02: saturating).
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
        // ARITH-02: saturating_sub + checked_div
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

    // ── Pressure per node ─────────────────────────────────────────────────────

    /// Current pressure level of a node (ARITH-02).
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

    /// Node under highest pressure (RECUR-01: while).
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

    /// Resets policy state.
    pub fn reset(&self) {
        self.last_tune_tick.store(0, Ordering::Relaxed);
        self.evaluations.store(0, Ordering::Relaxed);
        self.strategy_changes.store(0, Ordering::Relaxed);
        self.forced_strategy.store(u8::MAX, Ordering::Relaxed);
    }
}

/// Global singleton of NUMA policy.
pub static NUMA_POLICY: NumaPolicy = NumaPolicy::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_policy() -> NumaPolicy {
        NumaPolicy::new_const()
    }

    #[test]
    fn test_configure_ok() {
        let p = fresh_policy();
        p.configure(200, true, 50_000).test_unwrap();
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
        p.configure(200, false, 1000).test_unwrap();
        let r = p.evaluate(100_000);
        assert_eq!(r.event, TuningEvent::NoChange);
    }

    #[test]
    fn test_evaluate_before_interval_no_change() {
        let p = fresh_policy();
        p.configure(200, true, 100_000).test_unwrap();
        // First call OK
        p.evaluate(100_000);
        // Second call too early
        let r = p.evaluate(100_001);
        assert_eq!(r.event, TuningEvent::NoChange);
    }

    #[test]
    fn test_evaluate_after_interval_triggers() {
        let p = fresh_policy();
        p.configure(200, true, 1_000).test_unwrap();
        let r = p.evaluate(1_000);
        // Must have been evaluated (not NoChange if interval reached)
        // But with no load data it changes nothing
        let _ = r;
        assert!(p.evaluations() >= 1);
    }

    #[test]
    fn test_force_strategy_override() {
        let p = fresh_policy();
        p.configure(200, true, 1).test_unwrap();
        p.force_strategy(Some(PlacementStrategy::RoundRobin));
        let r = p.evaluate(1);
        assert_eq!(r.event, TuningEvent::ManualOverride);
    }

    #[test]
    fn test_force_strategy_clear() {
        let p = fresh_policy();
        p.force_strategy(None);
        // Must be u8::MAX
        assert_eq!(p.forced_strategy.load(Ordering::Relaxed), u8::MAX);
    }

    #[test]
    fn test_set_node_capacity_ok() {
        let p = fresh_policy();
        p.set_node_capacity(0, 8_000_000_000).test_unwrap();
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
        // Node with no capacity -> Normal
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
        // No defined capacity -> imbalance = 0
        let imb = p._compute_imbalance_ppt();
        assert_eq!(imb, 0);
    }

    #[test]
    fn test_reset() {
        let p = fresh_policy();
        p.configure(200, true, 1).test_unwrap();
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
        p.configure(300, true, 1).test_unwrap();
        // Imbalance 0 < threshold 300 -> balanced
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
