// kernel/src/memory/dma/channels/affinity.rs
//
// CPU/NUMA affinity of DMA channels.
//
// Allows associating each DMA channel with:
//   - a preferred CPU (for interrupt affinity and per-CPU pools),
//   - a NUMA node (to favor local RAM),
//   - a CPU mask (for multi-CPU IRQs).
//
// The channel manager consults these affinities when allocating
// a channel for a request on a given CPU, in order to minimize
// transaction latency (local data + IRQ handled on the right CPU).
//
// LAYER 0 — zero external dependencies.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::memory::dma::channels::manager::MAX_DMA_CHANNELS;
use crate::memory::dma::core::types::DmaChannelId;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinel value: no CPU affinity defined.
pub const CPU_AFFINITY_NONE: u8 = u8::MAX;

/// Sentinel value: no preferred NUMA node.
pub const NUMA_NODE_NONE: u8 = u8::MAX;

/// Maximum number of CPUs tracked in mask (bits).
pub const MAX_AFFINITY_CPUS: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// AFFINITY ENTRY
// ─────────────────────────────────────────────────────────────────────────────

/// CPU/NUMA affinity of a DMA channel.
#[repr(C, align(16))]
struct AffinityEntry {
    /// Preferred CPU for IRQ and completion processing (255 = none).
    preferred_cpu: AtomicU8,
    /// Preferred NUMA node for memory allocations (255 = none).
    numa_node: AtomicU8,
    /// Mask of CPUs allowed to handle IRQs for this channel (64 bits max).
    /// Bit i = 1 -> CPU i can handle interrupts for this channel.
    /// 0 = unconfigured (all CPUs allowed).
    cpu_mask: AtomicU64,
    /// Number of times this channel was selected from its preferred CPU.
    local_hits: AtomicU32,
    /// Number of times this channel was selected from a non-preferred CPU.
    remote_hits: AtomicU32,
}

