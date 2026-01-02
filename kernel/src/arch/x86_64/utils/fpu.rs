//! Lazy FPU/SSE State Management
//!
//! Optimizes context switches by only saving/restoring FPU state when actually used.
//! Saves ~50-100 cycles per context switch for threads that don't use floating point.
//!
//! Technique:
//! 1. Set CR0.TS (Task Switched) on every context switch
//! 2. First FPU instruction triggers #NM (Device Not Available)
//! 3. #NM handler saves old FPU state, restores new FPU state, clears CR0.TS
//! 4. Thread continues with FPU enabled
//!
//! This is what Linux and other modern OSes do for maximum performance.

use core::arch::asm;

/// FPU state structure (512 bytes aligned to 16-byte boundary)
/// Uses FXSAVE/FXRSTOR format (faster than FSAVE/FRSTOR)
#[repr(C, align(16))]
#[derive(Debug)]
pub struct FpuState {
    pub data: [u8; 512],
}

impl FpuState {
    pub const fn new() -> Self {
        Self { data: [0; 512] }
    }
}

/// Per-CPU FPU ownership tracking
static mut LAST_FPU_THREAD: Option<usize> = None; // TID of thread that owns FPU

/// Initialize FPU subsystem
pub fn init() {
    unsafe {
        // Enable FPU (CR0.MP = 1, CR0.EM = 0)
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack));
        cr0 |= 1 << 1;  // CR0.MP (Monitor Coprocessor)
        cr0 &= !(1 << 2); // CR0.EM (Emulation) = 0
        asm!("mov cr0, {}", in(reg) cr0, options(nostack));

        // Enable FXSAVE/FXRSTOR (CR4.OSFXSR = 1)
        // Enable SSE (CR4.OSXMMEXCPT = 1)
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
        cr4 |= (1 << 9);  // CR4.OSFXSR
        cr4 |= (1 << 10); // CR4.OSXMMEXCPT
        asm!("mov cr4, {}", in(reg) cr4, options(nostack));

        // Set CR0.TS to trigger #NM on first FPU use
        set_task_switched();
    }
}

/// Set CR0.TS bit (called on every context switch)
#[inline(always)]
pub fn set_task_switched() {
    unsafe {
        asm!(
            "mov rax, cr0",
            "or rax, 0x8",     // CR0.TS = bit 3
            "mov cr0, rax",
            out("rax") _,
            options(nostack, nomem)
        );
    }
}

/// Clear CR0.TS bit (called in #NM handler)
#[inline(always)]
pub fn clear_task_switched() {
    unsafe {
        asm!(
            "clts",  // Dedicated instruction to clear CR0.TS
            options(nostack, nomem)
        );
    }
}

/// Save FPU state using FXSAVE (fast)
#[inline(always)]
pub unsafe fn save(state: &mut FpuState) {
    asm!(
        "fxsave [{}]",
        in(reg) state.data.as_mut_ptr(),
        options(nostack)
    );
}

/// Restore FPU state using FXRSTOR (fast)
#[inline(always)]
pub unsafe fn restore(state: &FpuState) {
    asm!(
        "fxrstor [{}]",
        in(reg) state.data.as_ptr(),
        options(nostack)
    );
}

/// Handle #NM exception (Device Not Available)
/// This is called when a thread tries to use FPU after context switch
pub unsafe fn handle_device_not_available(current_tid: usize, fpu_state: &mut FpuState) {
    // Save FPU state of previous owner (if any)
    if let Some(last_tid) = LAST_FPU_THREAD {
        if last_tid != current_tid {
            // TODO: Get FPU state pointer of thread `last_tid` and save
            // For now, we just save to a dummy location
            // save(&mut DUMMY_FPU_STATE);
        }
    }

    // Restore FPU state of current thread
    restore(fpu_state);

    // Update ownership
    LAST_FPU_THREAD = Some(current_tid);

    // Clear CR0.TS to allow FPU instructions
    clear_task_switched();
}

/// Initialize FPU state to default (clean slate for new threads)
pub unsafe fn init_state(state: &mut FpuState) {
    // Initialize with FNINIT, then save
    asm!("fninit", options(nostack, nomem));
    save(state);
}
