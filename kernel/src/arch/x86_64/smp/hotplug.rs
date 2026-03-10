//! # arch/x86_64/smp/hotplug.rs — CPU Hotplug (online/offline)
//!
//! Permet de mettre des CPUs en ligne ou hors ligne à chaud.
//! Utilisé pour la gestion de l'énergie et les migrations SMP.
//!
//! ## Séquence offline
//! 1. Migrer tous les threads du CPU vers d'autres CPUs
//! 2. Envoyer IPI_CPU_HOTPLUG au CPU cible
//! 3. Le CPU cible entre en boucle halt et se déclare offline
//! 4. BSP attend la confirmation dans `CPU_ONLINE_MASK`
//!
//! ## Séquence online
//! 1. Envoyer INIT + SIPI si le CPU est totalement froid
//! 2. (Alternative) Réveiller depuis la boucle halt via IPI wakeup
//!    si le CPU était juste en veille légère


use core::sync::atomic::{AtomicU64, Ordering};
use super::super::cpu::tsc;
use super::super::apic::ipi;

// ── Masque de CPUs online ─────────────────────────────────────────────────────

/// Bitmask des CPUs online — bit N = CPU N online
/// Supporte jusqu'à 64 CPUs directement (au-delà, utiliser un tableau d'AtomicU64)
static CPU_ONLINE_MASK: AtomicU64 = AtomicU64::new(1); // bit 0 = BSP toujours online

/// Retourne `true` si le CPU `cpu_id` est online
pub fn cpu_is_online(cpu_id: u32) -> bool {
    if cpu_id >= 64 { return false; }
    CPU_ONLINE_MASK.load(Ordering::Acquire) & (1u64 << cpu_id) != 0
}

/// Marque un CPU comme online
pub fn set_cpu_online(cpu_id: u32) {
    if cpu_id >= 64 { return; }
    CPU_ONLINE_MASK.fetch_or(1u64 << cpu_id, Ordering::AcqRel);
}

/// Marque un CPU comme offline
pub fn set_cpu_offline(cpu_id: u32) {
    if cpu_id >= 64 { return; }
    CPU_ONLINE_MASK.fetch_and(!(1u64 << cpu_id), Ordering::AcqRel);
}

/// Retourne le nombre de CPUs online
pub fn online_cpu_count() -> u32 {
    CPU_ONLINE_MASK.load(Ordering::Relaxed).count_ones()
}

// ── Mise en ligne / hors ligne ─────────────────────────────────────────────────

/// Tente de mettre un CPU en ligne
///
/// Si le CPU était en boucle halt, envoie un IPI wakeup.
/// Si le CPU était froid, lance la séquence INIT+SIPI.
///
/// Retourne `true` si le CPU est online après 100ms.
pub fn cpu_online(cpu_id: u32, lapic_id: u32) -> bool {
    if cpu_is_online(cpu_id) { return true; }

    // Tenter un IPI wakeup d'abord (CPU en halt léger)
    ipi::send_ipi_wakeup(lapic_id);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(100);
    while tsc::read_tsc() < deadline {
        if cpu_is_online(cpu_id) { return true; }
        tsc::tsc_delay_us(1000); // 1ms poll
    }

    // CPU ne répond pas : séquence boot complète
    ipi::send_init_ipi(lapic_id as u8);
    tsc::tsc_delay_ms(10);
    ipi::send_startup_ipi(lapic_id as u8, super::init::TRAMPOLINE_PAGE);
    tsc::tsc_delay_ms(1);
    ipi::send_startup_ipi(lapic_id as u8, super::init::TRAMPOLINE_PAGE);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(200);
    while tsc::read_tsc() < deadline {
        if cpu_is_online(cpu_id) { return true; }
        tsc::tsc_delay_us(1000);
    }

    false
}

/// Met un CPU hors ligne
///
/// Envoie un IPI HOTPLUG au CPU cible, puis attend sa confirmation.
/// Le CPU cible appelle `hotplug_cpu_halt()` en réponse.
///
/// Retourne `true` si le CPU est offline après 500ms.
pub fn cpu_offline(cpu_id: u32, lapic_id: u32) -> bool {
    if !cpu_is_online(cpu_id) { return true; }
    if cpu_id == 0 { return false; } // BSP ne peut pas se mettre offline

    ipi::send_ipi_cpu_hotplug(lapic_id);

    let deadline = tsc::read_tsc() + tsc::tsc_ms_to_cycles(500);
    while tsc::read_tsc() < deadline {
        if !cpu_is_online(cpu_id) { return true; }
        tsc::tsc_delay_us(1000);
    }

    false
}

/// Handler hotplug appelé par l'AP ciblé lors de la réception de l'IPI
///
/// L'AP se déclare offline et entre en boucle halt.
/// Il peut être réveillé ultérieurement par un IPI wakeup.
pub fn hotplug_cpu_halt(cpu_id: u32) -> ! {
    // Masquer toutes les interruptions locales sauf l'IPI wakeup
    // SAFETY: désactivation interruptions sur ce CPU avant halt
    unsafe { core::arch::asm!("cli", options(nostack, nomem)); }

    set_cpu_offline(cpu_id);

    // Boucle halt (réactivable par IPI wakeup + STI dans le handler)
    loop {
        // SAFETY: halt sûr — seule sortie = NMI ou IPI wakeup (si STI pré-halt)
        unsafe { core::arch::asm!("sti\n\thlt\n\tcli", options(nostack, nomem)); }
    }
}

/// Retourne le masque de bits des CPUs online (64 premiers CPUs)
pub fn online_mask() -> u64 {
    CPU_ONLINE_MASK.load(Ordering::Acquire)
}
