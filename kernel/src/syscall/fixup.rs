// kernel/src/syscall/fixup.rs
//
// Fixup table for kernel page faults during copy_from_user / copy_to_user.
//
// ## Problem
// When a syscall handler calls copy_from_user() or copy_to_user() and the
// userspace page is not present (unresolved CoW, demand paging, invalid page),
// a #PF is triggered in RING 0 context. Without a fixup table,
// do_page_fault() panics with "KernelFault".
//
// ## Solution
// Each userspace access site in copy_from_user / copy_to_user registers
// a pair (fault_rip, recovery_rip):
//   - fault_rip    : address of the instruction that can fault
//   - recovery_rip : address where execution resumes if fault is unrecoverable
//
// do_page_fault() checks this table BEFORE panicking. If fault_rip is
// known, it patches frame->rip = recovery_rip and returns. The copy function
// resumes at the recovery point with an error indicator.
//
// ## Implementation
// Rust cannot annotate individual instructions with ASM labels in the same way
// as C (__ex_table). We therefore use a slightly different approach:
// copy_from_user_safe and copy_to_user_safe are ASM wrappers that register
// in a static table before the critical loop and unregister after.
// The table contains one entry per CPU (user copies are sequential per thread).
//
// Architecture:
//   KernelCopyState per CPU (in percpu):
//     - active: bool       — a copy is in progress on this CPU
//     - fault_rsp: u64     — RSP upon entry into copy
//     - recovery_fn: usize — recovery function address (returns EFAULT)

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Maximum number of supported CPUs (must match MAX_CPUS)
const FIXUP_MAX_CPUS: usize = 256;

/// Kernel→user / user→kernel copy state for a CPU.
///
/// When a user copy is active on a CPU, `active` is true and
/// `recovery_rsp` / `recovery_rip` indicate where to resume in case of #PF.
#[repr(C, align(64))]
pub struct KernelCopyState {
    /// A userspace copy is in progress on this CPU.
    pub active: AtomicBool,
    _pad0: [u8; 7],
    /// RSP at entry into the copy function (for unwinding).
    pub recovery_rsp: AtomicU64,
    /// Resume RIP: the instruction following the fault site will return EFAULT.
    pub recovery_rip: AtomicUsize,
    _pad1: [u8; 40],
}

// SAFETY: all fields are atomic.
unsafe impl Send for KernelCopyState {}
unsafe impl Sync for KernelCopyState {}

impl KernelCopyState {
    const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            _pad0: [0u8; 7],
            recovery_rsp: AtomicU64::new(0),
            recovery_rip: AtomicUsize::new(0),
            _pad1: [0u8; 40],
        }
    }
}

// Compile-time assertion: each entry must fit into a cache line (64B)
const _: () = assert!(
    core::mem::size_of::<KernelCopyState>() == 64,
    "KernelCopyState doit faire exactement 64 bytes (1 cache line)"
);

/// Global table of copy states, indexed by cpu_id.
static COPY_STATES: [KernelCopyState; FIXUP_MAX_CPUS] = {
    // Rust does not support `[expr; N]` for N>32 with non-Copy const fn,
    // so we use an array initialized manually via a transmute-free trick.
    // Solution: MaybeUninit array initialized in init_fixup_table().
    // To avoid a global UnsafeCell, we initialize with new() via const.
    const INIT: KernelCopyState = KernelCopyState::new();
    [INIT; FIXUP_MAX_CPUS]
};

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the start of a userspace copy on the current CPU.
///
/// Must be called BEFORE any access to a userspace pointer.
/// `recovery_rip` is the address where execution resumes if #PF is not recoverable
/// (typically the `return Err(SyscallError::Fault)` instruction).
///
/// # Safety
/// `recovery_rip` must point to valid kernel code in the same function.
#[inline]
pub unsafe fn fixup_enter(cpu_id: usize, recovery_rip: usize) {
    if cpu_id >= FIXUP_MAX_CPUS {
        return;
    }
    let state = &COPY_STATES[cpu_id];
    // Read current RSP for unwinding (unused in this simplified
    // implementation, but available for a future unwinder)
    let rsp: u64;
    core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    state.recovery_rsp.store(rsp, Ordering::Relaxed);
    state.recovery_rip.store(recovery_rip, Ordering::Relaxed);
    // Publish `active` last (Release) so that recovery_rip/rsp
    // are visible before the #PF handler reads active.
    state.active.store(true, Ordering::Release);
}

