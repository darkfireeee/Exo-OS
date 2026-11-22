//! High-precision TSC timing metrics stored in SSR v7 at `SSR_METRICS_OFFSET (0xC000)` (P1).
//!
//! Fits strictly inside the 16 KiB metrics region (`0xC000..0x10000`) with zero layout breakage.
//! Uses 128-bit integer arithmetic without floating point in Ring 0.

/// Magic identifying a populated `PhoenixTimingRecord` (`"PMET"`).
pub const PHOENIX_METRICS_MAGIC: u32 = 0x504D_4554;

/// Trigger reason constants stored in `PhoenixTimingRecord::trigger_reason`.
pub const PHOENIX_REASON_FORCE_SYSCALL: u32 = 1;
pub const PHOENIX_REASON_PANIC: u32 = 2;
pub const PHOENIX_REASON_EXONMI: u32 = 3;
pub const PHOENIX_REASON_LIVE_UPDATE: u32 = 4;

/// Bit flags stored in `PhoenixTimingRecord::flags`.
pub const PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT: u32 = 1 << 0;
pub const PHOENIX_FLAG_COLD_DISK_FALLBACK: u32 = 1 << 1;

/// Serialized ExoPhoenix switch telemetry record stored at `SSR_BASE_PHYS + 0xC000`.
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhoenixTimingRecord {
    pub magic: u32,
    pub cycle_id: u32,
    pub trigger_reason: u32,
    pub flags: u32,
    pub tsc_khz: u64,

    // Serialized TSC milestones (`lfence; rdtsc` / `rdtscp; lfence`)
    pub t0_freeze_req: u64,
    pub t1_freeze_ack_all: u64,
    pub t2_iommu_and_forge: u64,
    pub t3_kernel_b_active: u64,
    pub t4_crypto_reseeded: u64,
    pub t5_strate1_ready: u64,
    pub t6_strate2_ready: u64,

    // Derived SLA durations in microseconds (128-bit overflow-free conversion)
    pub delta_s0_freeze_us: u64,
    pub delta_s0_forge_us: u64,
    pub delta_s0_us: u64,
    pub delta_crypto_us: u64,
    pub delta_sla_us: u64,
    pub delta_full_us: u64,

    // Monotonic post-switch epoch emitted by `crypto_server` on `PhoenixWakeEntropy`.
    pub crypto_epoch_id: u64,
}

impl PhoenixTimingRecord {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            cycle_id: 0,
            trigger_reason: 0,
            flags: 0,
            tsc_khz: 0,
            t0_freeze_req: 0,
            t1_freeze_ack_all: 0,
            t2_iommu_and_forge: 0,
            t3_kernel_b_active: 0,
            t4_crypto_reseeded: 0,
            t5_strate1_ready: 0,
            t6_strate2_ready: 0,
            delta_s0_freeze_us: 0,
            delta_s0_forge_us: 0,
            delta_s0_us: 0,
            delta_crypto_us: 0,
            delta_sla_us: 0,
            delta_full_us: 0,
            crypto_epoch_id: 0,
        }
    }

    /// Recomputes all microsecond deltas from the captured raw TSC timestamps using 128-bit math.
    pub fn compute_deltas(&mut self) {
        self.delta_s0_freeze_us = tsc_delta_to_us(self.t0_freeze_req, self.t1_freeze_ack_all, self.tsc_khz);
        self.delta_s0_forge_us = tsc_delta_to_us(self.t1_freeze_ack_all, self.t2_iommu_and_forge, self.tsc_khz);
        self.delta_s0_us = tsc_delta_to_us(self.t0_freeze_req, self.t3_kernel_b_active, self.tsc_khz);
        self.delta_crypto_us = tsc_delta_to_us(self.t3_kernel_b_active, self.t4_crypto_reseeded, self.tsc_khz);
        self.delta_sla_us = tsc_delta_to_us(self.t0_freeze_req, self.t5_strate1_ready, self.tsc_khz);
        self.delta_full_us = tsc_delta_to_us(self.t0_freeze_req, self.t6_strate2_ready, self.tsc_khz);
    }

    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.magic == PHOENIX_METRICS_MAGIC && self.tsc_khz > 0
    }

    #[inline]
    pub const fn is_degraded(&self) -> bool {
        (self.flags & PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT) != 0
    }

    #[inline]
    pub const fn reason_str(&self) -> &'static str {
        match self.trigger_reason {
            PHOENIX_REASON_FORCE_SYSCALL => "FORCE",
            PHOENIX_REASON_PANIC => "PANIC",
            PHOENIX_REASON_EXONMI => "EXONMI",
            PHOENIX_REASON_LIVE_UPDATE => "LIVEUPDATE",
            _ => "UNKNOWN",
        }
    }
}

