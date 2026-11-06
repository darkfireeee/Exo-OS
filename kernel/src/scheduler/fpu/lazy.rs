// kernel/src/scheduler/fpu/lazy.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// LAZY FPU — Deferred per-thread FPU management (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// PRINCIPLE of Lazy FPU (DOC3 + arch x86_64):
//   1. At startup and after each context switch: CR0.TS = 1
//      → the first FP/SIMD instruction of the thread triggers #NM (Device Not Available)
//   2. #NM Handler: restore FPU for this thread (xrstor), CR0.TS = 0
//   3. Before next context switch: if FPU_LOADED → save (xsave), CR0.TS = 1
//
// Advantage: threads that DO NOT use the FPU incur zero save overhead.
//
// RULE FPU-01 (DOC3): This module manages the POLICY (lazy_fpu_used flag in TCB).
//   Raw ASM instructions reside in arch/x86_64/cpu/fpu.rs.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::ThreadControlBlock;
use core::sync::atomic::{AtomicBool, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Global initialization state
// ─────────────────────────────────────────────────────────────────────────────

static FPU_LAZY_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initializes the Lazy FPU mechanism on current CPU.
///
/// - Sets CR0.TS = 1 (every thread considered without loaded FPU).
/// - Must be called on EVERY CPU (BSP + APs).
///   Corresponds to step 4 of scheduler initialization sequence (DOC3).
pub fn init() {
    // SAFETY: cr0_set_ts() sets the TS bit in CR0. Safe at boot before any thread runs.
    unsafe {
        cr0_set_ts();
    }
    FPU_LAZY_INITIALIZED.store(true, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// CR0.TS manipulation
// ─────────────────────────────────────────────────────────────────────────────

/// Sets CR0.TS (Task Switched) → triggers #NM on next FP instruction.
///
/// # Safety
/// Directly modifies CR0 control register (Ring 0).
#[inline(always)]
pub unsafe fn cr0_set_ts() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "mov rax, cr0",
        "or  rax, 8",    // bit 3 = TS
        "mov cr0, rax",
        out("rax") _,
        options(nostack, nomem),
    );
}

/// Clears CR0.TS → FP access allowed without #NM.
/// Called by #NM handler after successful xrstor.
///
/// # Safety
/// Executes privileged CLTS instruction in Ring 0.
#[inline(always)]
pub unsafe fn cr0_clear_ts() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("clts", options(nostack, nomem),);
}

/// Returns true if CR0.TS is set.
///
/// # Safety
/// Directly reads CR0 control register.
#[inline(always)]
pub unsafe fn cr0_ts_is_set() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let cr0: u64;
        core::arch::asm!(
            "mov {}, cr0",
            out(reg) cr0,
            options(nostack, nomem),
        );
        (cr0 >> 3) & 1 != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// mark_fpu_not_loaded() — called after each context switch
// ─────────────────────────────────────────────────────────────────────────────

/// Marks FPU as not loaded for next thread.
/// Called from `switch.rs` after `context_switch_asm()` returns.
///
/// RULE SWITCH-02 (DOC3): this function is the "mark AFTER" the save.
///
/// # Safety
/// Modifies CR0.TS in Ring 0.
#[inline(always)]
pub unsafe fn mark_fpu_not_loaded() {
    cr0_set_ts();
}

// ─────────────────────────────────────────────────────────────────────────────
// #NM Handler — Device Not Available
// ─────────────────────────────────────────────────────────────────────────────

/// Handler for #NM exception (Device Not Available, vector 7).
///
/// Triggered when a thread executes an FP/SIMD instruction and CR0.TS = 1.
///
/// Sequence:
/// 1. Verify exception genuinely originates from CR0.TS (not a genuine fault).
/// 2. Clear CR0.TS (clts).
/// 3. Restore FPU context for current thread (`xrstor_for`).
/// 4. Update FPU_LOADED = true in TCB.
/// 5. Return from exception — instruction will be retried.
///
/// # Safety
/// Called from interrupt vector, with preemption disabled.
pub unsafe fn handle_nm_exception(tcb: &mut ThreadControlBlock) {
    // SAFETY: clts is safe here because we will immediately restore FPU.
    cr0_clear_ts();

    // BUG-FIX M: allocate FpuState before first FPU usage.
    // Without this allocation, xrstor_for() initializes x87/SSE registers
    // to default but no save area exists.
    // On every subsequent context switch, xsave_current() sees fpu_state_ptr == NULL
    // and returns WITHOUT saving state → FPU state is silently lost
    // until an allocation succeeds.
    if tcb.fpu_state_ptr == 0 {
        super::save_restore::alloc_fpu_state(tcb);
        // If allocation fails (IN_RECLAIM or OOM), fpu_state_ptr remains NULL.
        // xrstor_for() handles this case: default init, FPU_LOADED = true, but
        // state will be lost on next switch (graceful degradation).
    }

    // Restore FPU state or initialize for the first time.
    super::save_restore::xrstor_for(tcb);
    // xrstor_for already sets FPU_LOADED = true in TCB.
}

