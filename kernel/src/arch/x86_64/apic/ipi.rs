//! # arch/x86_64/apic/ipi.rs — Inter-Processor Interrupts
//!
//! Envoi d'IPIs vers les autres CPU pour la synchronisation SMP.
//!
//! ## Vecteurs IPI (définis dans idt.rs)
//! - 0xF0 `VEC_IPI_WAKEUP`         : reveiller un thread sur un CPU
//! - 0xF1 `VEC_IPI_RESCHEDULE`     : forcer un reschedule
//! - 0xF2 `VEC_IPI_TLB_SHOOTDOWN`  : invalider une page TLB
//! - 0xF3 `VEC_IPI_CPU_HOTPLUG`    : notification hotplug CPU
//! - 0xFE `VEC_IPI_PANIC`          : arrêt d'urgence (broadcast)
//!
//! ## Routage
//! Délégué vers x2APIC (MSR ICR) ou xAPIC (MMIO ICR) selon le mode actif.


use core::sync::atomic::{AtomicU64, Ordering};
use super::super::idt::{
    VEC_IPI_WAKEUP, VEC_IPI_RESCHEDULE, VEC_IPI_TLB_SHOOTDOWN,
    VEC_IPI_CPU_HOTPLUG, VEC_IPI_PANIC,
};

// ── TLB Shootdown payload ─────────────────────────────────────────────────────

/// Adresse virtuelle cible du TLB shootdown en cours (0 = flush total)
static TLB_SHOOTDOWN_ADDR: AtomicU64 = AtomicU64::new(0);
/// Compteur de CPUs ayant acquitté le TLB shootdown courant
static TLB_SHOOTDOWN_ACK:  AtomicU64 = AtomicU64::new(0);
/// Génération du TLB shootdown (anti-spurious)
static TLB_SHOOTDOWN_GEN:  AtomicU64 = AtomicU64::new(0);

// ── Envoi IPI générique ───────────────────────────────────────────────────────

/// Envoie un IPI vers un APIC ID spécifique (auto-sélection xAPIC/x2APIC)
#[inline]
fn send_ipi_to(dest_apic_id: u32, vector: u8) {
    if super::is_x2apic() {
        super::x2apic::send_ipi_x2apic(dest_apic_id, vector, 0);
    } else {
        super::local_apic::send_ipi(dest_apic_id as u8, vector, super::local_apic::ICR_DM_FIXED);
    }
}

/// Broadcast IPI vers tous les CPUs SAUF soi-même
#[inline]
fn broadcast_ipi_except_self(vector: u8) {
    if super::is_x2apic() {
        super::x2apic::broadcast_ipi_except_self_x2apic(vector);
    } else {
        super::local_apic::broadcast_ipi_except_self(vector);
    }
}

// ── IPIs spécialisés ──────────────────────────────────────────────────────────

/// Envoie un IPI wakeup vers un CPU cible (pour réveiller un thread)
///
/// Utilisé par `scheduler::smp::wakeup::wake_cpu()`.
pub fn send_ipi_wakeup(dest_apic_id: u32) {
    IPI_WAKEUP_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_WAKEUP);
}

/// Envoie un IPI reschedule vers un CPU cible (préemption SMP)
///
/// Utilisé par `scheduler::smp::preempt::kick_cpu()`.
pub fn send_ipi_reschedule(dest_apic_id: u32) {
    IPI_RESCHEDULE_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_RESCHEDULE);
}

/// Envoie un IPI TLB shootdown vers un CPU cible
///
/// - `virt_addr = 0` : flush TLB complet (`INVLPG` sur tous les CPUs)
/// - `virt_addr ≠ 0` : flush d'une page spécifique
///
/// Appelé par `memory::virtual::address_space::tlb::shootdown_page()`.
pub fn send_ipi_tlb_shootdown(dest_apic_id: u32, virt_addr: u64) {
    TLB_SHOOTDOWN_ADDR.store(virt_addr, Ordering::Release);
    TLB_SHOOTDOWN_GEN.fetch_add(1, Ordering::AcqRel);
    IPI_TLB_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_TLB_SHOOTDOWN);
}

