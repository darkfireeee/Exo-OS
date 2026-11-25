//! # main.rs — Exo-OS Kernel Entry Point
//!
//! Boot assembly (`_start`, `_start_uefi`, `_start64`, `.multiboot2`, `.boot_stack`)
//! resides in `exo_os_kernel::arch::x86_64::boot::multiboot2_trampoline`.
//! Panic and OOM handlers reside in `exo_os_kernel::boot_seq`.

#![no_std]
#![no_main]
#![allow(unexpected_cfgs)]

use exo_os_kernel as kernel;
use kernel::arch::x86_64::debug_e9_byte;

/// `kernel_main` — first Rust function executed on the BSP.
///
/// # Safety
/// - Called from `_start64` (ASM) exactly once on the BSP
/// - Interrupts disabled (EFLAGS.IF = 0)
/// - Valid BSP stack (`_exo_boot_stack_top`)
/// - 64-bit long mode active, minimal bootloader GDT loaded
#[no_mangle]
pub unsafe extern "C" fn kernel_main(
    mb2_magic: u32,
    mb2_info: u64,
    rsdp_phys: u64,
) -> ! {
    #[cfg(all(target_os = "none", not(test)))]
    kernel::arch::x86_64::boot::multiboot2_trampoline::link_boot_trampoline_anchor();

    // Debug: kernel_main start marker on port 0xE9 ('K' = 0x4B)
    debug_e9_byte(b'K');

    kernel::arch::x86_64::boot_display::boot_screen();

    // ── Phase 1: Architecture (GDT, IDT, TSS, per-CPU, TSC, FPU, ACPI, APIC, SYSCALL, SMP)
    let boot_info = kernel::arch_boot_init(mb2_magic, mb2_info, rsdp_phys);
    debug_e9_byte(b'A');

    debug_e9_byte(if boot_info.framebuffer_phys_addr != 0 && boot_info.framebuffer_size_bytes != 0 {
        b'P'
    } else {
        b'p'
    });

    let framebuffer_attached = kernel::arch::x86_64::boot_display::attach_framebuffer(&boot_info);
    debug_e9_byte(if framebuffer_attached { b'V' } else { b'v' });

    kernel::arch::x86_64::boot_display::stage_ok("ARCH");
    debug_e9_byte(if kernel::arch::x86_64::framebuffer_early::is_active() {
        b'W'
    } else {
        b'w'
    });

    // ── Phases 2–7: Layer initialization (memory, scheduler, process, security, ipc, fs)
    kernel::kernel_init(boot_info.cpu_count as usize);
    debug_e9_byte(b'I');

    let userspace_status = kernel::userspace_boot::boot_userspace();

    kernel::arch::x86_64::boot_display::boot_complete();
    match userspace_status {
        kernel::userspace_boot::BootUserspaceStatus::Started { .. } => {
            kernel::arch::x86_64::boot_display::userspace_status(
                "USERSPACE HANDOFF: INIT PID 1",
                "/sbin/exo-init-server charge depuis le disque ExoFS racine.",
                "/lib/ld-exo.so assure le handoff avant les services Ring1.",
            );
            debug_e9_byte(b'U');
        }
        kernel::userspace_boot::BootUserspaceStatus::AlreadyRunning => {
            kernel::arch::x86_64::boot_display::userspace_status(
                "USERSPACE HANDOFF: INIT DEJA ACTIF",
                "PID 1 est deja present dans la registry processus.",
                "Scheduler actif: attente des services Ring1.",
            );
        }
        kernel::userspace_boot::BootUserspaceStatus::InitLoadFailed(_) => {
            kernel::arch::x86_64::boot_display::userspace_status(
                "USERSPACE HANDOFF: INIT ELF REFUSE",
                "Le chargeur ELF n'a pas pu lire /sbin/exo-init-server depuis ExoFS.",
                "Verifier target/qemu/exofs-root.img et /lib/ld-exo.so.",
            );
        }
        kernel::userspace_boot::BootUserspaceStatus::InitCreateFailed(_) => {
            kernel::arch::x86_64::boot_display::userspace_status(
                "USERSPACE HANDOFF: PID 1 NON CREE",
                "L'image init est chargee mais le PCB/TCB PID 1 n'a pas ete publie.",
                "Le scheduler reste en idle interruptible pour diagnostic.",
            );
        }
    }

    for &b in b"\nOK\n" {
        debug_e9_byte(b);
    }

    #[cfg(exophoenix_resurrection_test)]
    kernel::exophoenix::resurrection::trigger_self_destruct();

    #[cfg(not(exophoenix_resurrection_test))]
    kernel::arch::x86_64::run_scheduler_idle()
}
