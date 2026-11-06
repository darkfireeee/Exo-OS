//! # main.rs — Exo-OS Kernel Entry Point
//!
//! Boot flow (GRUB Multiboot2 BIOS):
//! ```
//!   GRUB  _start  (32-bit protected mode, EAX=magic, EBX=info)
//!      trampoline 32→64 bits (page tables + EFER.LME + CR0.PG + retf)
//!      _start64  (64-bit long mode)
//!        call kernel_main(mb2_magic, mb2_info, 0)
//!          arch_boot_init()  (CPU, GDT, IDT, TSS, APIC, SMP)
//!          kernel_init(cpu_count) (memory  scheduler  process  ipc  fs)
//!          halt_cpu()
//! ```
//!
//! Boot flow (exo-boot UEFI):
//! ```
//!   exo-boot  handoff_to_kernel()  sets EAX=EXOBOOT_MAGIC_U32, RBX=BootInfo
//!      jump to _start_uefi (64-bit entry, long mode already active)
//!      _start64  kernel_main(EXOBOOT_MAGIC_U32, boot_info_phys, 0)
//! ```

#![no_std]
#![no_main]
#![allow(binary_asm_labels)]
#![allow(unexpected_cfgs)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::explicit_auto_deref)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::unnecessary_safety_doc)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::declare_interior_mutable_const)]
#![allow(clippy::mut_from_ref)]

use exo_os_kernel as kernel;

// ── Multiboot2 Header ─────────────────────────────────────────────────────────
// PATCH-P2-BOOT: Multiboot2 Header gated behind --features multiboot2_compat.
// DEPRECATED (Strata vision: UEFI-only via exo-boot SPEC-BOOTLOADER-GPT-STRATA.md).
// ACTIVE by default (default feature) to preserve current QEMU/GRUB workflow.
// Phase 8 not yet complete -- removal planned after UEFI GPT migration.
// Dev QEMU/GRUB (default) : cargo build
// Production UEFI-only    : cargo build --release --no-default-features
//
// GRUB searches for magic 0xE85250D6 within the first 32 KiB of the image.
// Must be in the `.multiboot2` section, which is FIRST in linker.ld.
#[cfg(feature = "multiboot2_compat")]
core::arch::global_asm!(
    ".section .multiboot2, \"a\"",
    ".align 8",
    ".long 0xE85250D6", // Magic Multiboot2
    ".long 0",          // Architecture i386 (GRUB enters in 32-bit prot. mode)
    ".long 48",         // Header length: 16 + fb tag(20) + pad(4) + end tag(8)
    ".long 0x17ADAEFA", // Checksum : -(0xE85250D6 + 0 + 48) mod 2^32
    ".short 5",
    ".short 0",
    ".long 20", // Framebuffer request tag
    ".long 1024",
    ".long 768",
    ".long 32",
    ".long 0", // padding 8-byte alignment before end tag
    ".short 0",
    ".short 0",
    ".long 8", // End tag
);

// ── Boot Page Tables (BSS, zero-initialized, 4 KiB-aligned) ───────────────────
// Used by the 32→64 bits trampoline to establish the identity mapping
// 0..1 GiB before enabling paging (long mode requires PTEs).
//
// Layout:
//   PML4[0]     _boot_pdpt (identity: virtual 0..512 GiB)
//   PDPT[0]     _boot_pd   (identity: virtual 0..1 GiB)
//   PDPT[3]     _boot_pd_high (MMIO APIC: virtual 3..4 GiB, for LAPIC/IOAPIC)
//   PD[0..512] = huge pages 2 MiB (identity: virtual 0..1 GiB -> phys 0..1 GiB)
//   _boot_pd_high[502] = 0xFEC00000 (IOAPIC MMIO)
//   _boot_pd_high[503] = 0xFEE00000 (LAPIC MMIO)
//
// NOTE: 1 GiB amply covers kernel (~10 MiB) + stack (~46 MiB).
//       APIC mapping (0xFEE00000) is required for init_apic_system.
core::arch::global_asm!(
    ".section .bss",
    ".balign 4096",
    ".global _boot_pml4",
    "_boot_pml4:",
    ".space 4096",
    ".global _boot_pdpt",
    "_boot_pdpt:",
    ".space 4096",
    ".global _boot_pd",
    "_boot_pd:",
    ".space 4096",
    ".global _boot_pd_high",
    "_boot_pd_high:",
    ".space 4096",
    // Save Multiboot2 args (EAX/EBX) during 32→64 transition
    ".global _mb2_saved_magic",
    "_mb2_saved_magic:",
    ".long 0",
    ".global _mb2_saved_info",
    "_mb2_saved_info:",
    ".quad 0",
);

