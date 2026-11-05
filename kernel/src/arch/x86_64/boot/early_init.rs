//! # arch/x86_64/boot/early_init.rs — Early Architecture Initialization
//!
//! First Rust code executed after the bootloader transitions to 64-bit mode.
//! This function is called from the ASM entry point (`_start` or `kernel_start`).
//!
//! ## Entry Invariants
//! - 64-bit long mode active
//! - Identity paging (1:1) over the first 4 GiB (or per Multiboot2/UEFI)
//! - Temporary bootloader GDT loaded
//! - IDT empty (or absent)
//! - Interrupts disabled (EFLAGS.IF = 0)
//!
//! ## What early_init does
//! 1. Detect CPU features
//! 2. Load BSP per-CPU GDT
//! 3. Load IDT
//! 4. Initialize TSS + IST stacks
//! 5. Init per-CPU / GS
//! 6. Initialize TSC
//! 7. Hypervisor detection
//! 8. Init ACPI (RSDP → MADT → HPET)
//! 9. Init APIC
//! 10. Init FPU / SSE / AVX
//! 11. Apply Spectre/Meltdown mitigations
//! 12. Boot APs (SMP)
//! 13. Return → kernel_main()

// ── BSP Boot Stack ───────────────────────────────────────────────────────────

// The BSP boot stack (64 KiB) is defined in main.rs via global_asm! under
// the .boot_stack section (type @nobits, not stored in the image).
// The `_exo_boot_stack_top` symbol is the top (end address) of the stack;
// this is the address loaded into RSP by `_start` at the very beginning of boot.

extern "C" {
    /// Top of BSP boot stack (end address + 1 of the .boot_stack section).
    /// Defined by global_asm! in main.rs.
    static _exo_boot_stack_top: u8;
}

/// Returns the top address of the BSP boot stack.
///
/// Used to initialize RSP0 in the TSS before the first task switch.
pub fn boot_stack_top() -> u64 {
    // &raw const is stable since Rust 1.82 and does not require unsafe.
    // _exo_boot_stack_top is defined in main.rs (global_asm!),
    // never null, valid for the entire kernel lifetime.
    &raw const _exo_boot_stack_top as u64
}

// ── Boot Information Passed to Main Kernel ───────────────────────────────────

/// Boot information gathered by early_init
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BootFramebufferFormat {
    Rgbx = 0,
    Bgrx = 1,
    Unknown = 2,
    None = 0xFFFF_FFFF,
}

/// Boot information gathered by early_init
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub rsdp_phys: u64,
    pub multiboot2_magic: u32,
    pub multiboot2_addr: u64,
    pub uefi_memmap_addr: u64,
    pub uefi_memmap_size: u64,
    pub uefi_desc_size: u32,
    pub total_memory_kb: u64,
    pub cpu_count: u32,
    pub lapic_base: u64,
    pub framebuffer_phys_addr: u64,
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub framebuffer_stride: u32,
    pub framebuffer_bpp: u32,
    pub framebuffer_format: BootFramebufferFormat,
    pub framebuffer_size_bytes: u64,
}

impl BootInfo {
    const fn zeroed() -> Self {
        Self {
            rsdp_phys: 0,
            multiboot2_magic: 0,
            multiboot2_addr: 0,
            uefi_memmap_addr: 0,
            uefi_memmap_size: 0,
            uefi_desc_size: 0,
            total_memory_kb: 0,
            cpu_count: 1,
            lapic_base: 0xFEE00000,
            framebuffer_phys_addr: 0,
            framebuffer_width: 0,
            framebuffer_height: 0,
            framebuffer_stride: 0,
            framebuffer_bpp: 0,
            framebuffer_format: BootFramebufferFormat::None,
            framebuffer_size_bytes: 0,
        }
    }
}

// ── Main Arch Initialization Entry Point ─────────────────────────────────────

