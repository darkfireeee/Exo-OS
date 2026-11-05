//! # arch/x86_64 — x86_64 Architecture Implementation
//!
//! Entry point for all x86_64-specific logic.
//! Exports primitives used by higher layers.

pub mod acpi;
pub mod apic;
pub mod boot;
pub mod boot_display;
pub mod cpu;
pub mod exceptions;
pub mod framebuffer_early;
pub mod gdt;
pub mod idt;
pub mod irq;
pub mod memory_iface;
pub mod paging;
pub mod sched_iface; // FFI bridge arch → scheduler (C ABI exports)
pub mod smp;
pub mod spectre;
pub mod syscall;
pub mod terminal;
pub mod time;
pub mod tss;
pub mod vga_early;
pub mod virt;

use core::sync::atomic::{AtomicBool, Ordering};

use super::ArchInfo;
use crate::arch::x86_64::cpu::features::cpu_features_or_none;

// ── Global Constants ─────────────────────────────────────────────────────────

/// Standard page size (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// First canonical address of the kernel high-half.
pub use crate::arch::constants::KERNEL_BASE;

/// Link address of the kernel image.
pub const KERNEL_IMAGE_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Upper limit of addressable physical memory (48 bits PA)
pub const MAX_PHYS_ADDR: u64 = (1u64 << 48) - 1;

/// Page table levels (PML4 → PDPT → PD → PT)
pub const PAGE_TABLE_LEVELS: usize = 4;

// ── Global Architecture State ───────────────────────────────────────────────

static ARCH_INITIALIZED: AtomicBool = AtomicBool::new(false);

#[inline]
pub(crate) fn mark_arch_initialized() {
    ARCH_INITIALIZED.store(true, Ordering::Release);
}

#[inline]
pub fn arch_initialized() -> bool {
    ARCH_INITIALIZED.load(Ordering::Acquire)
}

/// Collects architecture information post-init
pub fn arch_info() -> ArchInfo {
    let features = cpu_features_or_none();
    ArchInfo {
        cpu_count: smp::percpu::cpu_count(),
        has_apic: features.is_some_and(|cpu| cpu.has_apic()),
        has_x2apic: features.is_some_and(|cpu| cpu.has_x2apic()),
        has_acpi: acpi::parser::acpi_available(),
        page_size: PAGE_SIZE,
    }
}