impl AffinityEntry {
    const fn new() -> Self {
        AffinityEntry {
            preferred_cpu: AtomicU8::new(CPU_AFFINITY_NONE),
            numa_node: AtomicU8::new(NUMA_NODE_NONE),
            cpu_mask: AtomicU64::new(0),
            local_hits: AtomicU32::new(0),
            remote_hits: AtomicU32::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AFFINITY TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// CPU/NUMA affinity table for all registered DMA channels.
///
/// Indexed by `DmaChannelId.0`. Thread-safe: each field is atomic.
pub struct DmaAffinityTable {
    entries: [AffinityEntry; MAX_DMA_CHANNELS],
    /// Global statistics.
    pub stats_local_total: AtomicU64,
    pub stats_remote_total: AtomicU64,
}

// SAFETY: DmaAffinityTable uses only atomics.
unsafe impl Sync for DmaAffinityTable {}
unsafe impl Send for DmaAffinityTable {}

impl Default for DmaAffinityTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaAffinityTable {
    const fn new() -> Self {
        DmaAffinityTable {
            entries: [const { AffinityEntry::new() }; MAX_DMA_CHANNELS],
            stats_local_total: AtomicU64::new(0),
            stats_remote_total: AtomicU64::new(0),
        }
    }

    // ── Configuration ────────────────────────────────────────────────────────

    /// Sets preferred CPU of a channel.
    ///
    /// `cpu` = `CPU_AFFINITY_NONE` to clear preference.
    pub fn set_preferred_cpu(&self, channel: DmaChannelId, cpu: u8) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .preferred_cpu
                .store(cpu, Ordering::Relaxed);
        }
    }

    /// Sets preferred NUMA node of a channel.
    pub fn set_numa_node(&self, channel: DmaChannelId, node: u8) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .numa_node
                .store(node, Ordering::Relaxed);
        }
    }

    /// Sets CPU mask for IRQs of this channel.
    ///
    /// `mask` = 0 -> all CPUs are allowed (no constraint).
    pub fn set_cpu_mask(&self, channel: DmaChannelId, mask: u64) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .cpu_mask
                .store(mask, Ordering::Relaxed);
        }
    }

    // ── Read ─────────────────────────────────────────────────────────────────

    /// Returns preferred CPU of a channel (or `CPU_AFFINITY_NONE`).
    pub fn preferred_cpu(&self, channel: DmaChannelId) -> u8 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return CPU_AFFINITY_NONE;
        }
        self.entries[channel.0 as usize]
            .preferred_cpu
            .load(Ordering::Relaxed)
    }

    /// Returns preferred NUMA node of a channel (or `NUMA_NODE_NONE`).
    pub fn numa_node(&self, channel: DmaChannelId) -> u8 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return NUMA_NODE_NONE;
        }
        self.entries[channel.0 as usize]
            .numa_node
            .load(Ordering::Relaxed)
    }

    /// Returns CPU mask of a channel (0 = no constraint).
    pub fn cpu_mask(&self, channel: DmaChannelId) -> u64 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return 0;
        }
        self.entries[channel.0 as usize]
            .cpu_mask
            .load(Ordering::Relaxed)
    }

    // ── Selection with accounting ────────────────────────────────────────────

    /// Records that a channel was used from `cpu_id` (for statistics).
    ///
    /// Updates `local_hits` / `remote_hits` counters depending on whether
    /// `cpu_id` matches the channel's preferred CPU or not.
    pub fn record_usage(&self, channel: DmaChannelId, cpu_id: u8) {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return;
        }
        let entry = &self.entries[channel.0 as usize];
        let pref = entry.preferred_cpu.load(Ordering::Relaxed);
        if pref == CPU_AFFINITY_NONE || pref == cpu_id {
            entry.local_hits.fetch_add(1, Ordering::Relaxed);
            self.stats_local_total.fetch_add(1, Ordering::Relaxed);
        } else {
            entry.remote_hits.fetch_add(1, Ordering::Relaxed);
            self.stats_remote_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── Selection of best channel for a given CPU ────────────────────────────

    /// Returns index of the channel closest to `cpu_id` among channels
    /// whose IDs are listed in `candidates`.
    ///
    /// Selection criteria (descending priority order):
    ///   1. Channel where `preferred_cpu == cpu_id` (exact match).
    ///   2. Channel whose `cpu_mask` contains `cpu_id` (partial affinity).
    ///   3. Channel with no defined affinity (fallback).
    ///
    /// Returns `None` if `candidates` is empty.
    pub fn best_for_cpu(
        &self,
        candidates: &[DmaChannelId],
        cpu_id: u8,
    ) -> Option<DmaChannelId> {
        let mut best_exact: Option<DmaChannelId> = None;
        let mut best_partial: Option<DmaChannelId> = None;
        let mut best_any: Option<DmaChannelId> = None;

        for &ch in candidates {
            if (ch.0 as usize) >= MAX_DMA_CHANNELS {
                continue;
            }
            let entry = &self.entries[ch.0 as usize];
            let pref = entry.preferred_cpu.load(Ordering::Relaxed);
            let mask = entry.cpu_mask.load(Ordering::Relaxed);

            if pref != CPU_AFFINITY_NONE && pref == cpu_id {
                best_exact = Some(ch);
                break; // Perfect match, no need to continue.
            }
            if mask != 0 && mask & (1u64 << (cpu_id as u64)) != 0 && best_partial.is_none() {
                best_partial = Some(ch);
            }
            if best_any.is_none() {
                best_any = Some(ch);
            }
        }

        best_exact.or(best_partial).or(best_any)
    }

    /// Global locality rate: proportion of accesses from preferred CPU.
    pub fn locality_rate(&self) -> u32 {
        let local = self.stats_local_total.load(Ordering::Relaxed);
        let remote = self.stats_remote_total.load(Ordering::Relaxed);
        let total = local + remote;
        if total == 0 {
            return 100;
        }
        ((local * 100) / total) as u32
    }
}

/// Global DMA affinity table.
pub static DMA_AFFINITY: DmaAffinityTable = DmaAffinityTable::new();
