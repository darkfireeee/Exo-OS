//! ExoFS NUMA facade (`NumaConfig`, `NumaModule`, global `NUMA`) and tests.
//!
//! Extracted from `exofs/numa/mod.rs` so `mod.rs` remains a pure wiring facade.

use alloc::vec::Vec;

use super::{
    CpuId, NumaNodeId, NumaNodeStats, PlacementHint, PlacementStrategy, PressureZone, TuningReport,
    AFFINITY_MAP, NUMA_MIGRATION, NUMA_PLACEMENT, NUMA_POLICY, NUMA_STATS,
};
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

#[derive(Clone, Copy, Debug)]
pub struct NumaConfig {
    pub n_nodes: u8,
    pub initial_strategy: PlacementStrategy,
    pub auto_tune: bool,
    pub imbalance_threshold_ppt: u64,
    pub tune_interval_ticks: u64,
    pub node_capacity_bytes: u64,
}

impl NumaConfig {
    pub const fn default_config() -> Self {
        Self {
            n_nodes: 1,
            initial_strategy: PlacementStrategy::RoundRobin,
            auto_tune: false,
            imbalance_threshold_ppt: 300,
            tune_interval_ticks: 100_000,
            node_capacity_bytes: 0,
        }
    }

    pub const fn multi_node(n: u8) -> Self {
        Self {
            n_nodes: n,
            initial_strategy: PlacementStrategy::LeastUsed,
            auto_tune: true,
            imbalance_threshold_ppt: 200,
            tune_interval_ticks: 50_000,
            node_capacity_bytes: 0,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.n_nodes == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.imbalance_threshold_ppt > 1000 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.tune_interval_ticks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumaModuleState {
    Uninitialized,
    Ready,
    Degraded,
}

impl NumaModuleState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
        }
    }
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

pub struct NumaModule {
    config: core::cell::UnsafeCell<NumaConfig>,
    state: core::cell::UnsafeCell<NumaModuleState>,
    lock: core::sync::atomic::AtomicU64,
}

unsafe impl Sync for NumaModule {}
unsafe impl Send for NumaModule {}

impl NumaModule {
    pub const fn new_const() -> Self {
        Self {
            config: core::cell::UnsafeCell::new(NumaConfig::default_config()),
            state: core::cell::UnsafeCell::new(NumaModuleState::Uninitialized),
            lock: core::sync::atomic::AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        use core::sync::atomic::Ordering;
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn release(&self) {
        use core::sync::atomic::Ordering;
        self.lock.store(0, Ordering::Release);
    }

    pub fn init(&self, cfg: NumaConfig, _tick: u64) -> ExofsResult<()> {
        cfg.validate()?;
        self.acquire();
        let state = unsafe { &mut *self.state.get() };
        if *state == NumaModuleState::Ready {
            self.release();
            return Ok(());
        }
        unsafe {
            *self.config.get() = cfg;
        }
        self.release();

        NUMA_PLACEMENT.init(cfg.n_nodes, cfg.initial_strategy)?;
        NUMA_POLICY.configure(
            cfg.imbalance_threshold_ppt,
            cfg.auto_tune,
            cfg.tune_interval_ticks,
        )?;

        if cfg.node_capacity_bytes > 0 {
            let mut i = 0usize;
            while i < cfg.n_nodes as usize {
                NUMA_POLICY.set_node_capacity(i, cfg.node_capacity_bytes)?;
                i = i.wrapping_add(1);
            }
        }

        self.acquire();
        *unsafe { &mut *self.state.get() } = NumaModuleState::Ready;
        self.release();
        Ok(())
    }

    pub fn state(&self) -> NumaModuleState {
        self.acquire();
        let s = unsafe { *self.state.get() };
        self.release();
        s
    }

    pub fn config(&self) -> NumaConfig {
        self.acquire();
        let c = unsafe { *self.config.get() };
        self.release();
        c
    }

    pub fn preferred_node_for(&self, blob_id: Option<BlobId>) -> ExofsResult<NumaNodeId> {
        let hint = PlacementHint::simple(blob_id);
        let result = NUMA_PLACEMENT.preferred_node(&hint)?;
        Ok(result.node)
    }

    pub fn record_alloc(&self, node: NumaNodeId, bytes: u64) {
        if node.is_valid() {
            NUMA_STATS.record_alloc(node.idx(), bytes);
        }
    }

    pub fn record_free(&self, node: NumaNodeId, bytes: u64) {
        if node.is_valid() {
            NUMA_STATS.record_free(node.idx(), bytes);
        }
    }

    pub fn record_io(&self, node: NumaNodeId, read_bytes: u64, write_bytes: u64) {
        if !node.is_valid() {
            return;
        }
        if read_bytes > 0 {
            NUMA_STATS.record_read(node.idx(), read_bytes);
        }
        if write_bytes > 0 {
            NUMA_STATS.record_write(node.idx(), write_bytes);
        }
    }

    pub fn register_cpu(&self, cpu: CpuId, node: NumaNodeId) -> ExofsResult<()> {
        AFFINITY_MAP.register_cpu(cpu, node)
    }

    pub fn node_of_cpu(&self, cpu: CpuId) -> Option<NumaNodeId> {
        AFFINITY_MAP.node_of_cpu(cpu)
    }

    pub fn tune(&self, tick: u64) -> TuningReport {
        NUMA_POLICY.evaluate(tick)
    }

    pub fn pressure_zone(&self, node: NumaNodeId) -> PressureZone {
        NUMA_POLICY.pressure_zone(node.idx())
    }

    pub fn is_healthy(&self) -> bool {
        if !self.state().is_ready() {
            return false;
        }
        NUMA_STATS.is_healthy() && NUMA_MIGRATION.is_healthy()
    }

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

    pub fn reset_all_stats(&self) {
        NUMA_STATS.reset_all();
        NUMA_MIGRATION.reset_stats();
        NUMA_POLICY.reset();
        NUMA_PLACEMENT.reset_counters();
    }
}

pub static NUMA: NumaModule = NumaModule::new_const();

pub fn numa_init(n_nodes: u8, tick: u64) -> ExofsResult<()> {
    let cfg = if n_nodes <= 1 {
        NumaConfig::default_config()
    } else {
        NumaConfig::multi_node(n_nodes)
    };
    NUMA.init(cfg, tick)
}

pub fn numa_preferred_node(blob_id: Option<BlobId>) -> usize {
    NUMA.preferred_node_for(blob_id)
        .map(|n| n.idx())
        .unwrap_or(0)
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> NumaConfig {
        NumaConfig::default_config()
    }
    fn multi_cfg() -> NumaConfig {
        NumaConfig::multi_node(4)
    }

    #[test]
    fn test_config_validate_ok() {
        default_cfg().validate().test_unwrap();
        multi_cfg().validate().test_unwrap();
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
        m.init(default_cfg(), 0).test_unwrap();
        assert!(m.state().is_ready());
    }

    #[test]
    fn test_module_double_init_ok() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        m.init(default_cfg(), 1).test_unwrap();
        assert_eq!(m.state(), NumaModuleState::Ready);
    }

    #[test]
    fn test_preferred_node_single_node() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        let n = m.preferred_node_for(None).test_unwrap();
        assert_eq!(n, NumaNodeId(0));
    }

