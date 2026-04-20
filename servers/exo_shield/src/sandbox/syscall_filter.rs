//! Syscall filtering for the exo_shield sandbox.
//!
//! Implements a deny-by-default syscall bitmap (256 bits = 4 × u64),
//! per-PID filter profiles (max 16), and violation tracking.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of per-PID filter profiles.
pub const MAX_FILTER_PROFILES: usize = 16;

/// Maximum number of tracked violations per profile.
pub const MAX_VIOLATIONS: usize = 32;

// ---------------------------------------------------------------------------
// Syscall bitmap — 256 bits (4 × u64)
// ---------------------------------------------------------------------------

/// A 256-bit bitmap where each bit corresponds to a syscall number.
/// Bit set → syscall is *allowed*.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SyscallBitmap {
    bits: [u64; 4],
}

impl SyscallBitmap {
    /// Create a new bitmap with all bits clear (deny all).
    pub const fn deny_all() -> Self {
        Self { bits: [0u64; 4] }
    }

    /// Create a new bitmap with all bits set (allow all).
    pub const fn allow_all() -> Self {
        Self {
            bits: [u64::MAX; 4],
        }
    }

    /// Allow a syscall by number.
    pub fn allow(&mut self, nr: u8) {
        let word = (nr as usize) / 64;
        let bit = (nr as u64) % 64;
        self.bits[word] |= 1u64 << bit;
    }

    /// Deny a syscall by number.
    pub fn deny(&mut self, nr: u8) {
        let word = (nr as usize) / 64;
        let bit = (nr as u64) % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// Check whether a syscall is allowed.
    pub fn is_allowed(&self, nr: u8) -> bool {
        let word = (nr as usize) / 64;
        let bit = (nr as u64) % 64;
        self.bits[word] & (1u64 << bit) != 0
    }

    /// Merge another bitmap into this one (union — any bit set in either
    /// becomes allowed).
    pub fn union(&mut self, other: &SyscallBitmap) {
        self.bits[0] |= other.bits[0];
        self.bits[1] |= other.bits[1];
        self.bits[2] |= other.bits[2];
        self.bits[3] |= other.bits[3];
    }

    /// Intersect another bitmap into this one (intersection — only bits set
    /// in *both* remain allowed).
    pub fn intersect(&mut self, other: &SyscallBitmap) {
        self.bits[0] &= other.bits[0];
        self.bits[1] &= other.bits[1];
        self.bits[2] &= other.bits[2];
        self.bits[3] &= other.bits[3];
    }

    /// Count the number of allowed syscalls.
    pub fn count_allowed(&self) -> u32 {
        let mut count: u32 = 0;
        for word in &self.bits {
            count += word.count_ones();
        }
        count
    }

    /// Get raw bits for a specific word.
    pub fn word(&self, idx: usize) -> u64 {
        self.bits[idx.min(3)]
    }
}

// ---------------------------------------------------------------------------
// Violation tracking
// ---------------------------------------------------------------------------

/// Record of a single syscall violation.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SyscallViolation {
    /// PID that committed the violation.
    pid: u32,
    /// Syscall number that was denied.
    syscall_nr: u8,
    /// Timestamp of the violation (arbitrary tick unit).
    timestamp: u64,
    /// Whether the violation was lethal (process killed).
    lethal: bool,
}

impl SyscallViolation {
    pub const fn new(pid: u32, syscall_nr: u8, timestamp: u64, lethal: bool) -> Self {
        Self {
            pid,
            syscall_nr,
            timestamp,
            lethal,
        }
    }

    pub fn pid(&self) -> u32 { self.pid }
    pub fn syscall_nr(&self) -> u8 { self.syscall_nr }
    pub fn timestamp(&self) -> u64 { self.timestamp }
    pub fn is_lethal(&self) -> bool { self.lethal }
}

// ---------------------------------------------------------------------------
// Per-PID filter profile
// ---------------------------------------------------------------------------

/// A filter profile associated with a specific PID.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SyscallFilterProfile {
    /// PID this profile governs.
    pid: u32,
    /// Syscall bitmap (allowed set).
    bitmap: SyscallBitmap,
    /// Violation ring-buffer.
    violations: [SyscallViolation; MAX_VIOLATIONS],
    /// Ring-buffer write index.
    violation_head: u32,
    /// Total number of violations recorded (may exceed MAX_VIOLATIONS).
    total_violations: u64,
    /// Maximum violations before the process is auto-killed.
    violation_threshold: u32,
    /// Whether the profile is in active use.
    active: bool,
}