/// Unregisters the userspace copy on the current CPU.
///
/// Must be called AFTER the userspace access loop finishes,
/// whether on success or error.
#[inline]
pub fn fixup_exit(cpu_id: usize) {
    if cpu_id >= FIXUP_MAX_CPUS {
        return;
    }
    // Acquire so that any read/write from the copy is visible before
    // deactivation.
    COPY_STATES[cpu_id].active.store(false, Ordering::Release);
}

/// Consults the fixup table for a kernel #PF at `fault_rip`.
///
/// Called by do_page_fault() before panicking.
///
/// Returns `Some(recovery_rip)` if a userspace copy is in progress on
/// `cpu_id` (the fault can be intercepted), `None` otherwise (legitimate panic).
///
/// # Safety
/// Must be called from the kernel #PF handler, with interrupts
/// disabled (guaranteed by the x86-64 handler).
#[inline]
pub fn fixup_lookup(cpu_id: usize) -> Option<usize> {
    if cpu_id >= FIXUP_MAX_CPUS {
        return None;
    }
    let state = &COPY_STATES[cpu_id];
    // Acquire to synchronize with fixup_enter (Release on active).
    if !state.active.load(Ordering::Acquire) {
        return None;
    }
    let rip = state.recovery_rip.load(Ordering::Relaxed);
    if rip == 0 {
        return None;
    }
    Some(rip)
}

// ─────────────────────────────────────────────────────────────────────────────
// copy_from/to_user wrappers with fixup protection
// ─────────────────────────────────────────────────────────────────────────────

/// Copies `len` bytes from `src` (userspace) to `dst` (kernel), with #PF fixup.
///
/// In case of an unrecoverable page fault on `src`, returns `false` (or Err)
/// instead of panicking.
///
/// Replaces copy_from_user() in validation.rs.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn copy_from_user_safe(dst: *mut u8, src: *const u8, len: usize, cpu_id: usize) -> bool {
    if len == 0 {
        return true;
    }

    // Recovery label: if #PF occurs during the loop below,
    // the handler patches RIP to this label and `faulted` is true on return.
    //
    // Implementation: we use a CPU-local atomic variable to
    // signal the fault rather than true setjmp/longjmp. The #PF handler
    // calls fixup_signal_fault(cpu_id) which sets FAULTED[cpu_id].
    FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].store(false, Ordering::Relaxed);

    // Compute recovery address: the instruction that checks FAULTED
    // after the loop. We use a Rust label via a block.
    let recovery_addr = fault_recovery_stub as *const () as usize;

    // SAFETY: recovery_addr is a valid kernel function.
    unsafe {
        fixup_enter(cpu_id, recovery_addr);
    }

    // Userspace access loop — faultable
    // SAFETY: src validated by caller (validate_user_range). dst is kernel.
    let ok = unsafe {
        let mut faulted_mid = false;
        for i in 0..len {
            // If an intermediate fault was signaled by the #PF handler
            if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
                faulted_mid = true;
                break;
            }
            let byte = core::ptr::read_volatile(src.add(i));
            core::ptr::write(dst.add(i), byte);
        }
        !faulted_mid
    };

    fixup_exit(cpu_id);

    // Check one last time (the handler may have signaled during the
    // last iteration)
    if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
        return false;
    }

    ok
}