// ── Minimal 64-bit GDT for boot trampoline ───────────────────────────────────
// Loaded in 32-bit by lgdt, then used in 64-bit after retf.
// Format: null (8B) + code64 (8B) + data64 (8B) = 24 bytes.
//
//   CS = 0x08: L=1 (64-bit code), DPL=0, P=1, Type=0xA (code exec/read)
//   DS = 0x10: L=0, D=1 (32/64-bit data), DPL=0, P=1, Type=0x2 (data r/w)
core::arch::global_asm!(
    ".section .rodata",
    ".balign 16",
    ".global _boot_gdt",
    "_boot_gdt:",
    ".quad 0x0000000000000000", // 0x00 — null descriptor
    ".quad 0x00af9a000000ffff", // 0x08 — 64-bit code, DPL=0, L=1
    ".quad 0x00cf92000000ffff", // 0x10 — data,       DPL=0, D=1
    "_boot_gdt_end_marker:",
    ".global _boot_gdtr",
    "_boot_gdtr:",
    ".short 23",        // limit = 3*8 - 1 = 23
    ".long  _boot_gdt", // base (32-bit physical — kernel < 4 GiB)
);

// ── Boot trampoline: 32-bit → 64-bit (long mode) ──────────────────────────────
//
// GRUB enters here in 32-bit protected mode:
//   EAX = 0x36d76289 (Multiboot2 magic)
//   EBX = physical address of Multiboot2 Info structure (<4 GiB)
//   CS  = flat 32-bit descriptor (base=0, limit=4GiB)
//   PE=1, IF=0, paging=0
//
// Goal: enable long mode before calling kernel_main (64-bit Rust).
//
// SAFETY: all symbol addresses are < 1 GiB (kernel at 1 MiB).
//         Identity-mapping covers 0..1 GiB with 2 MiB huge pages.
core::arch::global_asm!(
    ".section .text.boot",
    ".global _start",
    ".type _start, @function",
    ".code32",
    // 
    "_start:",
    "cli",
    "cld",
    // Save Multiboot2 args before clobbering EBX
    "mov dword ptr [_mb2_saved_magic], eax",
    "mov dword ptr [_mb2_saved_info],  ebx",
    "mov dword ptr [_mb2_saved_info + 4], 0", // zero-extend to 64 bits
    // Provisional 32-bit stack (physical address = virtual before paging)
    // _exo_boot_stack_top is defined below (.boot_stack section).
    // lea yields the effective ADDRESS of the label (not its content).
    "lea esp, [_exo_boot_stack_top]",
    "and esp, -16",
    // Build PD: 512 huge pages 2 MiB (identity 0..1 GiB)
    // Flags: P=1 (present), R/W=1 (writable), PS=1 (2 MiB page) = 0x83
    // Each entry: (index << 21) | 0x83
    "xor ecx, ecx",
    "_pd_loop:",
    "  cmp ecx, 512",
    "  jge _pd_done",
    "  mov eax, ecx",
    "  shl eax, 21",         // physical address = index × 2 MiB
    "  or  eax, 0x83",       // flags R/W + Present + PageSize
    "  lea edi, [_boot_pd]", // base of PD
    "  mov ebx, ecx",
    "  shl ebx, 3", // offset = index × 8 bytes
    "  add edi, ebx",
    "  mov dword ptr [edi],     eax",
    "  mov dword ptr [edi + 4], 0", // upper 32 bits = 0
    "  inc ecx",
    "  jmp _pd_loop",
    "_pd_done:",
    // PDPT[0] -> PD (identity 0..1 GiB)
    "lea eax, [_boot_pd]",
    "or  eax, 0x03", // P + R/W
    "mov dword ptr [_boot_pdpt],     eax",
    "mov dword ptr [_boot_pdpt + 4], 0",
    // _boot_pd_high: MMIO APIC mapping (LAPIC + IOAPIC) in 3..4 GiB
    // LAPIC MMIO: 0xFEE00000 -> PD_high[503] (PDPT[3], PD entry 503)
    // IOAPIC MMIO: 0xFEC00000 -> PD_high[502] (PD entry 502)
    // Calculation: PD_high entry index = (phys - 0xC0000000) >> 21
    //  - IOAPIC 0xFEC00000: (0xFEC00000 - 0xC0000000) >> 21 = 0x3EC00000 >> 21 = 502
    //  - LAPIC  0xFEE00000: (0xFEE00000 - 0xC0000000) >> 21 = 0x3EE00000 >> 21 = 503
    // Entry format (2 MiB huge page): phys_base | 0x83 (P + R/W + PS)
    //   IOAPIC page base: 503 - 1 = 502  phys = 0xC0000000 + 502*0x200000 = 0xFEC00000
    //   LAPIC  page base: 503         phys = 0xC0000000 + 503*0x200000 = 0xFEE00000
    //   Note: 0xFEC00000 | 0x83 = 0xFEC00083 (lower 32 bits; upper = 0)
    //         0xFEE00000 | 0x83 = 0xFEE00083

    // Write PD_high[502] = 0xFEC0009B (IOAPIC 2 MiB page, P+R/W+PS+PWT+PCD UC)
    // 0x9B = 0x83 | 0x08 (PWT) | 0x10 (PCD): selects PAT[3]=UC (Intel default)
    "lea edi, [_boot_pd_high]",
    "add edi, 4016",
    "mov dword ptr [edi],     0xFEC0009B",
    "mov dword ptr [edi + 4], 0",
    // Write PD_high[503] = 0xFEE0009B (LAPIC 2 MiB page, P+R/W+PS+PWT+PCD UC)
    "lea edi, [_boot_pd_high]",
    "add edi, 4024",
    "mov dword ptr [edi],     0xFEE0009B",
    "mov dword ptr [edi + 4], 0",
    // PDPT[3] -> _boot_pd_high (MMIO APIC region); offset = 3*8 = 24
    "lea eax, [_boot_pd_high]",
    "or  eax, 0x03", // P + R/W
    "mov dword ptr [_boot_pdpt + 24],     eax",
    "mov dword ptr [_boot_pdpt + 28], 0",
    // PML4[256] -> _boot_pdpt (physmap 0xFFFF_8000_0000_0000)
    // Reuses existing _boot_pdpt (2 MiB pages via _boot_pd).
    // CPUID.PDPE1GB not required (1 GiB pages not universally supported).
    // Covers 0..1 GiB physical via PDPT[0]->_boot_pd (sufficient for -m 256M).
    // PML4 index 256 = bits[47:39] of 0xFFFF_8000_0000_0000; offset = 2048.
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03", // P + R/W
    "mov dword ptr [_boot_pml4 + 2048],     eax",
    "mov dword ptr [_boot_pml4 + 2052], 0",
    // PML4[0] -> PDPT
    "lea eax, [_boot_pdpt]",
    "or  eax, 0x03", // P + R/W
    "mov dword ptr [_boot_pml4],     eax",
    "mov dword ptr [_boot_pml4 + 4], 0",
    // Load our 64-bit GDT
    "lgdt [_boot_gdtr]",
    // CR3 = PML4
    "lea eax, [_boot_pml4]",
    "mov cr3, eax",
    // CR4.PAE = 1 (Physical Address Extension, required for long mode)
    "mov eax, cr4",
    "or  eax, 0x00000020", // bit 5 = PAE
    "mov cr4, eax",
    // EFER.LME = 1 (Long Mode Enable, MSR 0xC0000080)
    "mov ecx, 0xC0000080",
    "rdmsr",
    "or  eax, 0x00000100", // bit 8 = LME
    "wrmsr",
    // CR0.PG = 1: enables paging -> activates long mode
    // CPU immediately enters compatibility mode (32-bit submode of LM).
    // Far jump below switches to pure 64-bit mode (CS with L=1).
    "mov eax, cr0",
    "or  eax, 0x80000000", // bit 31 = PG
    "mov cr0, eax",
    // Far jump to CS=0x08 (64-bit): enters 64-bit mode
    // RETF technique: pushes (CS, EIP) in reverse order then `retf`.
    // `retf` pops EIP then CS -> loads CS=0x08 (64-bit code descriptor).
    // After: CPU in 64-bit mode, CS=0x08.
    "lea eax, [_start64]", // EAX = physical address of _start64
    "push dword ptr 0x08", // push CS = 0x08 (64-bit code)
    "push eax",            // push EIP = address of _start64
    "retf",                // far return → CS:EIP loaded → 64-bit mode
    // Halt (should never be reached)
    "_boot_halt32:",
    "cli",
    "hlt",
    "jmp _boot_halt32",
    // 
    // _start_uefi: 64-bit entry for exo-boot.
    //
    // exo-boot already arrives in long mode with final CR3 loaded. Jumping to
    // _start (code32) from this context is invalid; this entry preserves
    // the same EAX/RBX convention then joins the common _start64 path.
    // 
    ".code64",
    ".global _start_uefi",
    ".type _start_uefi, @function",
    "_start_uefi:",
    "cli",
    "cld",
    "mov dword ptr [rip + _mb2_saved_magic], eax",
    "mov qword ptr [rip + _mb2_saved_info], rbx",
    "jmp _start64",
    // 
    // _start64: first 64-bit code after long mode transition
    // 
    ".global _start64",
    ".type _start64, @function",
    "_start64:",
    // Debug marker port 0xE9: 'X' = 0x58 confirms that _start64 is reached
    "mov al, 0x58", // 'X'
    "out 0xe9, al",
    // Reload data selectors with DS=0x10 (64-bit data)
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "xor ax, ax",
    "mov fs, ax",
    "mov gs, ax",
    // Clean 64-bit stack (symbol resolved by linker)
    "lea rsp, [rip + _exo_boot_stack_top]",
    "and rsp, -16",
    // Restore Multiboot2 arguments saved in .bss
    //   RDI = mb2_magic (u32)
    //   RSI = mb2_info  (u64)
    //   RDX = rsdp_phys (u64, 0 -> arch_boot_init will scan ACPI)
    "mov edi, dword ptr [rip + _mb2_saved_magic]",
    "mov rsi, qword ptr [rip + _mb2_saved_info]",
    "xor edx, edx",
    // Call kernel_main (Rust, 64-bit SysV ABI)
    "call kernel_main",
    // Final idle if kernel_main returns (should not)
    "cli",
    "_start64_halt:",
    "hlt",
    "jmp _start64_halt",
    ".size _start, . - _start",
);