/// Full initialization of the x86_64 architecture
///
/// Called from `kernel_start` (ASM entry) with:
/// - `mb2_magic`: Multiboot2 magic (0x36d76289) or 0 if UEFI
/// - `mb2_info`: pointer to Multiboot2 structure (or 0)
/// - `rsdp_phys`: RSDP physical address (passed by bootloader, 0 = auto-scan)
///
/// # Safety
/// Preconditions: Ring 0, interrupts disabled, 64-bit long mode.
pub unsafe fn arch_boot_init(mb2_magic: u32, mb2_info: u64, rsdp_phys: u64) -> BootInfo {
    super::super::disable_interrupts();

    // Inline debug probe for each step (port 0xE9)
    macro_rules! probe {
        ($b:expr) => {
            core::arch::asm!("out 0xe9, al", in("al") $b as u8, options(nostack, nomem));
        }
    }

    // ── Step 1: Detect CPU features ──────────────────────────────────────────
    probe!(b'1');
    super::super::cpu::features::init_cpu_features();
    let features = super::super::cpu::features::cpu_features();

    // Critical assertions (init_cpu_features() already checks some)
    if !features.has_sse2() {
        crate::arch::x86_64::terminal::debug_write(b"early_init: SSE2 requis\n");
        crate::arch::x86_64::halt_cpu();
    }
    if !features.has_syscall() {
        crate::arch::x86_64::terminal::debug_write(b"early_init: SYSCALL requis\n");
        crate::arch::x86_64::halt_cpu();
    }
    super::super::cpu::msr::configure_pat();

    // ── Step 2: BSP per-CPU GDT ──────────────────────────────────────────────
    probe!(b'2');
    let kernel_stack_top = boot_stack_top();
    super::super::gdt::init_gdt_for_cpu(0, kernel_stack_top);

    // ── Step 3: IDT ───────────────────────────────────────────────────────────
    probe!(b'3');
    super::super::idt::init_idt();
    super::super::idt::load_idt();
    super::super::irq::pic::remap_and_mask();

    // ── Step 4: BSP per-CPU TSS (IST stacks) ──────────────────────────────────
    // (already done in init_gdt_for_cpu which calls init_tss_for_cpu + load_tss)
    probe!(b'4');

    // ── Step 5: Per-CPU data / GS ─────────────────────────────────────────────
    probe!(b'5');
    let lapic_id_bsp = 0; // BSP is CPU 0; LAPIC ID will be updated after APIC init
    super::super::smp::percpu::init_percpu_for_bsp(kernel_stack_top, lapic_id_bsp);

    // ── Step 6: TSC ───────────────────────────────────────────────────────────
    probe!(b'6');
    super::super::cpu::tsc::init_tsc(0);

    // ── Step 7: FPU / SSE / AVX ──────────────────────────────────────────────
    probe!(b'7');
    super::super::cpu::fpu::init_fpu_for_cpu();
    crate::security::exoargos::init_pmu();

    // ── Step 8: Hypervisor detection ─────────────────────────────────────────
    probe!(b'8');
    let hv = super::super::virt::detect::detect_hypervisor();
    let _ = hv;

    // ── Step 9: ACPI ──────────────────────────────────────────────────────────
    probe!(b'9');
    let mut boot_info = BootInfo::zeroed();

    let rsdp = if rsdp_phys != 0 {
        super::super::acpi::parser::init_acpi_from_rsdp(rsdp_phys);
        rsdp_phys
    } else {
        match super::super::acpi::parser::find_rsdp() {
            Some(addr) => {
                super::super::acpi::parser::init_acpi_from_rsdp(addr);
                addr
            }
            None => 0,
        }
    };
    boot_info.rsdp_phys = rsdp;

    let acpi = super::super::acpi::parser::acpi_info();
    let madt_missing_before_memory = acpi.madt_phys == 0;
    let hpet_missing_before_memory = acpi.hpet_phys == 0;
    let fadt_missing_before_memory = acpi.fadt_phys == 0;

    // MADT → LAPIC IDs + I/O APIC
    let mut madt_info = if acpi.madt_phys != 0 {
        Some(super::super::acpi::madt::parse_madt(acpi.madt_phys))
    } else {
        None
    };

    // HPET
    if acpi.hpet_phys != 0 {
        super::super::acpi::hpet::init_hpet(acpi.hpet_phys);
    }

    // PM Timer
    if acpi.fadt_phys != 0 {
        super::super::acpi::pm_timer::init_pm_timer(acpi.fadt_phys);
    }

    // ── Step 10: APIC ─────────────────────────────────────────────────────────
    probe!(b'a');
    super::super::apic::init_apic_system();
    if acpi.madt_phys != 0 {
        super::super::apic::io_apic::init_all_ioapics();
    }

    // Recalibrate LAPIC timer after TSC
    probe!(b'b');
    super::super::apic::local_apic::calibrate_lapic_timer();

    // ── Arch ↔ memory integration: register TLB IPI sender ────────────────────
    probe!(b'c');
    super::super::memory_iface::init_memory_integration();

    // ── Step 11: Init SYSCALL interface ──────────────────────────────────────
    probe!(b'd');
    super::super::syscall::init_syscall();

    // ── Step 12: Multiboot2 / UEFI ───────────────────────────────────────────
    probe!(b'e');
    if mb2_magic == 0x36d76289 && mb2_info != 0 {
        boot_info.multiboot2_magic = mb2_magic;
        boot_info.multiboot2_addr = mb2_info;
        let mb2 = super::multiboot2::parse_multiboot2(mb2_info);
        if mb2.module_start != 0 && mb2.module_end > mb2.module_start {
            super::multiboot2::set_multiboot2_module(mb2.module_start, mb2.module_end);
        }
        boot_info.total_memory_kb = mb2.total_memory_kb;
        boot_info.rsdp_phys = if mb2.rsdp_phys != 0 {
            mb2.rsdp_phys
        } else {
            rsdp
        };
        boot_info.framebuffer_phys_addr = mb2.framebuffer_phys_addr;
        boot_info.framebuffer_width = mb2.framebuffer_width;
        boot_info.framebuffer_height = mb2.framebuffer_height;
        boot_info.framebuffer_stride = mb2.framebuffer_stride;
        boot_info.framebuffer_bpp = mb2.framebuffer_bpp;
        boot_info.framebuffer_format = match mb2.framebuffer_format {
            super::multiboot2::MultibootFramebufferFormat::None => BootFramebufferFormat::None,
            super::multiboot2::MultibootFramebufferFormat::Rgbx => BootFramebufferFormat::Rgbx,
            super::multiboot2::MultibootFramebufferFormat::Bgrx => BootFramebufferFormat::Bgrx,
            super::multiboot2::MultibootFramebufferFormat::Unknown => {
                BootFramebufferFormat::Unknown
            }
        };
        boot_info.framebuffer_size_bytes = mb2.framebuffer_size_bytes;

        // ── Physical memory subsystem init (E820) ─────────────────────────────────────
        // Rule MEM-02 DOC2: EmergencyPool FIRST before any allocator
        crate::memory::physical::frame::emergency_pool::init();
        // Phases 1→4: bitmap | free_regions | slab | NUMA (from E820 map)
        super::memory_map::init_memory_subsystem_multiboot2(&mb2);

        // ── Kernel address space init ─────────────────────────────────────────────────
        // Registers current PML4 (already configured by bootloader)
        crate::memory::virt::address_space::KERNEL_AS.init(
            crate::memory::core::types::PhysAddr::new(super::super::read_cr3()),
        );
        crate::memory::virt::fault::swap_in::register_backend_swap_provider();

        // ── Hardware memory protections (NX / SMEP / SMAP / PKU) ──────────────────────
        // Enabled after complete memory subsystem init (DOC2 §2.3)
        crate::memory::protection::init();
    } else if mb2_magic == super::memory_map::EXOBOOT_MAGIC_U32 && mb2_info != 0 {
        // ── exo-boot UEFI path ───────────────────────────────────────────────
        // `mb2_info` = physical address of exo-boot BootInfo (identity-mapped).
        // `mb2_magic` = EXOBOOT_MAGIC_U32 (0x4F4F_5845 "EXOO").
        //
        // RULE MEM-02: EmergencyPool FIRST.
        crate::memory::physical::frame::emergency_pool::init();

        // Init memory subsystem from exo-boot memory map
        super::memory_map::init_memory_subsystem_exoboot(mb2_info);

        // Read RSDP from BootInfo (offset 6200)
        let rsdp_from_bi = core::ptr::read_volatile((mb2_info + 6200) as *const u64);
        if rsdp_from_bi != 0 {
            super::super::acpi::parser::init_acpi_from_rsdp(rsdp_from_bi);
            boot_info.rsdp_phys = rsdp_from_bi;
        } else if rsdp != 0 {
            // Use address passed as parameter (rsdp_phys = RDX = 0 from _start,
            // but arch_boot_init may have performed an ACPI scan before this block)
            boot_info.rsdp_phys = rsdp;
        }

        // exo-boot layout synchronized with docs/exo-boot/BOOTINFO.md:
        // 6160..6199 = FramebufferInfo repr(C).
        boot_info.framebuffer_phys_addr = core::ptr::read_volatile((mb2_info + 6160) as *const u64);
        boot_info.framebuffer_width = core::ptr::read_volatile((mb2_info + 6168) as *const u32);
        boot_info.framebuffer_height = core::ptr::read_volatile((mb2_info + 6172) as *const u32);
        boot_info.framebuffer_stride = core::ptr::read_volatile((mb2_info + 6176) as *const u32);
        boot_info.framebuffer_bpp = core::ptr::read_volatile((mb2_info + 6180) as *const u32);
        boot_info.framebuffer_format =
            match core::ptr::read_volatile((mb2_info + 6184) as *const u32) {
                0 => BootFramebufferFormat::Rgbx,
                1 => BootFramebufferFormat::Bgrx,
                0xFFFF_FFFF => BootFramebufferFormat::None,
                _ => BootFramebufferFormat::Unknown,
            };
        boot_info.framebuffer_size_bytes =
            core::ptr::read_volatile((mb2_info + 6192) as *const u64);

        // Registers current PML4 (configured by exo-boot)
        crate::memory::virt::address_space::KERNEL_AS.init(
            crate::memory::core::types::PhysAddr::new(super::super::read_cr3()),
        );
        crate::memory::virt::fault::swap_in::register_backend_swap_provider();

        // Hardware memory protections (NX / SMEP / SMAP)
        crate::memory::protection::init();
    } else {
        crate::arch::x86_64::terminal::debug_write(b"early_init: boot protocol non supporte\n");
        crate::arch::x86_64::halt_cpu();
    }

    if boot_info.rsdp_phys != 0
        && (madt_missing_before_memory || hpet_missing_before_memory || fadt_missing_before_memory)
    {
        super::super::acpi::parser::init_acpi_from_rsdp(boot_info.rsdp_phys);
        let acpi_post_memory = super::super::acpi::parser::acpi_info();

        if madt_missing_before_memory && acpi_post_memory.madt_phys != 0 {
            madt_info = Some(super::super::acpi::madt::parse_madt(
                acpi_post_memory.madt_phys,
            ));
            super::super::apic::io_apic::init_all_ioapics();
        }
        if hpet_missing_before_memory && acpi_post_memory.hpet_phys != 0 {
            super::super::acpi::hpet::init_hpet(acpi_post_memory.hpet_phys);
        }
        if fadt_missing_before_memory && acpi_post_memory.fadt_phys != 0 {
            super::super::acpi::pm_timer::init_pm_timer(acpi_post_memory.fadt_phys);
        }
    }

    // ── Step 12b: Spectre/Meltdown Mitigations ───────────────────────────────
    // KPTI can construct shadow page tables and allocate frames; we must
    // therefore wait until the memory subsystem and kernel PML4 are ready.
    probe!(b'f');
    super::super::spectre::apply_mitigations_bsp();

    // ── Step 13b: Security init (SECURITY_READY) ───────────────────────────
    // Releases APs from BOOT-SEC spin-wait in smp/init.rs.
    // The call is idempotent on the boot flow side: kernel_init() also checks this flag.
    probe!(b'g');
    if !crate::security::is_security_ready() {
        let mut kaslr_entropy =
            super::super::cpu::tsc::read_tsc() ^ ((mb2_magic as u64) << 32) ^ mb2_info ^ rsdp;
        let mut rdrand = [0u8; 8];
        if crate::security::crypto::rng::rdrand_fill(&mut rdrand).is_ok() {
            kaslr_entropy ^= u64::from_le_bytes(rdrand);
        }
        crate::security::security_init(
            kaslr_entropy,
            crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR,
        );
    }
    super::super::mark_arch_initialized();

    // ── Step 14: SMP — boot APs ──────────────────────────────────────────────
    probe!(b'h');
    if let Some(ref madt) = madt_info {
        if madt.cpu_count > 1 {
            super::trampoline_asm::install_trampoline();
            let bsp_lapic = super::super::apic::local_apic::lapic_id();
            super::super::smp::init::smp_boot_aps(madt, bsp_lapic);
        }
    }
    boot_info.cpu_count = super::super::smp::init::smp_cpu_count();
    if boot_info.cpu_count as usize > 256 {
        crate::arch::x86_64::terminal::debug_write(b"early_init: trop de CPUs\n");
        crate::arch::x86_64::halt_cpu();
    }

    probe!(b'i'); // arch_boot_init completed
    boot_info
}
