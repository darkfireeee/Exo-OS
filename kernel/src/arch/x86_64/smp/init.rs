//! # arch/x86_64/smp/init.rs — Démarrage des Application Processors
//!
//! Implémente la séquence de démarrage SMP :
//! 1. BSP envoie INIT IPI à chaque AP
//! 2. BSP attend 10ms (ACPI spec)
//! 3. BSP envoie STARTUP IPI × 2 (avec adresse du trampoline)
//! 4. AP exécute le trampoline (mode réel → 64 bits)
//! 5. AP appelle `ap_entry()` pour initialiser ses structures et rejoindre le scheduler
//!
//! ## Trampoline
//! Le code assembleur du trampoline est dans `boot/trampoline_asm.rs`.
//! Il est copié à l'adresse 0x6000 (page trampoline = 6).

use super::super::acpi::madt;
use super::super::apic::ipi;
use super::super::cpu::tsc;
use super::percpu;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Adresse physique du trampoline AP (doit être < 1 MiB, aligné sur page)
pub const TRAMPOLINE_PHYS: u64 = 0x6000;
pub const TRAMPOLINE_PAGE: u8 = 6; // SIPI vector = adresse / 4096

/// Délais ACPI SMP (Intel MP Spec Section B.4)
const INIT_IPI_DELAY_MS: u64 = 10; // 10ms après INIT IPI
const STARTUP_IPI_DELAY_MS: u64 = 1; // 1ms entre les deux SIPI
const AP_STARTUP_TIMEOUT_MS: u64 = 100; // timeout d'attente par AP

static SMP_BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// Retourne le nombre de CPUs logical online
pub fn smp_cpu_count() -> u32 {
    percpu::cpu_count()
}

#[inline]
pub fn smp_boot_complete() -> bool {
    SMP_BOOT_DONE.load(Ordering::Acquire)
}

// ── Handshake BSP ↔ AP ───────────────────────────────────────────────────────

/// Zone de handshake dans le trampoline (offset 0x10 depuis le début du trampoline)
///
/// L'AP écrit `AP_ALIVE_MAGIC` ici pour signaler sa présence au BSP.
const AP_ALIVE_MAGIC: u32 = 0xA1_1A_1A_A1;
const HANDSHAKE_OFFSET: u64 = 0x10; // offset dans le trampoline

fn write_trampoline_u32(offset: u64, val: u32) {
    // SAFETY: trampoline mappé en identité, offset validé
    unsafe {
        core::ptr::write_volatile((TRAMPOLINE_PHYS + offset) as *mut u32, val);
    }
}

fn read_trampoline_u32(offset: u64) -> u32 {
    // SAFETY: trampoline mappé en identité
    unsafe { core::ptr::read_volatile((TRAMPOLINE_PHYS + offset) as *const u32) }
}

// ── Démarrage des APs ─────────────────────────────────────────────────────────

/// Démarre tous les APs listés dans la MADT
///
/// Appelé par le BSP depuis `boot::early_init` après init mémoire.
/// Chaque AP démarré incrémente `ONLINE_CPU_COUNT` et appelle `percpu::init_percpu_for_ap`.
pub fn smp_boot_aps(madt_info: &madt::MadtInfo, bsp_lapic_id: u32) {
    let n = madt_info.cpu_count;
    let apic_ids = &madt_info.apic_ids;

    for i in 0..(n as usize) {
        let apic_id = apic_ids[i];
        if apic_id == bsp_lapic_id {
            continue;
        } // skip BSP

        boot_ap(apic_id as u8);
    }

    SMP_BOOT_DONE.store(true, Ordering::Release);
}

/// Séquence INIT → SIPI → SIPI pour un AP
fn boot_ap(dest_apic_id: u8) {
    // 1. Préparer zone de handshake
    write_trampoline_u32(HANDSHAKE_OFFSET, 0);

    // 2. INIT IPI
    ipi::send_init_ipi(dest_apic_id);
    tsc::tsc_delay_ms(INIT_IPI_DELAY_MS);

    // 3. Premier SIPI
    ipi::send_startup_ipi(dest_apic_id, TRAMPOLINE_PAGE);
    tsc::tsc_delay_ms(STARTUP_IPI_DELAY_MS);

    // 4. Deuxième SIPI (certains chipsets nécessitent deux SIPIs)
    ipi::send_startup_ipi(dest_apic_id, TRAMPOLINE_PAGE);

    // 5. Attendre que l'AP signale sa présence
    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(AP_STARTUP_TIMEOUT_MS);
    loop {
        let sig = read_trampoline_u32(HANDSHAKE_OFFSET);
        if sig == AP_ALIVE_MAGIC {
            break;
        }
        if tsc::read_tsc() > deadline {
            // AP non-réactif : ignorer (peut être absent ou désactivé dans MADT)
            break;
        }
        core::hint::spin_loop();
    }
}

// ── Point d'entrée AP ─────────────────────────────────────────────────────────

/// Point d'entrée Rust pour chaque AP
///
/// Appelé depuis le trampoline assembleur après la transition en 64 bits.
/// À ce stade : paging activé, pile dédiée, GDT/IDT déjà en place depuis BSP.
///
/// # Safety
/// Appelé uniquement depuis le trampoline, en contexte monothread pour ce CPU.
#[no_mangle]
pub unsafe extern "C" fn ap_entry(cpu_id: u32, lapic_id: u32, kernel_stack_top: u64) -> ! {
    // 1. Per-CPU data (GS_BASE)
    percpu::init_percpu_for_ap(cpu_id, kernel_stack_top, lapic_id);

    // 2. GDT per-CPU + TSS
    super::super::gdt::init_gdt_for_cpu(cpu_id as usize, kernel_stack_top);

    // 3. IDT (partagée — juste LIDT)
    super::super::idt::load_idt();

    // 3b. SYSCALL/SYSRET MSRs (STAR/LSTAR/SFMASK) sur chaque AP.
    // Doit être fait avant STI pour éviter un CPU AP sans chemin syscall configuré.
    super::super::syscall::init_syscall();

    // 4. LAPIC AP
    super::super::apic::init_ap_local_apic();

    // 5. TSC calibration basique (TSC invariant → pas de recalibration)
    tsc::init_tsc(cpu_id);

    // 6. FPU
    super::super::cpu::fpu::init_fpu_for_cpu();

    // 6b. Publier un TCB idle de bootstrap pour cet AP avant STI.
    // L'AP exécute déjà sa boucle `hlt`; ce TCB représente donc le contexte
    // courant du CPU jusqu'au premier vrai switch scheduler.
    let _ = crate::scheduler::core::publish_current_boot_idle(cpu_id, kernel_stack_top);

    // 7. Mitigations spectre
    super::super::spectre::apply_mitigations_ap();

    // 8. Signaler au BSP que l'AP est prêt
    core::ptr::write_volatile(
        (TRAMPOLINE_PHYS + HANDSHAKE_OFFSET) as *mut u32,
        AP_ALIVE_MAGIC,
    );

    // 8b. RÈGLE BOOT-SEC (V-26) : attendre que le sous-système de sécurité
    //     soit initialisé (SECURITY_READY) avant toute IPC.
    while !crate::security::is_security_ready() {
        core::hint::spin_loop();
    }

    // 9. Activer les interruptions et entrer dans la boucle idle scheduler
    // SAFETY: toutes les structures sont initialisées sur cet AP
    core::arch::asm!("sti", options(nostack, nomem));

    // Boucle idle (scheduler::idle::run() sera appelé lors de l'intégration)
    loop {
        core::arch::asm!("hlt", options(nostack, nomem));
    }
}
