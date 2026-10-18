//! # exophoenix/stage0/mod.rs — ExoPhoenix Stage 0 (Bootstrap Kernel-B)
//!
//! Orchestrateur central du bootstrap de Kernel B (étapes 1a→13) :
//! - 1a) install page tables B
//! - 1b) probe CPUID global
//! - 2) stack B + guard page !PRESENT
//! - 3) init TSS (init_b_tss)
//! - 4) IDT stubs (vecteurs ExoPhoenix inactifs)
//! - 5) parse ACPI (MADT/FADT/FACS)
//! - 5b) enum PCI + calcul taille pool R3
//! - 6) build apic_to_slot[256] depuis MADT réel
//! - 7) calibration APIC timer via PIT ch2 -> TICKS_PER_US
//! - 8) init APIC local dispatch
//! - 9) setup IOMMU deny-by-default + ACS root ports
//! - 10) FACS RO + hash MADT
//! - 11) init pool R3
//! - 12) armer watchdog APIC
//! - 13) armer vecteurs de récupération et SIPI

pub mod apic_routing;
pub mod features;
pub mod hardware_scan;
pub mod isolation;

pub use apic_routing::{
    apic_slot, arm_apic_watchdog, build_apic_to_slot_from_real_madt,
    calibrate_and_store_ticks_per_us, calibrate_apic_timer_via_pit_ch2,
    init_local_apic_dispatch, send_sipi_once, set_apic_slot, ticks_per_us,
    watchdog_armed_ms, watchdog_armed_ticks, SendSipiError, TICKS_PER_US,
};
pub use features::{
    detect_apic_mode, init_feature_probe, vmxoff_if_active, BootApicMode, BootFeatures, B_FEATURES,
};
pub use hardware_scan::{
    b_device, b_device_count, calc_pool_r3_size, driver_blob_id, enumerate_pci_devices,
    hash_and_store_madt, madt_hash, parse_facs_from_fadt, parse_stage0_acpi,
    PciDeviceRecord, Stage0AcpiSnapshot, MAX_B_DEVICES,
};
pub use isolation::{
    activate_exophoenix_vectors, blocked_domain_id, deactivate_exophoenix_vectors,
    exophoenix_vectors_active, facs_ro_marked, init_b_tss, init_pool_r3_from_stage0_size,
    install_b_page_tables, iommu_acs_root_ports, iommu_policy_ready, mark_facs_ro_in_a_pts,
    pool_r3_alloc_bytes, pool_r3_base_phys, pool_r3_size_bytes, setup_b_idt_with_stubs,
    setup_b_stack_with_guard_page, setup_iommu_stage0, B_STACK_GUARD_SIZE, B_STACK_SIZE,
};

use core::sync::atomic::Ordering;
use crate::exophoenix::{sentinel, PhoenixState, PHOENIX_STATE};
use apic_routing::{CORE_A_SLOT, A_ENTRY_VECTOR, WATCHDOG_DEFAULT_MS};

#[derive(Clone, Copy, Debug)]
pub struct Stage0Summary {
    pub b_cr3: u64,
    pub b_stack_top: u64,
    pub acpi: Stage0AcpiSnapshot,
    pub pci_device_count: usize,
    pub pool_r3_size: u64,
    pub apic_slots_mapped: usize,
    pub ticks_per_us: u64,
}

