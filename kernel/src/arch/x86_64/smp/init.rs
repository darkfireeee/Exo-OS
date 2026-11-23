//! # arch/x86_64/smp/init.rs — Application Processor Startup
//!
//! Implements the SMP boot sequence:
//! 1. BSP sends INIT IPI to each AP
//! 2. BSP waits 10ms (ACPI spec)
//! 3. BSP sends STARTUP IPI × 2 (with trampoline address)
//! 4. AP executes trampoline (real mode → 64-bit)
//! 5. AP calls `ap_entry()` to initialize its structures and join the scheduler
//!
//! ## Trampoline
//! The trampoline assembly code resides in `boot/trampoline_asm.rs`.
//! It is copied to address 0x6000 (trampoline page = 6).

use super::super::acpi::madt;
use super::super::apic::ipi;
use super::super::cpu::tsc;
use super::percpu;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Physical address of AP trampoline (must be < 1 MiB, page-aligned)
pub const TRAMPOLINE_PHYS: u64 = 0x6000;
pub const TRAMPOLINE_PAGE: u8 = 6; // SIPI vector = address / 4096

/// ACPI SMP delays (Intel MP Spec Section B.4)
const INIT_IPI_DELAY_MS: u64 = 10; // 10ms after INIT IPI
const STARTUP_IPI_DELAY_MS: u64 = 1; // 1ms between the two SIPIs
const AP_STARTUP_TIMEOUT_MS: u64 = 100; // timeout waiting for each AP

static SMP_BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// Returns the number of logical CPUs online
pub fn smp_cpu_count() -> u32 {
    percpu::cpu_count()
}

#[inline]
pub fn smp_boot_complete() -> bool {
    SMP_BOOT_DONE.load(Ordering::Acquire)
}

// ── BSP ↔ AP Handshake ───────────────────────────────────────────────────────

/// Handshake area within the trampoline (offset 0x10 from trampoline start)
///
/// The AP writes `AP_ALIVE_MAGIC` here to report its presence to the BSP.
const AP_ALIVE_MAGIC: u32 = 0xA1_1A_1A_A1;
const HANDSHAKE_OFFSET: u64 = 0x10; // offset within trampoline
const AP_STACK_SIZE: usize = 32 * 1024;
const MAX_BOOT_APS: usize = 16;

#[repr(C, align(16))]
struct ApBootStack([u8; AP_STACK_SIZE]);

static mut AP_BOOT_STACKS: [ApBootStack; MAX_BOOT_APS] =
    [const { ApBootStack([0u8; AP_STACK_SIZE]) }; MAX_BOOT_APS];

fn write_trampoline_u32(offset: u64, val: u32) {
    // SAFETY: identity-mapped trampoline, validated offset
    unsafe {
        core::ptr::write_volatile((TRAMPOLINE_PHYS + offset) as *mut u32, val);
    }
}

fn write_trampoline_u64(offset: u64, val: u64) {
    // SAFETY: identity-mapped trampoline, validated offset
    unsafe {
        core::ptr::write_volatile((TRAMPOLINE_PHYS + offset) as *mut u64, val);
    }
}

fn read_trampoline_u32(offset: u64) -> u32 {
    // SAFETY: identity-mapped trampoline
    unsafe { core::ptr::read_volatile((TRAMPOLINE_PHYS + offset) as *const u32) }
}

// ── AP Startup ────────────────────────────────────────────────────────────────

/// Starts all APs listed in MADT
///
/// Called by BSP from `boot::early_init` after memory initialization.
/// Each started AP increments `ONLINE_CPU_COUNT` and calls `percpu::init_percpu_for_ap`.
pub fn smp_boot_aps(madt_info: &madt::MadtInfo, bsp_lapic_id: u32) {
    let n = madt_info.cpu_count;
    let apic_ids = &madt_info.apic_ids;
    let mut next_cpu_id = 1u32;

    for &apic_id in &apic_ids[..(n as usize)] {
        if apic_id == bsp_lapic_id {
            continue;
        } // skip BSP
        if (next_cpu_id as usize) >= MAX_BOOT_APS {
            break;
        }
        if boot_ap(next_cpu_id, apic_id as u8) {
            next_cpu_id += 1;
        }
    }

    SMP_BOOT_DONE.store(true, Ordering::Release);
}