/// Overflow-free 128-bit TSC-to-microseconds conversion (Spec §5.2):
/// `floor((t_end - t_start) * 1_000 / tsc_khz)`.
#[inline]
pub const fn tsc_delta_to_us(t_start: u64, t_end: u64, tsc_khz: u64) -> u64 {
    if tsc_khz == 0 || t_end <= t_start {
        return 0;
    }
    let delta = (t_end - t_start) as u128;
    let us = (delta * 1_000u128) / (tsc_khz as u128);
    if us > u64::MAX as u128 {
        u64::MAX
    } else {
        us as u64
    }
}

const _: () = assert!(
    core::mem::size_of::<PhoenixTimingRecord>() <= crate::SSR_METRICS_SIZE,
    "PhoenixTimingRecord must fit inside the 16 KiB SSR_METRICS region"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsc_conversion_is_exact_and_overflow_safe() {
        // 3.0 GHz = 3_000_000 kHz -> 3_000 ticks = 1 us; 3_000_000 ticks = 1000 us (1 ms)
        assert_eq!(tsc_delta_to_us(100, 3_000_100, 3_000_000), 1_000);
        // Large delta near u64::MAX must not overflow
        let big = tsc_delta_to_us(0, u64::MAX / 2, 3_000_000);
        assert!(big > 0);
        assert_eq!(tsc_delta_to_us(500, 400, 3_000_000), 0);
        assert_eq!(tsc_delta_to_us(0, 10_000, 0), 0);
    }

    #[test]
    fn record_compute_deltas_populates_s0_and_sla_windows() {
        let mut rec = PhoenixTimingRecord::zeroed();
        rec.magic = PHOENIX_METRICS_MAGIC;
        rec.cycle_id = 1;
        rec.trigger_reason = PHOENIX_REASON_FORCE_SYSCALL;
        rec.tsc_khz = 2_000_000; // 2 GHz -> 2000 ticks/us
        rec.t0_freeze_req = 10_000;
        rec.t1_freeze_ack_all = 10_000 + 2_000 * 500;       // +500 us
        rec.t2_iommu_and_forge = 10_000 + 2_000 * 2_500;    // +2000 us forge
        rec.t3_kernel_b_active = 10_000 + 2_000 * 3_000;    // 3000 us S0 total
        rec.t4_crypto_reseeded = 10_000 + 2_000 * 4_500;    // +1500 us crypto
        rec.t5_strate1_ready = 10_000 + 2_000 * 12_000;     // 12000 us SLA
        rec.t6_strate2_ready = 10_000 + 2_000 * 25_000;     // 25000 us full
        rec.compute_deltas();
        assert_eq!(rec.delta_s0_freeze_us, 500);
        assert_eq!(rec.delta_s0_forge_us, 2_000);
        assert_eq!(rec.delta_s0_us, 3_000);
        assert_eq!(rec.delta_crypto_us, 1_500);
        assert_eq!(rec.delta_sla_us, 12_000);
        assert_eq!(rec.delta_full_us, 25_000);
        assert!(rec.is_valid());
        assert!(!rec.is_degraded());
        assert_eq!(rec.reason_str(), "FORCE");
    }
}