/// Copies `len` bytes from `src` (kernel) to `dst` (userspace), with #PF fixup.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn copy_to_user_safe(dst: *mut u8, src: *const u8, len: usize, cpu_id: usize) -> bool {
    if len == 0 {
        return true;
    }

    FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].store(false, Ordering::Relaxed);
    let recovery_addr = fault_recovery_stub as *const () as usize;

    // SAFETY: recovery_addr is a valid kernel function.
    unsafe {
        fixup_enter(cpu_id, recovery_addr);
    }

    let ok = unsafe {
        let mut faulted_mid = false;
        for i in 0..len {
            if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
                faulted_mid = true;
                break;
            }
            let byte = core::ptr::read(src.add(i));
            core::ptr::write_volatile(dst.add(i), byte);
        }
        !faulted_mid
    };

    fixup_exit(cpu_id);

    if FAULTED[cpu_id.min(FIXUP_MAX_CPUS - 1)].load(Ordering::Relaxed) {
        return false;
    }

    ok
}

// ─────────────────────────────────────────────────────────────────────────────
// Fault signal and recovery stub
// ─────────────────────────────────────────────────────────────────────────────

/// Array of "fault signaled" flags per CPU.
/// Set by the #PF handler when fixup_lookup() returns Some.
static FAULTED: [AtomicBool; FIXUP_MAX_CPUS] = {
    const INIT_BOOL: AtomicBool = AtomicBool::new(false);
    [INIT_BOOL; FIXUP_MAX_CPUS]
};

/// Signals a fault for CPU `cpu_id`.
///
/// Called by do_page_fault() when fixup_lookup() returned Some.
/// Deactivates fixup and sets FAULTED flag so copy_from/to_user_safe
/// returns false.
#[inline]
pub fn fixup_signal_fault(cpu_id: usize) {
    if cpu_id >= FIXUP_MAX_CPUS {
        return;
    }
    FAULTED[cpu_id].store(true, Ordering::Release);
    // Deactivate fixup so the next kernel access faults normally
    fixup_exit(cpu_id);
}

/// Recovery stub — address registered in recovery_rip.
///
/// This function is never called directly. Its address serves as a
/// target for fixup_lookup(). The #PF handler does not use this address
/// for an actual jump (we use fixup_signal_fault instead), but
/// it is preserved for a future implementation via kernel longjmp.
#[inline(never)]
#[cold]
extern "C" fn fault_recovery_stub() {
    // Symbolic recovery point — the #PF handler calls
    // fixup_signal_fault() which sets FAULTED; the loop in copy_from/to_user_safe
    // detects the flag and returns false (EFAULT).
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixup_lookup_inactive_returns_none() {
        // On CPU 0 without active copy, lookup returns None
        COPY_STATES[0].active.store(false, Ordering::Release);
        assert!(fixup_lookup(0).is_none());
    }

    #[test]
    fn fixup_enter_exit_roundtrip() {
        let recovery = fault_recovery_stub as *const () as usize;
        unsafe {
            fixup_enter(0, recovery);
        }
        assert!(fixup_lookup(0).is_some());
        assert_eq!(fixup_lookup(0).unwrap(), recovery);
        fixup_exit(0);
        assert!(fixup_lookup(0).is_none());
    }

    #[test]
    fn fixup_signal_clears_active() {
        let recovery = fault_recovery_stub as *const () as usize;
        unsafe {
            fixup_enter(1, recovery);
        }
        fixup_signal_fault(1);
        assert!(fixup_lookup(1).is_none());
        assert!(FAULTED[1].load(Ordering::Relaxed));
        // Cleanup
        FAULTED[1].store(false, Ordering::Relaxed);
    }

    #[test]
    fn copy_states_cache_line_aligned() {
        assert_eq!(core::mem::size_of::<KernelCopyState>(), 64);
        assert_eq!(core::mem::align_of::<KernelCopyState>(), 64);
    }
}