/// Halts the current CPU (irreversible halt)
///
/// # SAFETY
/// Called only from the kernel panic handler or idle loop.
#[inline(never)]
pub fn halt_cpu() -> ! {
    loop {
        // SAFETY: atomic cli+hlt; IF=0 so never awakened — intentional infinite loop.
        unsafe {
            core::arch::asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

/// End-of-boot idle loop with active scheduler and IRQs.
///
/// To be used after `kernel_init()`: the boot idle TCB is published, run
/// queues exist, and waiting kthreads/processes must be able to take
/// the CPU. Unlike `halt_cpu()`, this loop keeps interrupts
/// active and attempts explicit scheduling before each `hlt`.
pub fn run_scheduler_idle() -> ! {
    loop {
        // SAFETY: at this stage the IDT/LAPIC are initialized. Keeping IF=1 allows
        // the timer, PS/2 keyboard, and scheduler IPIs to wake the BSP.
        unsafe {
            enable_interrupts();
        }

        let cpu_id = smp::percpu::current_cpu_id();
        let switched = unsafe {
            crate::scheduler::core::switch::schedule_idle_cpu_once(
                crate::scheduler::core::task::CpuId(cpu_id),
            )
        };
        if switched {
            continue;
        }

        // SAFETY: interruptible idle wait. `sti` covers the case where a caller
        // cleared IF right before entering here; HLT wakes up on the next IRQ.
        unsafe {
            core::arch::asm!("sti", "hlt", options(nomem, nostack));
        }
    }
}

/// Short spin-loop delay (in approximate nanoseconds)
#[inline(always)]
pub fn spin_delay_cycles(cycles: u64) {
    let start = cpu::tsc::read_tsc();
    while cpu::tsc::read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Full memory barrier (mfence)
#[inline(always)]
pub fn memory_barrier() {
    // SAFETY: architectural instruction without hazardous side effects
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
}

/// Read barrier (lfence)
#[inline(always)]
pub fn load_fence() {
    // SAFETY: same as above
    unsafe {
        core::arch::asm!("lfence", options(nostack, preserves_flags));
    }
}

/// Write barrier (sfence)
#[inline(always)]
pub fn store_fence() {
    // SAFETY: same as above
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
}

/// Invalidates a TLB line for the given virtual address
#[inline(always)]
pub fn invlpg(virt_addr: u64) {
    // SAFETY: invlpg is safe — only invalidates a local TLB entry
    unsafe {
        core::arch::asm!(
            "invlpg [{addr}]",
            addr = in(reg) virt_addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Reads CR3 (PML4 Physical address + PCID flags)
#[inline(always)]
pub fn read_cr3() -> u64 {
    let val: u64;
    // SAFETY: reading CR3 — required Ring 0 privilege guaranteed by kernel context
    unsafe {
        core::arch::asm!("mov {val}, cr3", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Writes CR3 (triggers TLB flush if PCID != 0 and bit 63 = 0)
///
/// # SAFETY
/// Caller must ensure that `cr3_val` points to a valid PML4
/// and that the return address remains mapped in the new table.
#[inline(always)]
pub unsafe fn write_cr3(cr3_val: u64) {
    // SAFETY: verification delegated to caller
    unsafe {
        core::arch::asm!("mov cr3, {val}", val = in(reg) cr3_val, options(nostack, nomem));
    }
}

/// Reads CR2 (page fault address)
#[inline(always)]
pub fn read_cr2() -> u64 {
    let val: u64;
    // SAFETY: reading control register — no side effects
    unsafe {
        core::arch::asm!("mov {val}, cr2", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Reads CR4 (advanced control flags)
#[inline(always)]
pub fn read_cr4() -> u64 {
    let val: u64;
    // SAFETY: reading control register
    unsafe {
        core::arch::asm!("mov {val}, cr4", val = out(reg) val, options(nostack, nomem));
    }
    val
}

/// Writes CR4
///
/// # SAFETY
/// Caller is responsible for not disabling critical protections
/// (SMEP, SMAP, PCIDE, etc.) without having properly disabled them beforehand.
#[inline(always)]
pub unsafe fn write_cr4(cr4_val: u64) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!("mov cr4, {val}", val = in(reg) cr4_val, options(nostack, nomem));
    }
}

/// Enables interrupts (sti)
///
/// # SAFETY
/// Caller must ensure that IDT and IST stack are properly configured.
#[inline(always)]
pub unsafe fn enable_interrupts() {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!("sti", options(nostack, nomem));
    }
}

/// Disables interrupts (cli)
#[inline(always)]
pub fn disable_interrupts() {
    // SAFETY: cli is safe in Ring 0 — only modifies IF in RFLAGS
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
    }
}

/// Reads RFLAGS
#[inline(always)]
pub fn read_rflags() -> u64 {
    let flags: u64;
    // SAFETY: reading status register — no side effects
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            flags = out(reg) flags,
            options(nostack)
        );
    }
    flags
}

/// Critical section: disables interrupts and returns previous RFLAGS state
#[must_use]
#[inline(always)]
#[cfg(all(test, not(target_os = "none")))]
pub fn irq_save() -> u64 {
    0
}

/// Critical section: disables interrupts and returns previous RFLAGS state
#[must_use]
#[inline(always)]
#[cfg(not(all(test, not(target_os = "none"))))]
pub fn irq_save() -> u64 {
    let flags = read_rflags();
    disable_interrupts();
    flags
}

/// Restores interrupt state from saved RFLAGS
#[inline(always)]
#[cfg(all(test, not(target_os = "none")))]
pub fn irq_restore(_flags: u64) {}

/// Restores interrupt state from saved RFLAGS
#[inline(always)]
#[cfg(not(all(test, not(target_os = "none"))))]
pub fn irq_restore(flags: u64) {
    // SAFETY: restoring previously saved RFLAGS state — safe
    unsafe {
        core::arch::asm!(
            "push {flags}",
            "popfq",
            flags = in(reg) flags,
            options(nostack)
        );
    }
}

/// Performs an OUT byte to an I/O port
///
/// # SAFETY
/// Caller guarantees that `port` is a valid I/O port and that
/// writing at this moment is coherent with the driver.
#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

/// Performs an IN byte from an I/O port
///
/// # SAFETY
/// Caller guarantees that `port` is a valid I/O port.
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nostack, nomem)
        );
    }
    val
}

/// OUT dword to I/O port
///
/// # SAFETY
/// Same as outb.
#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") val,
            options(nostack, nomem)
        );
    }
}

/// IN dword from I/O port
///
/// # SAFETY
/// Same as inb.
#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    // SAFETY: delegated to caller
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx") port,
            out("eax") val,
            options(nostack, nomem)
        );
    }
    val
}

/// I/O delay (~1-4µs) — write to port 0x80 (POST debug port)
#[inline(always)]
pub fn io_delay() {
    // SAFETY: port 0x80 is the BIOS/POST debug port — harmless write
    unsafe {
        outb(0x80, 0x00);
    }
}

/// CPU pause primitive used by lock-free loops / fast IPC.
#[no_mangle]
pub extern "C" fn arch_cpu_relax() {
    // SAFETY: `pause` is a hint instruction without architectural side effects.
    unsafe {
        core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
    }
}