impl SyscallFilterProfile {
    /// Create a new profile for `pid` with the given bitmap and threshold.
    pub fn new(pid: u32, bitmap: SyscallBitmap, violation_threshold: u32) -> Self {
        Self {
            pid,
            bitmap,
            violations: [SyscallViolation::new(0, 0, 0, false); MAX_VIOLATIONS],
            violation_head: 0,
            total_violations: 0,
            violation_threshold,
            active: true,
        }
    }

    /// Create an inactive (empty) profile slot.
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            bitmap: SyscallBitmap::deny_all(),
            violations: [SyscallViolation::new(0, 0, 0, false); MAX_VIOLATIONS],
            violation_head: 0,
            total_violations: 0,
            violation_threshold: 0,
            active: false,
        }
    }

    /// Check whether a syscall is allowed for this profile's PID.
    pub fn check_syscall(&self, nr: u8) -> bool {
        self.bitmap.is_allowed(nr)
    }

    /// Record a violation.  Returns `true` if the violation threshold has
    /// been reached (suggesting the process should be killed).
    pub fn record_violation(&mut self, nr: u8, timestamp: u64) -> bool {
        let idx = (self.violation_head % MAX_VIOLATIONS as u32) as usize;
        let lethal = self.total_violations + 1 >= self.violation_threshold as u64;
        self.violations[idx] = SyscallViolation::new(self.pid, nr, timestamp, lethal);
        self.violation_head += 1;
        self.total_violations += 1;
        lethal
    }

    /// Allow a syscall in this profile.
    pub fn allow(&mut self, nr: u8) {
        self.bitmap.allow(nr);
    }

    /// Deny a syscall in this profile.
    pub fn deny(&mut self, nr: u8) {
        self.bitmap.deny(nr);
    }

    /// Get the PID.
    pub fn pid(&self) -> u32 { self.pid }

    /// Whether the profile is in active use.
    pub fn is_active(&self) -> bool { self.active }

    /// Total violations recorded.
    pub fn total_violations(&self) -> u64 { self.total_violations }

    /// Current violation threshold.
    pub fn violation_threshold(&self) -> u32 { self.violation_threshold }

    /// Set the violation threshold.
    pub fn set_violation_threshold(&mut self, threshold: u32) {
        self.violation_threshold = threshold;
    }

    /// Get the Nth most recent violation (0 = most recent).
    /// Returns `None` if the index is out of range.
    pub fn get_violation(&self, recency: usize) -> Option<&SyscallViolation> {
        if recency >= self.violation_head as usize || recency >= MAX_VIOLATIONS {
            return None;
        }
        // Map from "recency" to ring-buffer index
        let idx = if self.violation_head as usize <= MAX_VIOLATIONS {
            self.violation_head as usize - 1 - recency
        } else {
            // Wrap-around: head-1 is the most recent
            let head = self.violation_head as usize % MAX_VIOLATIONS;
            let idx = if recency <= head {
                head - recency
            } else {
                MAX_VIOLATIONS - (recency - head)
            };
            idx % MAX_VIOLATIONS
        };
        Some(&self.violations[idx])
    }
}

// ---------------------------------------------------------------------------
// Syscall filter manager
// ---------------------------------------------------------------------------

/// Manages per-PID syscall filter profiles with deny-by-default policy.
pub struct SyscallFilterManager {
    profiles: [SyscallFilterProfile; MAX_FILTER_PROFILES],
    /// Global default bitmap (applied when no per-PID profile exists).
    default_bitmap: SyscallBitmap,
    /// Global violation counter.
    global_violations: AtomicU64,
    /// Generation counter.
    generation: AtomicU32,
}

impl SyscallFilterManager {
    /// Create a new manager with a deny-all default policy.
    pub const fn new() -> Self {
        Self {
            profiles: [SyscallFilterProfile::empty(); MAX_FILTER_PROFILES],
            default_bitmap: SyscallBitmap::deny_all(),
            global_violations: AtomicU64::new(0),
            generation: AtomicU32::new(0),
        }
    }