    #[test]
    fn test_record_alloc_and_stat() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        NUMA_STATS.reset_all();
        m.record_alloc(NumaNodeId(0), 4096);
        assert_eq!(NUMA_STATS.node_stats(0).allocs, 1);
    }

    #[test]
    fn test_record_free_and_stat() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        NUMA_STATS.reset_all();
        m.record_alloc(NumaNodeId(0), 8192);
        m.record_free(NumaNodeId(0), 4096);
        assert_eq!(NUMA_STATS.node_stats(0).net_bytes(), 4096);
    }

    #[test]
    fn test_record_io() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        NUMA_STATS.reset_all();
        m.record_io(NumaNodeId(0), 1024, 512);
        let s = NUMA_STATS.node_stats(0);
        assert_eq!(s.read_bytes, 1024);
        assert_eq!(s.write_bytes, 512);
    }

    #[test]
    fn test_register_cpu() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        m.register_cpu(CpuId(10), NumaNodeId(0)).test_unwrap();
        assert_eq!(m.node_of_cpu(CpuId(10)), Some(NumaNodeId(0)));
    }

    #[test]
    fn test_all_node_stats_len() {
        let m = NumaModule::new_const();
        m.init(multi_cfg(), 0).test_unwrap();
        let stats = m.all_node_stats().test_unwrap();
        assert_eq!(stats.len(), 4);
    }

    #[test]
    fn test_is_healthy_initial() {
        let m = NumaModule::new_const();
        m.init(default_cfg(), 0).test_unwrap();
        NUMA_STATS.reset_all();
        NUMA_MIGRATION.reset_stats();
        assert!(m.is_healthy());
    }

    #[test]
    fn test_numa_init_fn_single() {
        numa_init(1, 0).test_unwrap();
    }

    #[test]
    fn test_numa_init_fn_multi() {
        numa_init(4, 0).test_unwrap();
    }

    #[test]
    fn test_numa_preferred_node_fn() {
        numa_init(1, 0).test_unwrap();
        let n = numa_preferred_node(None);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_state_name() {
        assert_eq!(NumaModuleState::Ready.name(), "ready");
        assert_eq!(NumaModuleState::Uninitialized.name(), "uninitialized");
        assert_eq!(NumaModuleState::Degraded.name(), "degraded");
    }
}