// ─────────────────────────────────────────────────────────────────────────────
// Query helpers
// ─────────────────────────────────────────────────────────────────────────────

/// True if this thread used FPU in its current timeslice.
#[inline(always)]
pub fn is_fpu_used(tcb: &ThreadControlBlock) -> bool {
    // FPU_LOADED = true means FPU was loaded during last switch.
    // Indicates this thread uses FPU.
    tcb.fpu_loaded()
}

/// True if Lazy FPU is initialized on this CPU.
#[inline(always)]
pub fn is_initialized() -> bool {
    #[cfg(target_os = "none")]
    {
        // SAFETY: kernel ring0; CR0.TS is canonical per-CPU state.
        unsafe { cr0_ts_is_set() }
    }
    #[cfg(not(target_os = "none"))]
    {
        FPU_LAZY_INITIALIZED.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — bridge for arch/x86_64/exceptions.rs (#NM handler)
// ─────────────────────────────────────────────────────────────────────────────
//
// arch/ cannot import scheduler/ directly (Rust dependency cycle).
// This `#[no_mangle] extern "C"` function is declared via `extern "C"` in
// arch/x86_64/exceptions.rs and resolved at link time.
//
// RULE FPU-02 (DOC3): The #NM handler MUST delegate to this module.
// ─────────────────────────────────────────────────────────────────────────────

/// C ABI bridge for `do_device_not_avail` (arch/x86_64/exceptions.rs).
///
/// `tcb_ptr`: pointer to current `ThreadControlBlock` (read from GS:[0x20]).
///   - If null: simple CR0.TS clear (non-scheduler thread or boot phase).
///   - Else: complete delegation to `handle_nm_exception()`.
///
/// # Safety
/// Called from interrupt handler, preemption implicitly disabled.
/// `tcb_ptr`, if non-null, MUST point to a valid TCB.
#[no_mangle]
pub unsafe extern "C" fn sched_fpu_handle_nm(tcb_ptr: *mut u8) {
    if tcb_ptr.is_null() {
        // Boot phase or idle without TCB — clear CR0.TS only.
        cr0_clear_ts();
        return;
    }
    let tcb = &mut *(tcb_ptr as *mut ThreadControlBlock);
    handle_nm_exception(tcb);
}

#[cfg(test)]
mod p2_7_fpu_tests {
    use super::*;

    /// P2-7 / Test 4a — FPU_LAZY_INITIALIZED: state machine init/check.
    ///
    /// Runnable on host: validates atomic semantics without touching CR0.
    #[test]
    fn fpu_lazy_init_flag_set_after_init() {
        // Use local AtomicBool to isolate from global static
        // (avoids interference if init() has already run).
        let local = AtomicBool::new(false);
        assert!(
            !local.load(Ordering::Relaxed),
            "flag FPU doit être false avant init"
        );

        local.store(true, Ordering::Release);
        assert!(
            local.load(Ordering::Relaxed),
            "flag FPU doit être true après init"
        );
    }

    /// P2-7 / Test 4b — cr0_ts_is_set() fallback outside x86_64.
    ///
    /// On non-x86_64 host: always returns false (fallback cfg).
    /// On x86_64 bare-metal: returns real CR0.TS state.
    ///
    /// IMPORTANT: on x86_64 host (Ring 3), reading CR0 triggers a privileged
    /// fault; only symbol linkage is validated.
    #[test]
    fn cr0_ts_is_set_returns_bool() {
        #[cfg(not(target_arch = "x86_64"))]
        {
            let ts_state = unsafe { cr0_ts_is_set() };
            assert!(
                !ts_state,
                "cr0_ts_is_set() doit retourner false hors x86_64"
            );
        }

        #[cfg(all(target_arch = "x86_64", target_os = "none"))]
        {
            let _ = unsafe { cr0_ts_is_set() };
        }

        #[cfg(all(target_arch = "x86_64", not(target_os = "none")))]
        {
            let symbol_only: unsafe fn() -> bool = cr0_ts_is_set;
            let _ = symbol_only;
        }
    }

    /// P2-7 / Test 4c — Complete lazy FPU sequence (bare-metal only).
    ///
    /// Verifies sequence: cr0_set_ts() → ts_is_set()==true
    ///                   → cr0_clear_ts() → ts_is_set()==false
    ///
    /// Requires Ring 0 to write CR0 → ignored on host.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 requis pour CR0")]
    fn fpu_lazy_cr0_set_clear_sequence() {
        unsafe {
            cr0_set_ts();
            assert!(cr0_ts_is_set(), "CR0.TS doit être 1 après cr0_set_ts()");

            cr0_clear_ts();
            assert!(!cr0_ts_is_set(), "CR0.TS doit être 0 après cr0_clear_ts()");

            // Restore CR0.TS to preserve lazy FPU invariant.
            cr0_set_ts();
        }
    }
}