    /// Register a new per-PID profile.  Returns `false` if the table is
    /// full or a profile for this PID already exists.
    pub fn create_profile(
        &mut self,
        pid: u32,
        bitmap: SyscallBitmap,
        violation_threshold: u32,
    ) -> bool {
        // Check for duplicate
        for p in &self.profiles {
            if p.is_active() && p.pid() == pid {
                return false;
            }
        }
        // Find empty slot
        for p in &mut self.profiles {
            if !p.is_active() {
                *p = SyscallFilterProfile::new(pid, bitmap, violation_threshold);
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Remove a per-PID profile.
    pub fn remove_profile(&mut self, pid: u32) -> bool {
        for p in &mut self.profiles {
            if p.is_active() && p.pid() == pid {
                *p = SyscallFilterProfile::empty();
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Look up the profile for `pid`.
    pub fn get_profile(&self, pid: u32) -> Option<&SyscallFilterProfile> {
        for p in &self.profiles {
            if p.is_active() && p.pid() == pid {
                return Some(p);
            }
        }
        None
    }

    /// Look up the profile for `pid` (mutable).
    pub fn get_profile_mut(&mut self, pid: u32) -> Option<&mut SyscallFilterProfile> {
        for p in &mut self.profiles {
            if p.is_active() && p.pid() == pid {
                return Some(p);
            }
        }
        None
    }

    /// Check whether a syscall is allowed for a given PID.  Falls back to
    /// the global default bitmap if no per-PID profile exists.
    pub fn check_syscall(&self, pid: u32, nr: u8) -> bool {
        if let Some(profile) = self.get_profile(pid) {
            profile.check_syscall(nr)
        } else {
            self.default_bitmap.is_allowed(nr)
        }
    }

    /// Record a violation for a PID.  If no profile exists, one is not
    /// automatically created — the violation is counted globally.
    /// Returns `true` if the PID should be killed (threshold exceeded).
    pub fn record_violation(&mut self, pid: u32, nr: u8, timestamp: u64) -> bool {
        self.global_violations.fetch_add(1, Ordering::Relaxed);
        if let Some(profile) = self.get_profile_mut(pid) {
            profile.record_violation(nr, timestamp)
        } else {
            false
        }
    }

    /// Set the global default bitmap.
    pub fn set_default_bitmap(&mut self, bitmap: SyscallBitmap) {
        self.default_bitmap = bitmap;
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Get the global default bitmap.
    pub fn default_bitmap(&self) -> &SyscallBitmap {
        &self.default_bitmap
    }

    /// Total global violations.
    pub fn global_violations(&self) -> u64 {
        self.global_violations.load(Ordering::Relaxed)
    }

    /// Generation counter.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Count active profiles.
    pub fn active_profile_count(&self) -> u32 {
        let mut count = 0u32;
        for p in &self.profiles {
            if p.is_active() {
                count += 1;
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_deny_all() {
        let bm = SyscallBitmap::deny_all();
        assert!(!bm.is_allowed(0));
        assert!(!bm.is_allowed(255));
        assert_eq!(bm.count_allowed(), 0);
    }

    #[test]
    fn bitmap_allow_deny() {
        let mut bm = SyscallBitmap::deny_all();
        bm.allow(1);  // write
        bm.allow(60); // exit
        assert!(bm.is_allowed(1));
        assert!(bm.is_allowed(60));
        assert!(!bm.is_allowed(0));
        assert_eq!(bm.count_allowed(), 2);
        bm.deny(1);
        assert!(!bm.is_allowed(1));
    }

    #[test]
    fn bitmap_union_intersect() {
        let mut a = SyscallBitmap::deny_all();
        a.allow(1);
        a.allow(2);
        let mut b = SyscallBitmap::deny_all();
        b.allow(2);
        b.allow(3);
        a.union(&b);
        assert!(a.is_allowed(1));
        assert!(a.is_allowed(2));
        assert!(a.is_allowed(3));

        let mut c = SyscallBitmap::allow_all();
        let mut d = SyscallBitmap::deny_all();
        d.allow(1);
        c.intersect(&d);
        assert!(c.is_allowed(1));
        assert!(!c.is_allowed(2));
    }

    #[test]
    fn profile_violation_tracking() {
        let bitmap = SyscallBitmap::deny_all();
        let mut profile = SyscallFilterProfile::new(42, bitmap, 3);
        assert!(!profile.record_violation(0, 100));
        assert!(!profile.record_violation(1, 200));
        assert!(profile.record_violation(2, 300)); // threshold hit
        assert_eq!(profile.total_violations(), 3);
    }

    #[test]
    fn manager_create_check() {
        let mut mgr = SyscallFilterManager::new();
        let mut bm = SyscallBitmap::deny_all();
        bm.allow(0);
        bm.allow(1);
        assert!(mgr.create_profile(100, bm, 10));
        assert!(mgr.check_syscall(100, 0));
        assert!(!mgr.check_syscall(100, 2));
        assert!(!mgr.check_syscall(999, 0)); // no profile, deny-all default
        assert!(mgr.remove_profile(100));
        assert!(!mgr.check_syscall(100, 0)); // removed
    }
}
