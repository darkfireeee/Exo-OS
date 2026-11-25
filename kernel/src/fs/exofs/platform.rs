//! # fs/exofs/platform.rs — Portable Platform Abstraction Seam for ExoFS
//!
//! Defines the trait boundary separating ExoFS core logic from Ring 0 hardware
//! details (diagnostic trace port, monotonic clock, hardware RNG, audit sink,
//! and block storage backend).
//!
//! Concrete x86_64 implementations live outside `fs/exofs/` in `crate::fs::exofs_hal`.

pub use crate::fs::exofs::recovery::boot_recovery::BlockDevice as BlockBackend;

/// Early boot diagnostic byte sink (e.g. QEMU port 0xE9 in `exofs_hal`).
pub trait BootTrace: Sync {
    fn write_byte(&self, byte: u8);

    #[inline(always)]
    fn write_pair(&self, prefix: u8, tag: u8) {
        self.write_byte(prefix);
        self.write_byte(tag);
    }
}

/// Monotonic relative tick source for GC age, log rotation, and timestamps.
pub trait MonotonicClock: Sync {
    fn monotonic_ticks(&self) -> u64;
}

/// Optional direct hardware entropy source (e.g. CPU RDRAND).
pub trait EntropySource: Sync {
    fn try_hw_random_u64(&self) -> Option<u64>;
}

/// Kernel security audit forwarding sink.
pub trait AuditSink: Sync {
    fn record_security_denial(&self, actor_uid: u64, object_id: u64);
}

/// Combined platform capability bundle required by ExoFS core.
pub trait ExofsPlatform: BootTrace + MonotonicClock + EntropySource + AuditSink {}

/// Returns the active platform adapter provided by `crate::fs::exofs_hal`.
#[inline(always)]
pub fn platform() -> &'static dyn ExofsPlatform {
    &crate::fs::exofs_hal::x86_platform::X86_EXOFS_PLATFORM
}

/// Emits a 2-byte diagnostic marker through the platform boot trace sink.
#[inline(always)]
pub fn boot_trace_pair(prefix: u8, tag: u8) {
    crate::fs::exofs_hal::x86_platform::boot_trace_pair(prefix, tag);
}

/// Reads the platform monotonic tick counter without touching ISA instructions directly.
#[inline(always)]
pub fn monotonic_ticks() -> u64 {
    crate::fs::exofs_hal::x86_platform::monotonic_ticks()
}

/// Reads one 64-bit hardware random word if available on the current platform.
#[inline(always)]
pub fn try_hw_random_u64() -> Option<u64> {
    crate::fs::exofs_hal::x86_platform::try_hw_random_u64()
}
