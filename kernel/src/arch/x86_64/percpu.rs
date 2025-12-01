//! Per-CPU Data Structure
//!
//! Holds CPU-local data, accessed via GS segment.

use alloc::boxed::Box;
use core::arch::asm;

#[repr(C)]
pub struct PerCpuData {
    /// Self pointer (GS:0)
    pub self_ptr: *const PerCpuData,
    /// Current thread's kernel stack top (GS:8)
    pub kernel_stack: u64,
    /// Scratch space for user RSP (GS:16)
    pub user_rsp: u64,
    /// CPU ID (GS:24)
    pub cpu_id: u32,
}

static mut PER_CPU_DATA: Option<Box<PerCpuData>> = None;

/// Initialize per-CPU data for the bootstrap CPU
pub fn init(cpu_id: u32) {
    let data = Box::new(PerCpuData {
        self_ptr: core::ptr::null(), // Fixed below
        kernel_stack: 0,
        user_rsp: 0,
        cpu_id,
    });

    // Leak the box to keep it alive forever
    let ptr = Box::into_raw(data);

    unsafe {
        (*ptr).self_ptr = ptr;
        PER_CPU_DATA = Some(Box::from_raw(ptr)); // Keep a reference if needed, but we leaked it?
                                                 // Actually Box::into_raw consumes the box.
                                                 // We just want to store the pointer in GS_BASE.

        // Write GS_BASE MSR (0xC0000101)
        let msr_gs_base: u32 = 0xC0000101;
        let addr = ptr as u64;
        let low = (addr & 0xFFFFFFFF) as u32;
        let high = (addr >> 32) as u32;

        asm!(
            "wrmsr",
            in("ecx") msr_gs_base,
            in("eax") low,
            in("edx") high,
        );

        // Also set KERNEL_GS_BASE (0xC0000102) to the same for now?
        // No, swapgs swaps GS_BASE and KERNEL_GS_BASE.
        // When in user mode, GS_BASE is user's GS (or 0). KERNEL_GS_BASE is kernel's.
        // When syscall enters, we swapgs. So GS_BASE becomes kernel's.
        // So we should write to KERNEL_GS_BASE if we are currently in user mode?
        // But we are in kernel mode now.
        // So we write to GS_BASE.
        // And we should set KERNEL_GS_BASE to 0 (or user GS).

        // Wait, if we are in kernel, GS_BASE is active.
        // When we go to user, we swapgs. So KERNEL_GS_BASE gets the kernel pointer.
        // So we should write to GS_BASE now.
    }
}

/// Set current kernel stack (called by scheduler)
pub fn set_kernel_stack(stack_top: u64) {
    unsafe {
        // Write to GS:8
        asm!(
            "mov gs:[8], {}",
            in(reg) stack_top
        );
    }
}
