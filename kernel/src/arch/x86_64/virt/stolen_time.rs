//! # arch/x86_64/virt/stolen_time.rs — Temps volé par l'hyperviseur
//!
//! Le "stolen time" est le temps CPU consommé par l'hyperviseur ou par d'autres
//! VMs pendant que cette VM était supposément en exécution.
//!
//! ## KVM Steal Time
//! KVM implémente via une structure partagée par page (MSR_KVM_STEAL_TIME).
//! La VM lit `steal` (u64, nanoseconds cumulées) pour estimer l'overhead hyperviseur.


use core::sync::atomic::{AtomicU64, Ordering};
use super::super::cpu::msr;

/// MSR KVM Steal Time
const MSR_KVM_STEAL_TIME: u32 = 0x4b564d03;
const KVM_STEAL_TIME_ENABLE: u64 = 1;

/// Structure KVM Steal Time (partagée entre le host et le guest)
#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct KvmStealTime {
    pub steal:    u64,    // temps volé en ns (accumulé)
    pub version:  u32,    // incrémenté par le host avant/après update (odd = update en cours)
    pub flags:    u32,
    pub preempted:u8,     // 1 si le vCPU est préempté par le host
    _pad:         [u8; 47],
}

impl KvmStealTime {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        Self { steal: 0, version: 0, flags: 0, preempted: 0, _pad: [0u8; 47] }
    }
}

/// Per-CPU steal time structures (une par CPU)
const MAX_CPUS: usize = 512;

#[repr(align(64))]
struct StealTimeTable([KvmStealTime; MAX_CPUS]);
unsafe impl Sync for StealTimeTable {}

static STEAL_TIME_TABLE: StealTimeTable = StealTimeTable(
    // SAFETY: KvmStealTime est composé de types primitifs, tous-zéros est valide
    unsafe { core::mem::transmute([0u8; core::mem::size_of::<[KvmStealTime; MAX_CPUS]>()]) }
);

static TOTAL_STOLEN_NS: AtomicU64 = AtomicU64::new(0);
static LAST_STEAL_NS:   AtomicU64 = AtomicU64::new(0);

// ── Initialisation ────────────────────────────────────────────────────────────

/// Enregistre la page steal time pour le CPU `cpu_id` auprès de KVM
///
/// Appelé depuis `init_percpu_for_bsp/ap()` si KVM steal time est disponible.
pub fn init_steal_time_for_cpu(cpu_id: u32) {
    use super::detect::{hypervisor_type, HypervisorType, kvm_has_steal_time};
    if !(hypervisor_type() == HypervisorType::Kvm && kvm_has_steal_time()) { return; }
    if cpu_id as usize >= MAX_CPUS { return; }

    let phys_addr = &STEAL_TIME_TABLE.0[cpu_id as usize] as *const KvmStealTime as u64;
    // L'adresse doit être physique et alignée 64 octets (garantie par #[repr(align(64))])
    // SAFETY: MSR_KVM_STEAL_TIME write depuis Ring 0 en mode KVM
    unsafe { msr::write_msr(MSR_KVM_STEAL_TIME, phys_addr | KVM_STEAL_TIME_ENABLE); }
}

// ── Lecture ───────────────────────────────────────────────────────────────────

/// Lit la valeur de steal time en nanosecondes pour le CPU `cpu_id`
///
/// Utilise une lecture atomique stable (version field pair = données cohérentes).
pub fn read_steal_ns(cpu_id: u32) -> u64 {
    if cpu_id as usize >= MAX_CPUS { return 0; }
    let st = &STEAL_TIME_TABLE.0[cpu_id as usize];

    // Lire version paire (host a terminé l'update)
    loop {
        // SAFETY: lecture volatile de la structure partagée
        let ver_before = unsafe { core::ptr::read_volatile(&st.version) };
        if ver_before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        // SAFETY: même structure KVM partagée, seqlock : version paire garantit données cohérentes.
        let steal = unsafe { core::ptr::read_volatile(&st.steal) };
        let ver_after = unsafe { core::ptr::read_volatile(&st.version) };
        if ver_before == ver_after { return steal; }
    }
}

/// Met à jour le compteur total de temps volé (appelé depuis le timer tick)
pub fn update_stolen_time(cpu_id: u32) {
    let current = read_steal_ns(cpu_id);
    let prev = LAST_STEAL_NS.swap(current, Ordering::AcqRel);
    if current > prev {
        TOTAL_STOLEN_NS.fetch_add(current - prev, Ordering::Relaxed);
    }
}

/// Retourne le temps total volé en nanosecondes (depuis le boot)
pub fn stolen_time_ns() -> u64 {
    TOTAL_STOLEN_NS.load(Ordering::Relaxed)
}

/// Retourne `true` si le vCPU courant est actuellement préempté par l'hyperviseur
pub fn is_preempted(cpu_id: u32) -> bool {
    if cpu_id as usize >= MAX_CPUS { return false; }
    // SAFETY: lecture volatile du champ preempted
    unsafe { core::ptr::read_volatile(&STEAL_TIME_TABLE.0[cpu_id as usize].preempted) != 0 }
}