/// Broadcast TLB shootdown vers tous les CPUs SAUF soi-même
///
/// Attend le retour ACK de tous les CPUs (timeout 100µs par défaut).
pub fn broadcast_tlb_shootdown(virt_addr: u64, cpu_count: u32) {
    TLB_SHOOTDOWN_ADDR.store(virt_addr, Ordering::Release);
    TLB_SHOOTDOWN_ACK.store(0, Ordering::Release);
    let gen = TLB_SHOOTDOWN_GEN.fetch_add(1, Ordering::AcqRel) + 1;
    IPI_TLB_SENT.fetch_add(1, Ordering::Relaxed);

    broadcast_ipi_except_self(VEC_IPI_TLB_SHOOTDOWN);

    // Attendre ACK de (cpu_count - 1) CPU distants
    let expected = cpu_count.saturating_sub(1) as u64;
    let deadline = super::super::cpu::tsc::read_tsc()
        + super::super::cpu::tsc::tsc_us_to_cycles(100);

    while TLB_SHOOTDOWN_ACK.load(Ordering::Acquire) < expected {
        if super::super::cpu::tsc::read_tsc() > deadline {
            // Timeout : continuer quand même (certains CPUs peuvent être en halt)
            break;
        }
        core::hint::spin_loop();
    }
    let _ = gen;
}

/// Acquitte un TLB shootdown reçu (appelé par le handler IPI dans exceptions.rs)
#[inline]
pub fn ack_tlb_shootdown() {
    TLB_SHOOTDOWN_ACK.fetch_add(1, Ordering::Release);
}

/// Retourne l'adresse du TLB shootdown en cours
#[inline]
pub fn tlb_shootdown_addr() -> u64 {
    TLB_SHOOTDOWN_ADDR.load(Ordering::Acquire)
}

/// Envoie un IPI hotplug CPU
///
/// Utilisé pour notifier un CPU d'une mise hors ligne (online/offline).
pub fn send_ipi_cpu_hotplug(dest_apic_id: u32) {
    IPI_HOTPLUG_SENT.fetch_add(1, Ordering::Relaxed);
    send_ipi_to(dest_apic_id, VEC_IPI_CPU_HOTPLUG);
}

/// Broadcast IPI panic vers tous les CPUs (arrêt d'urgence)
///
/// Appelé lors d'un kernel panic pour stopper tous les APs immédiatement.
/// **Ne PAS appeler depuis un handler IPI.**
pub fn broadcast_ipi_panic() {
    IPI_PANIC_SENT.fetch_add(1, Ordering::Relaxed);
    broadcast_ipi_except_self(VEC_IPI_PANIC);
}

/// Envoi INIT IPI vers un AP (séquence SMP startup)
///
/// Délégué à `local_apic::send_init_ipi()`.
pub fn send_init_ipi(dest_apic_id: u8) {
    super::local_apic::send_init_ipi(dest_apic_id);
}

/// Envoi STARTUP IPI (SIPI) vers un AP
///
/// `page` = numéro de page 4K du vecteur de démarrage (trampoline).
/// Ex : trampoline à 0x6000 → page = 6.
pub fn send_startup_ipi(dest_apic_id: u8, trampoline_page: u8) {
    super::local_apic::send_startup_ipi(dest_apic_id, trampoline_page);
}

// ── Instrumentations ──────────────────────────────────────────────────────────

static IPI_WAKEUP_SENT:     AtomicU64 = AtomicU64::new(0);
static IPI_RESCHEDULE_SENT: AtomicU64 = AtomicU64::new(0);
static IPI_TLB_SENT:        AtomicU64 = AtomicU64::new(0);
static IPI_HOTPLUG_SENT:    AtomicU64 = AtomicU64::new(0);
static IPI_PANIC_SENT:      AtomicU64 = AtomicU64::new(0);

pub fn ipi_wakeup_sent()     -> u64 { IPI_WAKEUP_SENT.load(Ordering::Relaxed) }
pub fn ipi_reschedule_sent() -> u64 { IPI_RESCHEDULE_SENT.load(Ordering::Relaxed) }
pub fn ipi_tlb_sent()        -> u64 { IPI_TLB_SENT.load(Ordering::Relaxed) }
pub fn ipi_hotplug_sent()    -> u64 { IPI_HOTPLUG_SENT.load(Ordering::Relaxed) }
pub fn ipi_panic_sent()      -> u64 { IPI_PANIC_SENT.load(Ordering::Relaxed) }
