// SPDX-License-Identifier: MIT
// ExoFS NUMA — Module principal (façade publique)
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

//! # ExoFS NUMA
//!
//! Ce module gère la topologie NUMA pour le système de fichiers ExoFS :
//! - **Affinité**   : carte CPU↔nœud NUMA avec distances inter-nœuds.
//! - **Statistiques**: compteurs atomiques par nœud (allocs, I/O, migrations).
//! - **Placement**  : stratégies de placement (RoundRobin, LeastUsed, ContentHash…).
//! - **Migration**  : déplacement de blobs entre nœuds avec file de contrôle.
//! - **Tuning**     : politique adaptative qui ajuste les stratégies dynamiquement.
//!
//! ## Règles d'implémentation
//! - RECUR-01 : aucune récursion, boucles `while` uniquement.
//! - OOM-02   : `try_reserve(n).map_err(|_| ExofsError::NoMemory)?` avant push.
//! - ARITH-02 : `saturating_add/sub`, `checked_div`, `wrapping_add/mul`.
//! - Jamais `FsError` : uniquement `ExofsError` / `ExofsResult`.

pub mod numa_affinity;
pub mod numa_stats;
pub mod numa_placement;
pub mod numa_migration;
pub mod numa_tuning;

// ─── Réexports publics ────────────────────────────────────────────────────────

// Affinité
pub use numa_affinity::{
    AffinityMap, AffinityNodeEntry, CpuId, NumaNodeId,
    AFFINITY_MAP, MAX_NUMA_NODES, MAX_CPUS, CPU_NODE_NONE,
};

// Statistiques
pub use numa_stats::{
    NumaStats, NumaNodeStats, NUMA_STATS,
};

// Placement
pub use numa_placement::{
    PlacementStrategy, PlacementHint, PlacementResult, NumaPlacement,
    NUMA_PLACEMENT,
};

// Migration
pub use numa_migration::{
    MigrationStatus, MigrationResult, MigrationPolicy, MigrationQueue,
    NumaMigration, BlobNodeLocator, NUMA_MIGRATION, MIGRATION_QUEUE_MAX,
};

// Tuning
pub use numa_tuning::{
    TuningEvent, TuningReport, PressureZone, NumaPolicy, NUMA_POLICY,
};

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};

// ─── NumaConfig ───────────────────────────────────────────────────────────────

/// Configuration globale du module NUMA.
#[derive(Clone, Copy, Debug)]
pub struct NumaConfig {
    /// Nombre de nœuds NUMA actifs.
    pub n_nodes:               u8,
    /// Stratégie de placement initiale.
    pub initial_strategy:      PlacementStrategy,
    /// Auto-tuning activé.
    pub auto_tune:             bool,
    /// Seuil de déséquilibre en ‰ (entre 0 et 1000).
    pub imbalance_threshold_ppt: u64,
    /// Intervalle entre évaluations de tuning.
    pub tune_interval_ticks:   u64,
    /// Capacité mémoire de référence par nœud (octets, 0 = inconnue).
    pub node_capacity_bytes:   u64,
}

impl NumaConfig {
    pub const fn default_config() -> Self {
        Self {
            n_nodes:                 1,
            initial_strategy:        PlacementStrategy::RoundRobin,
            auto_tune:               false,
            imbalance_threshold_ppt: 300,
            tune_interval_ticks:     100_000,
            node_capacity_bytes:     0,
        }
    }

    pub const fn multi_node(n: u8) -> Self {
        Self {
            n_nodes:                 n,
            initial_strategy:        PlacementStrategy::LeastUsed,
            auto_tune:               true,
            imbalance_threshold_ppt: 200,
            tune_interval_ticks:     50_000,
            node_capacity_bytes:     0,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.n_nodes == 0 { return Err(ExofsError::InvalidArgument); }
        if self.imbalance_threshold_ppt > 1000 { return Err(ExofsError::InvalidArgument); }
        if self.tune_interval_ticks == 0 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

// ─── NumaModuleState ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumaModuleState { Uninitialized, Ready, Degraded }

impl NumaModuleState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Ready         => "ready",
            Self::Degraded      => "degraded",
        }
    }
    pub fn is_ready(self) -> bool { matches!(self, Self::Ready | Self::Degraded) }
}

// ─── NumaModule ───────────────────────────────────────────────────────────────

/// Façade principale du module NUMA.
pub struct NumaModule {
    config: core::cell::UnsafeCell<NumaConfig>,
    state:  core::cell::UnsafeCell<NumaModuleState>,
    lock:   core::sync::atomic::AtomicU64,
}