/// INIT → SIPI → SIPI sequence for an AP
fn boot_ap(cpu_id: u32, dest_apic_id: u8) -> bool {
    let stack_top = unsafe {
        let base = core::ptr::addr_of!(AP_BOOT_STACKS[cpu_id as usize].0) as u64;
        (base + AP_STACK_SIZE as u64) & !0xFu64
    };

    // 1. Prepare handshake & parameters in trampoline page (0x6010..0x6038)
    write_trampoline_u32(HANDSHAKE_OFFSET, 0);
    write_trampoline_u64(0x18, stack_top);
    write_trampoline_u32(0x28, cpu_id);
    write_trampoline_u32(0x2C, dest_apic_id as u32);
    write_trampoline_u64(0x30, ap_entry as *const () as u64);
    core::sync::atomic::fence(Ordering::SeqCst);

    // 2. INIT IPI
    ipi::send_init_ipi(dest_apic_id);
    tsc::tsc_delay_ms(INIT_IPI_DELAY_MS);

    // 3. First SIPI
    ipi::send_startup_ipi(dest_apic_id, TRAMPOLINE_PAGE);
    tsc::tsc_delay_ms(STARTUP_IPI_DELAY_MS);

    // 4. Second SIPI (certain chipsets require two SIPIs)
    ipi::send_startup_ipi(dest_apic_id, TRAMPOLINE_PAGE);

    // 5. Wait for AP to complete ap_entry initialization and report its presence
    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(AP_STARTUP_TIMEOUT_MS);
    loop {
        let sig = read_trampoline_u32(HANDSHAKE_OFFSET);
        if sig == AP_ALIVE_MAGIC {
            core::sync::atomic::fence(Ordering::Acquire);
            return true;
        }
        if tsc::read_tsc() > deadline {
            // Non-responsive AP: ignore (may be absent or disabled in MADT)
            return false;
        }
        core::hint::spin_loop();
    }
}

// ── AP Entry Point ────────────────────────────────────────────────────────────

/// Rust entry point for each AP
///
/// Called from assembly trampoline after transition to 64-bit mode.
/// At this stage: paging enabled, dedicated stack, GDT/IDT already set up from BSP.
///
/// # Safety
/// Called strictly from trampoline, in single-threaded context for this CPU.
#[no_mangle]
pub unsafe extern "C" fn ap_entry(cpu_id: u32, lapic_id: u32, kernel_stack_top: u64) -> ! {
    while !super::super::arch_initialized() {
        core::hint::spin_loop();
    }

    // 1. GDT per-CPU + TSS FIRST (note: init_gdt_for_cpu reloads GS selector = 0,
    //    which clears MSR_GS_BASE, so init_percpu_for_ap MUST run after this!).
    super::super::gdt::init_gdt_for_cpu(cpu_id as usize, kernel_stack_top);

    // 2. Per-CPU data (GS_BASE)
    percpu::init_percpu_for_ap(cpu_id, kernel_stack_top, lapic_id);

    // 3. IDT (shared — just LIDT)
    super::super::idt::load_idt();

    // 3b. SYSCALL/SYSRET MSRs (STAR/LSTAR/SFMASK) on each AP.
    // Must be done before STI to avoid an AP CPU running without configured syscall path.
    super::super::syscall::init_syscall();

    // 4. LAPIC AP
    super::super::apic::init_ap_local_apic();

    // 5. Basic TSC calibration (invariant TSC → no recalibration)
    tsc::init_tsc(cpu_id);

    // 6. FPU
    super::super::cpu::fpu::init_fpu_for_cpu();

    while !crate::security::is_security_ready() {
        core::hint::spin_loop();
    }

    // 6b. Publish a bootstrap idle TCB for this AP after SECURITY_READY.
    let _ = crate::scheduler::core::publish_current_boot_idle(cpu_id, kernel_stack_top);

    // 6c. Local AP scheduler initialization (lazy FPU + tick counters).
    crate::scheduler::init_ap(cpu_id);

    // 7. Spectre mitigations (IBRS/STIBP/SSBD + KPTI) on this AP.
    crate::arch::x86_64::spectre::apply_mitigations_ap();

    // 8. Signal BSP that this AP is online and ready to handle interrupts/IPIs
    write_trampoline_u32(HANDSHAKE_OFFSET, AP_ALIVE_MAGIC);
    core::sync::atomic::fence(Ordering::Release);

    // 9. Enable interrupts and enter IPI-servicing idle loop
    // SAFETY: all structures are initialized on this AP
    core::arch::asm!("sti", options(nostack, nomem));

    loop {
        core::arch::asm!("hlt", options(nostack, nomem));
    }
}