/// Orchestrateur strict des étapes 1a→12.
/// Exécute les étapes stage0 ExoPhoenix.
///
/// FIX-BOOT-STAGE0-HANG : `kernel_a_boot = true` quand la fonction est appelée
/// INLINE sur le boot du noyau principal (Kernel A, lib.rs Phase 5b). Dans ce
/// cas, on SAUTE les étapes qui reconfigurent l'état CPU live du BSP de Kernel A
/// (pile/TSS/IDT/timer APIC/watchdog), car elles appartiennent à Kernel B et
/// clobbent l'état de Kernel A.
pub fn stage0_init_all_steps(kernel_a_boot: bool) -> Stage0Summary {
    // SAFETY: Stage0 s'exécute sur Kernel B avant la prise de contrôle normale
    // de Kernel A ; ExoSeal phase 0 est idempotent.
    unsafe {
        crate::security::exoseal::exoseal_boot_phase0();
    }

    // 1a) Page tables de B
    let b_cr3 = install_b_page_tables();

    if let Err(err) = super::ssr::initialize_layout_v7() {
        log::error!(
            "SSR ExoPhoenix incompatible au boot: {:?} (attendu v{}.{} magic={:#x})",
            err,
            super::ssr::SSR_LAYOUT_MAJOR,
            super::ssr::SSR_LAYOUT_MINOR,
            exo_phoenix_ssr::SSR_MAGIC
        );
        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
    }

    #[inline(always)]
    fn s0dbg(tag: u8) {
        // SAFETY: port 0xE9 = ISA debug device QEMU, sans effet mémoire, dispo dès le reset.
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") b'@', options(nomem, nostack));
            core::arch::asm!("out 0xE9, al", in("al") tag, options(nomem, nostack));
        }
    }
    s0dbg(b'1'); // SSR ok/dégradé

    // 1b) Probe CPUID global (HPET faux jusqu'au parse ACPI)
    init_feature_probe(false);
    s0dbg(b'2');

    // ── Étapes 2/3/4 : état CPU de Kernel B (pile + TSS + IDT) ────────────────
    let b_stack_top = if kernel_a_boot {
        isolation::B_STACK_TOP.load(Ordering::Acquire)
    } else {
        // 2) Stack B + guard page
        let top = setup_b_stack_with_guard_page();
        s0dbg(b'3');
        // 3) TSS/IST B
        init_b_tss(top);
        s0dbg(b'4');
        // 4) IDT stubs
        setup_b_idt_with_stubs();
        top
    };
    s0dbg(b'5');

    // 5) ACPI (MADT/FADT/FACS) — lecture seule
    let acpi = parse_stage0_acpi();
    s0dbg(b'6');

    // 5b) PCI + taille pool R3 — lecture seule (config space)
    let pci_device_count = enumerate_pci_devices();
    s0dbg(b'7');
    const POOL_R3_MIN_SIZE: u64 = 8 * 1024 * 1024;
    let pool_r3_size = isolation::POOL_R3_SIZE_BYTES
        .load(Ordering::Acquire)
        .max(POOL_R3_MIN_SIZE);
    isolation::set_pool_r3_size_bytes(pool_r3_size);

    // 6) APIC->slot depuis MADT réel — lecture seule
    let apic_slots_mapped = build_apic_to_slot_from_real_madt(acpi.madt_phys);
    s0dbg(b'8');

    // ── Étapes 7/8 : timer APIC de Kernel B ───────────────────────────────────
    let ticks_per_us = if kernel_a_boot {
        ticks_per_us()
    } else {
        // 7) APIC timer via PIT ch2
        let t = calibrate_and_store_ticks_per_us();
        s0dbg(b'9');
        // 8) APIC local dispatch + VMXOFF défensif
        init_local_apic_dispatch();
        vmxoff_if_active();
        t
    };
    s0dbg(b'a');

    // 9) IOMMU + ACS root-ports + IOTLB flush (no-op sans VT-d / domain_count==0)
    setup_iommu_stage0(pool_r3_size);
    crate::security::exoseal::configure_nic_iommu_policy();
    s0dbg(b'b');

    // 10) FACS en RO + hash MADT
    let _ = mark_facs_ro_in_a_pts(acpi.facs_phys);
    let _ = hash_and_store_madt(acpi.madt_phys);
    s0dbg(b'c');

    // 11) Pool R3
    let _ = init_pool_r3_from_stage0_size(pool_r3_size);
    s0dbg(b'd');

    // 12) Watchdog APIC
    if !kernel_a_boot {
        let _ = arm_apic_watchdog(WATCHDOG_DEFAULT_MS);
    }
    s0dbg(b'e');

    // 13) Armer les vecteurs de récupération ExoPhoenix (0xF1/0xF2/0xF3).
    if PHOENIX_STATE.load(Ordering::Acquire) != PhoenixState::Degraded as u8 {
        activate_exophoenix_vectors();
    }

    Stage0Summary {
        b_cr3,
        b_stack_top,
        acpi,
        pci_device_count,
        pool_r3_size,
        apic_slots_mapped,
        ticks_per_us,
    }
}

/// Stage0 complet (1→13): bascule Normal, SIPI one-shot, puis boucle sentinelle.
pub fn stage0_init() -> ! {
    let _summary = stage0_init_all_steps(false);

    if !crate::exophoenix::forge::kernel_a_image_provisioned() {
        log::error!("FORGE: image Kernel A absente — ExoPhoenix désactivé (degraded)");
        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }

    let n_cores = crate::arch::x86_64::smp::init::smp_cpu_count();
    exo_phoenix_ssr::init_core_count(n_cores.min(exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT as u32));

    if let Err(err) = super::ssr::validate_layout_v7() {
        log::error!("SSR ExoPhoenix v7 invalide après init SMP: {:?}", err);
        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem));
            }
        }
    }

    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);
    let _ = send_sipi_once(CORE_A_SLOT, A_ENTRY_VECTOR);
    sentinel::run_forever()
}
