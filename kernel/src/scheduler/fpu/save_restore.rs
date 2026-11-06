// kernel/src/scheduler/fpu/save_restore.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// XSAVE/XRSTOR — FPU Save/Restore (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE FPU-01 (V7-C-02 + GI-02):
//   This module manages THE LOGIC — raw ASM instructions (xsave/xrstor/fxsave)
//   reside in arch/x86_64/cpu/fpu.rs. DO NOT duplicate instructions here.
//   Calls to arch/ are via extern "C" FFI.
//
//   MXCSR and x87 FCW are handled EXCLUSIVELY by XSAVE/XRSTOR here.
//   switch_asm.s does NOT touch MXCSR or FCW (V7-C-02 — kernel compiled without SSE).
// ═══════════════════════════════════════════════════════════════════════════════

use super::state::{FpuState, XSAVE_AREA_SIZE};
use crate::scheduler::core::task::ThreadControlBlock;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// FFI to arch/x86_64/cpu/fpu.rs (raw ASM instructions)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_xsave64(area: *mut u8, rfbm: u64);
    fn arch_xrstor64(area: *const u8, rfbm: u64);
    fn arch_fxsave64(area: *mut u8);
    fn arch_fxrstor64(area: *const u8);
    fn arch_has_xsave() -> u8;
    #[allow(dead_code)]
    fn arch_has_avx() -> u8;
}

/// Static cache of XSAVE availability (read once at boot).
static HAS_XSAVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initializes save_restore module (detects XSAVE, calls detect_xsave_size).
/// Called from scheduler::init(), step 3.
pub fn init() {
    super::state::detect_xsave_size();
    // SAFETY: arch_has_xsave() reads CPUID, no side effects.
    let has = unsafe { arch_has_xsave() != 0 };
    HAS_XSAVE.store(has, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Save FPU state of current thread
// ─────────────────────────────────────────────────────────────────────────────

/// Saves FPU state of current thread into its `FpuState`.
///
/// Called from `switch.rs` (step 1 before context_switch_asm) when
/// `prev.fpu_loaded() == true`.
///
/// # Safety
/// - `tcb.fpu_state_ptr` must point to a valid `FpuState`, 64-byte aligned.
/// - FPU must be loaded in physical registers (FPU_LOADED flag).
pub unsafe fn xsave_current(tcb: &mut ThreadControlBlock) {
    let state_ptr = tcb.fpu_state_ptr as *mut FpuState;
    if state_ptr.is_null() {
        // FpuState not yet allocated — nothing to save.
        // BUG-FIX N: mark FPU as not loaded even in this case.
        // Without this store, prev.fpu_loaded remains `true` after context switch,
        // creating visible inconsistency during potential concurrent access
        // (e.g. migration). The flag is corrected by context_switch step-4 when
        // this thread is rescheduled as `next`, but being explicit here is safer.
        tcb.set_fpu_loaded(false);
        return;
    }

    let state = &mut *state_ptr;
    state.refresh_size();
    state.generation = state.generation.wrapping_add(1);

    // SAFETY: state.as_mut_ptr() is 64-byte aligned (guaranteed by FpuState::repr(align(64))).
    // T7-002 FIX: Use actual supported XCR0 mask to avoid hardware #GP.
    if HAS_XSAVE.load(Ordering::Relaxed) {
        let xcr0 = crate::arch::x86_64::cpu::fpu::read_xcr0();
        arch_xsave64(state.as_mut_ptr(), xcr0);
    } else {
        arch_fxsave64(state.as_mut_ptr());
    }

    // Mark FPU as not loaded in TCB.
    tcb.set_fpu_loaded(false);
}

/// Restores FPU state of a thread into physical registers.
///
/// Called from #NM handler (Device Not Available) when
/// `fpu_loaded() == false` and the thread executes an FP instruction.
///
/// # Safety
/// - `tcb.fpu_state_ptr` must point to a valid `FpuState`.
/// - Called with preemption disabled (to avoid migration between XRSTOR and FPU_LOADED).
pub unsafe fn xrstor_for(tcb: &mut ThreadControlBlock) {
    let state_ptr = tcb.fpu_state_ptr as *mut FpuState;
    if state_ptr.is_null() {
        // This thread never saved FPU state → load initial state.
        init_fpu_registers();
        tcb.set_fpu_loaded(true);
        return;
    }

    let state = &*state_ptr;

    // SAFETY: state.as_ptr() is 64-byte aligned. T7-002 FIX: valid XCR0 mask to avoid #GP.
    if HAS_XSAVE.load(Ordering::Relaxed) {
        let xcr0 = crate::arch::x86_64::cpu::fpu::read_xcr0();
        arch_xrstor64(state.as_ptr(), xcr0);
    } else {
        arch_fxrstor64(state.as_ptr());
    }

    tcb.set_fpu_loaded(true);
}

/// Initializes FPU registers to their default state (FINIT + LDMXCSR).
unsafe fn init_fpu_registers() {
    let mxcsr_val: u32 = 0x1F80;
    // SAFETY: standard FPU instructions available on all x86_64.
    core::arch::asm!(
        "fninit",
        "ldmxcsr [{mxcsr}]",
        mxcsr = in(reg) &mxcsr_val,  // Default rounding mode, masks active
        options(nostack),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// FpuState allocation for a thread (first FPU usage)
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates an FpuState area for thread `tcb` upon its first
/// FPU usage (#NM when fpu_state_ptr == NULL).
///
/// Returns `true` if allocation succeeded.
///
/// RULE: Uses kernel heap allocator (global_allocator). CANNOT
/// be called from an IN_RECLAIM context (potential deadlock).
///
/// # Safety
/// Modifies internal FPU pointers of `tcb`. Must not be called concurrently
/// on the same TCB.
pub unsafe fn alloc_fpu_state(tcb: &mut ThreadControlBlock) -> bool {
    use core::alloc::Layout;

    // BUG-FIX F: forbid allocations from an IN_RECLAIM context.
    // Calling heap allocator from IN_RECLAIM can cause deadlock if
    // the allocator itself waits on EmergencyPool (RULE FPU-03).
    if tcb.sched_state.load(Ordering::Acquire) & crate::scheduler::core::task::SCHED_IN_RECLAIM_BIT
        != 0
    {
        return false;
    }

    // Layout: 2688 bytes, 64-byte aligned.
    let layout = Layout::new::<FpuState>();

    // Uses standard Rust API (alloc::alloc::alloc) which delegates to
    // #[global_allocator] KernelAllocator — avoids direct FFI call to
    // __rust_alloc which is unresolved on bare-metal target without alloc usage.
    let ptr = alloc::alloc::alloc(layout);
    if ptr.is_null() {
        return false;
    }

    // Initialize FpuState area to x87/SSE default state.
    let fpu = &mut *(ptr as *mut FpuState);
    core::ptr::write(fpu, FpuState::new());
    fpu.active_size = XSAVE_AREA_SIZE.load(Ordering::Relaxed);

    tcb.fpu_state_ptr = ptr as u64;
    true
}

/// Frees FPU area associated with a terminating thread.
///
/// # Safety
/// TCB must not be schedulable after this call.
pub unsafe fn free_fpu_state(tcb: &mut ThreadControlBlock) {
    use core::alloc::Layout;

    let ptr = tcb.fpu_state_ptr as *mut u8;
    if ptr.is_null() {
        return;
    }
    tcb.fpu_state_ptr = 0;
    tcb.set_fpu_loaded(false);
    let layout = Layout::new::<FpuState>();
    alloc::alloc::dealloc(ptr, layout);
}