unsafe impl Sync for NumaModule {}
unsafe impl Send for NumaModule {}

impl NumaModule {
    pub const fn new_const() -> Self {
        Self {
            config: core::cell::UnsafeCell::new(NumaConfig::default_config()),
            state:  core::cell::UnsafeCell::new(NumaModuleState::Uninitialized),
            lock:   core::sync::atomic::AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        use core::sync::atomic::Ordering;
        while self.lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        { core::hint::spin_loop(); }
    }
    fn release(&self) {
        use core::sync::atomic::Ordering;
        self.lock.store(0, Ordering::Release);
    }

    /// Initialise le module NUMA.
    pub fn init(&self, cfg: NumaConfig, tick: u64) -> ExofsResult<()> {
        cfg.validate()?;
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let state = unsafe { &mut *self.state.get() };
        if *state == NumaModuleState::Ready {
            self.release();
            return Ok(());
        }
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { *self.config.get() = cfg; }
        self.release();

        // Initialiser le placement
        NUMA_PLACEMENT.init(cfg.n_nodes, cfg.initial_strategy)?;

        // Configurer le tuning
        NUMA_POLICY.configure(cfg.imbalance_threshold_ppt, cfg.auto_tune, cfg.tune_interval_ticks)?;

        // Capacité mémoire par nœud si fournie
        if cfg.node_capacity_bytes > 0 {
            let mut i = 0usize;
            while i < cfg.n_nodes as usize {
                NUMA_POLICY.set_node_capacity(i, cfg.node_capacity_bytes)?;
                i = i.wrapping_add(1);
            }
        }

        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        *unsafe { &mut *self.state.get() } = NumaModuleState::Ready;
        self.release();
        Ok(())
    }

    pub fn state(&self) -> NumaModuleState {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { *self.state.get() };
        self.release();
        s
    }

    pub fn config(&self) -> NumaConfig {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let c = unsafe { *self.config.get() };
        self.release();
        c
    }

    // ── Placement ─────────────────────────────────────────────────────────────

    /// Retourne le nœud NUMA préféré pour un blob.
    pub fn preferred_node_for(&self, blob_id: Option<BlobId>) -> ExofsResult<NumaNodeId> {
        let hint = PlacementHint::simple(blob_id);
        let result = NUMA_PLACEMENT.preferred_node(&hint)?;
        Ok(result.node)
    }

    /// Enregistre une allocation sur un nœud.
    pub fn record_alloc(&self, node: NumaNodeId, bytes: u64) {
        if node.is_valid() { NUMA_STATS.record_alloc(node.idx(), bytes); }
    }

    /// Enregistre une libération sur un nœud.
    pub fn record_free(&self, node: NumaNodeId, bytes: u64) {
        if node.is_valid() { NUMA_STATS.record_free(node.idx(), bytes); }
    }

    /// Enregistre une opération I/O.
    pub fn record_io(&self, node: NumaNodeId, read_bytes: u64, write_bytes: u64) {
        if !node.is_valid() { return; }
        if read_bytes  > 0 { NUMA_STATS.record_read(node.idx(),  read_bytes); }
        if write_bytes > 0 { NUMA_STATS.record_write(node.idx(), write_bytes); }
    }

    // ── Affinité ──────────────────────────────────────────────────────────────

    /// Enregistre un CPU sur un nœud.
    pub fn register_cpu(&self, cpu: CpuId, node: NumaNodeId) -> ExofsResult<()> {
        AFFINITY_MAP.register_cpu(cpu, node)
    }

    /// Nœud NUMA d'un CPU.
    pub fn node_of_cpu(&self, cpu: CpuId) -> Option<NumaNodeId> {
        AFFINITY_MAP.node_of_cpu(cpu)
    }

    // ── Tuning ────────────────────────────────────────────────────────────────

    /// Lance une évaluation du tuning.
    pub fn tune(&self, tick: u64) -> TuningReport {
        NUMA_POLICY.evaluate(tick)
    }

    /// Pression sur un nœud.
    pub fn pressure_zone(&self, node: NumaNodeId) -> PressureZone {
        NUMA_POLICY.pressure_zone(node.idx())
    }

    // ── Santé ─────────────────────────────────────────────────────────────────

    /// Vrai si le système NUMA est sain.
    pub fn is_healthy(&self) -> bool {
        if !self.state().is_ready() { return false; }
        NUMA_STATS.is_healthy() && NUMA_MIGRATION.is_healthy()
    }

    /// Snapshot des statistiques de tous les nœuds (OOM-02, RECUR-01).
    pub fn all_node_stats(&self) -> ExofsResult<Vec<NumaNodeStats>> {
        let n = self.config().n_nodes as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < n {
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(NUMA_STATS.node_stats(i));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Réinitialise toutes les statistiques.
    pub fn reset_all_stats(&self) {
        NUMA_STATS.reset_all();
        NUMA_MIGRATION.reset_stats();
        NUMA_POLICY.reset();
        NUMA_PLACEMENT.reset_counters();
    }
}

/// Singleton global du module NUMA.
pub static NUMA: NumaModule = NumaModule::new_const();

// ─── Fonctions utilitaires globales ───────────────────────────────────────────

/// Initialise le module NUMA avec la configuration par défaut.
pub fn numa_init(n_nodes: u8, tick: u64) -> ExofsResult<()> {
    let cfg = if n_nodes <= 1 {
        NumaConfig::default_config()
    } else {
        NumaConfig::multi_node(n_nodes)
    };
    NUMA.init(cfg, tick)
}

/// Retourne le nœud NUMA préféré pour un blob (façade simple).
pub fn numa_preferred_node(blob_id: Option<BlobId>) -> usize {
    NUMA.preferred_node_for(blob_id)
        .map(|n| n.idx())
        .unwrap_or(0)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> NumaConfig { NumaConfig::default_config() }
    fn multi_cfg()   -> NumaConfig { NumaConfig::multi_node(4) }

    #[test]
    fn test_config_validate_ok() {
        default_cfg().validate().unwrap();
        multi_cfg().validate().unwrap();
    }

    #[test]
    fn test_config_validate_zero_nodes() {
        let mut c = default_cfg();
        c.n_nodes = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_config_validate_bad_threshold() {
        let mut c = default_cfg();
        c.imbalance_threshold_ppt = 1001;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_module_init() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_module_double_init_ok() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        m.init(default_cfg(), 1).unwrap();
        assert_eq!(m.state(), NumaModuleState::Ready);
    }

    #[test]
    fn test_preferred_node_single_node() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        let n = m.preferred_node_for(None).unwrap();
        assert_eq!(n, NumaNodeId(0));
    }

    #[test]
    fn test_record_alloc_and_stat() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        NUMA_STATS.reset_all();
        m.record_alloc(NumaNodeId(0), 4096);
        assert_eq!(NUMA_STATS.node_stats(0).allocs, 1);
    }

    #[test]
    fn test_record_free_and_stat() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        NUMA_STATS.reset_all();
        m.record_alloc(NumaNodeId(0), 8192);
        m.record_free(NumaNodeId(0), 4096);
        assert_eq!(NUMA_STATS.node_stats(0).net_bytes(), 4096);
    }

    #[test]
    fn test_record_io() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        NUMA_STATS.reset_all();
        m.record_io(NumaNodeId(0), 1024, 512);
        let s = NUMA_STATS.node_stats(0);
        assert_eq!(s.read_bytes, 1024);
        assert_eq!(s.write_bytes, 512);
    }

