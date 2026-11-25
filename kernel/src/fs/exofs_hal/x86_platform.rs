//! x86_64 implementation of ExoFS platform hooks (`BootTrace`, `MonotonicClock`,
//! `EntropySource`, `AuditSink`).
//!
//! All low-level CPU instructions are delegated to `crate::arch::x86_64` —
//! zero inline `asm!` lives in `fs/`.

use crate::fs::exofs::platform::{AuditSink, BootTrace, EntropySource, ExofsPlatform, MonotonicClock};

/// Concrete x86_64 kernel platform adapter for ExoFS.
pub struct X86ExofsPlatform;

/// Static singleton platform implementation used by default in Ring 0.
pub static X86_EXOFS_PLATFORM: X86ExofsPlatform = X86ExofsPlatform;

impl BootTrace for X86ExofsPlatform {
    #[inline(always)]
    fn write_byte(&self, byte: u8) {
        crate::arch::x86_64::debug_e9_byte(byte);
    }
}

impl MonotonicClock for X86ExofsPlatform {
    #[inline(always)]
    fn monotonic_ticks(&self) -> u64 {
        crate::arch::x86_64::cpu::tsc::read_tsc_begin()
    }
}

impl EntropySource for X86ExofsPlatform {
    #[inline(always)]
    fn try_hw_random_u64(&self) -> Option<u64> {
        // SAFETY: `rdrand64_once` safely checks CPU support / CF flag on x86_64.
        unsafe { crate::arch::x86_64::rdrand64_once() }
    }
}

impl AuditSink for X86ExofsPlatform {
    #[inline]
    fn record_security_denial(&self, actor_uid: u64, object_id: u64) {
        crate::security::audit::audit_file_deny(0, 0, actor_uid as u16, object_id as u32);
    }
}

impl ExofsPlatform for X86ExofsPlatform {}

/// Emits a 2-byte diagnostic marker on the platform boot trace sink.
#[inline(always)]
pub fn boot_trace_pair(prefix: u8, tag: u8) {
    X86_EXOFS_PLATFORM.write_pair(prefix, tag);
}

/// Serialized monotonic tick counter (delegated to `arch::x86_64::cpu::tsc`).
#[inline(always)]
pub fn monotonic_ticks() -> u64 {
    X86_EXOFS_PLATFORM.monotonic_ticks()
}

/// Hardware RNG sample when supported by the host CPU (`arch::x86_64::rdrand64_once`).
#[inline(always)]
pub fn try_hw_random_u64() -> Option<u64> {
    X86_EXOFS_PLATFORM.try_hw_random_u64()
}