// ── BSP Boot Stack ──────────────────────────────────────────────────────────
// Section .boot_stack: NOBITS (like .bss, absent from ELF image).
// `_exo_boot_stack_top` = initial RSP of BSP (stack grows downwards).
core::arch::global_asm!(
    ".section .boot_stack, \"aw\", @nobits",
    ".balign 4096",
    ".global _exo_boot_stack_bottom",
    "_exo_boot_stack_bottom:",
    // 512 KiB (was 64 KiB — too small: overflow on the deep
    // exofs_init/virtio/recovery path corrupted adjacent IDT/TSS/IST →
    // triple fault bypassing handlers. 4 MiB collided with SSR at 16 MiB;
    // 512 KiB = comfortable margin without collision. FIX-BOOT-STACK.)
    ".space 524288",
    ".global _exo_boot_stack_top",
    "_exo_boot_stack_top:",
    ".section .text",
);

// ── Main Rust Entry Point ───────────────────────────────────────────

/// `kernel_main` — first Rust function executed on the BSP.
///
/// # Safety
/// - Called from `_start` (ASM) exactly once on the BSP
/// - Interrupts disabled (EFLAGS.IF = 0)
/// - Valid BSP stack (`BOOT_STACK` in `early_init.rs`)
/// - 64-bit long mode active, minimal bootloader GDT loaded
#[no_mangle]
pub unsafe extern "C" fn kernel_main(
    mb2_magic: u32, // 0x36d76289 if valid Multiboot2 boot, else 0
    mb2_info: u64,  // Physical address of Multiboot2 Info structure (or 0)
    rsdp_phys: u64, // Physical RSDP address passed by bootloader (or 0)
) -> ! {
    // Debug: kernel_main start marker on port 0xE9 ('K' = 0x4B)
    core::arch::asm!("mov al, 0x4B", "out 0xe9, al", options(nostack, nomem));

    // Fallback boot console. In graphical GRUB mode, this fallback might not
    // be visible; the framebuffer will take over upon return from arch_boot_init().
    kernel::arch::x86_64::boot_display::boot_screen();

    // ── Phase 1: Architecture (GDT, IDT, TSS, per-CPU, TSC, FPU, ACPI, APIC,
    //             SYSCALL, Spectre, SMP boot of APs)
    // SAFETY: arch_boot_init must be the FIRST Rust code executed in Ring 0.
    // Satisfied preconditions: long mode, interrupts off, valid stack.
    let boot_info = kernel::arch_boot_init(mb2_magic, mb2_info, rsdp_phys);

    // Debug: arch_boot_init complete ('A' = 0x41)
    core::arch::asm!("mov al, 0x41", "out 0xe9, al", options(nostack, nomem));

    if boot_info.framebuffer_phys_addr != 0 && boot_info.framebuffer_size_bytes != 0 {
        core::arch::asm!("mov al, 0x50", "out 0xe9, al", options(nostack, nomem));
    // 'P'
    } else {
        core::arch::asm!("mov al, 0x70", "out 0xe9, al", options(nostack, nomem));
        // 'p'
    }

    let framebuffer_attached = kernel::arch::x86_64::boot_display::attach_framebuffer(&boot_info);
    if framebuffer_attached {
        core::arch::asm!("mov al, 0x56", "out 0xe9, al", options(nostack, nomem));
    // 'V'
    } else {
        core::arch::asm!("mov al, 0x76", "out 0xe9, al", options(nostack, nomem));
        // 'v'
    }
    kernel::arch::x86_64::boot_display::stage_ok("ARCH");
    if kernel::arch::x86_64::framebuffer_early::is_active() {
        core::arch::asm!("mov al, 0x57", "out 0xe9, al", options(nostack, nomem));
    // 'W'
    } else {
        core::arch::asm!("mov al, 0x77", "out 0xe9, al", options(nostack, nomem));
        // 'w'
    }

    // ── Phases 2–7: Layer initialization (memory, scheduler, process, ipc, fs)
    // SAFETY: kernel_init follows the strict layer ordering defined in docs/refonte.
    // Must be called after arch_boot_init (requires GDT + interrupts ready).
    kernel::kernel_init(boot_info.cpu_count as usize);

    // Debug: kernel_init complete ('I' = 0x49)
    core::arch::asm!("mov al, 0x49", "out 0xe9, al", options(nostack, nomem));

    let userspace_status = kernel::userspace_boot::boot_userspace();

    // VGA/framebuffer screen update: kernel initialized, scheduler alive.
    kernel::arch::x86_64::boot_display::boot_complete();
    match userspace_status {
        kernel::userspace_boot::BootUserspaceStatus::Started { .. } => {
            kernel::arch::x86_64::boot_display::userspace_status(
                "USERSPACE HANDOFF: INIT PID 1",
                "/sbin/exo-init-server charge depuis le disque ExoFS racine.",
                "/lib/ld-exo.so assure le handoff avant les services Ring1.",
            );
            core::arch::asm!("mov al, 0x55", "out 0xe9, al", options(nostack, nomem));
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

    // Debug: boot complete — '\n', 'O', 'K', '\n'
    core::arch::asm!(
        "mov al, 0x0a",
        "out 0xe9, al",
        "mov al, 0x4f",
        "out 0xe9, al", // 'O'
        "mov al, 0x4b",
        "out 0xe9, al", // 'K' (second, diff from first K)
        "mov al, 0x0a",
        "out 0xe9, al",
        options(nostack, nomem)
    );

    #[cfg(exophoenix_resurrection_test)]
    kernel::exophoenix::resurrection::trigger_self_destruct();

    // ── Scheduler idle ──────────────────────────────────────────────────
    // End of boot: do not use halt_cpu() here. `halt_cpu()` disables IF with
    // `cli; hlt` and prevents any enqueued kthread/PID 1 from taking the CPU. The
    // idle scheduler loop below keeps IRQs active and attempts a switch before
    // each HLT.
    #[cfg(not(exophoenix_resurrection_test))]
    kernel::arch::x86_64::run_scheduler_idle()
}

// ── Panic / OOM Handlers ─────────────────────────────────────────────
// Defined in lib.rs — binary inherits implementation via rlib.
// Do not redefine here to avoid lang_item conflicts.