    #[test]
    fn test_register_cpu() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        m.register_cpu(CpuId(10), NumaNodeId(0)).unwrap();
        assert_eq!(m.node_of_cpu(CpuId(10)), Some(NumaNodeId(0)));
    }

    #[test]
    fn test_all_node_stats_len() {
        let m = NumaModule::new_const();
        m.init(multi_cfg(), 0).unwrap();
        let stats = m.all_node_stats().unwrap();
        assert_eq!(stats.len(), 4);
    }

    #[test]
    fn test_is_healthy_initial() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).unwrap();
        NUMA_STATS.reset_all();
        NUMA_MIGRATION.reset_stats();
        assert!(m.is_healthy());
    }

    #[test]
    fn test_numa_init_fn_single() {
        numa_init(1, 0).unwrap();
    }

    #[test]
    fn test_numa_init_fn_multi() {
        numa_init(4, 0).unwrap();
    }

    #[test]
    fn test_numa_preferred_node_fn() {
        numa_init(1, 0).unwrap();
        let n = numa_preferred_node(None);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_state_name() {
        assert_eq!(NumaModuleState::Ready.name(),         "ready");
        assert_eq!(NumaModuleState::Uninitialized.name(), "uninitialized");
        assert_eq!(NumaModuleState::Degraded.name(),      "degraded");
    }
}
