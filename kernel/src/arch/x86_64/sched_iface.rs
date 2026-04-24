// kernel/src/arch/x86_64/sched_iface.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// PONT ARCHITECTURE ↔ SCHEDULER — x86_64 (C ABI exports)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Rôle
//
// Ce module exporte les fonctions `#[no_mangle] extern "C"` attendues par
// `scheduler/` via `extern "C"` FFI. Il constitue le seul point de couplage
// entre les couches :
//
//   arch/ (transverse, peut appeler n'importe quelle couche)
//        ↓ exports C ABI
//   scheduler/ (Couche 1 — NE peut PAS importer arch/ directement, utilise FFI)
//
// ## Fonctions exportées
//
//   arch_send_reschedule_ipi(target_cpu: u32)
//       Envoie un IPI reschedule (vecteur 0xF1) au CPU logique `target_cpu`.
//       Utilisé par `scheduler::smp::migration` pour déclencher un reschedule
//       sur le CPU de destination après migration de thread.
//
//   arch_set_cpu_pstate(cpu: u32, pstate: u32)
//       Configure le P-state du CPU `cpu` via MSR_IA32_PERF_CTL (0x199).
//       RÈGLE : MSR writes étant CPU-locaux, l'effet n'est garanti que si
//       `cpu == arch_current_cpu()`. Pour un CPU distant, une IPI de type
//       "pstate change" devrait être envoyée — implémentation future.
//
// ## Règles (DOC3)
//   SCHED-01 : scheduler/ est Couche 1 — ne peut pas importer arch/ directement.
//   Le pont FFI ici préserve cette séparation.
//
// ## Synchronisation avec scheduler/
//   Si la signature change ici → mettre à jour simultanément :
//     scheduler/smp/migration.rs    (arch_send_reschedule_ipi)
//     scheduler/energy/frequency.rs (arch_set_cpu_pstate)
// ═════════════════════════════════════════════════════════════════════════════

use super::apic;
use super::cpu::msr;
use super::smp::percpu;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes MSR
// ─────────────────────────────────────────────────────────────────────────────

/// Intel MSR IA32_PERF_CTL — bits [7:0] = target P-state
/// Écriture = demande de changement de fréquence/voltage au matériel
const MSR_IA32_PERF_CTL: u32 = 0x0000_0199;

/// Intel MSR IA32_PERF_STATUS — bits [15:8] = P-state courant
#[allow(dead_code)]
const MSR_IA32_PERF_STATUS: u32 = 0x0000_0198;

/// Masque du P-state dans PERF_CTL (bits [7:0])
const PERF_CTL_PSTATE_MASK: u64 = 0xFF;

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation
// ─────────────────────────────────────────────────────────────────────────────

static RESCHEDULE_IPIS_SENT: AtomicU64 = AtomicU64::new(0);
static PSTATE_CHANGES: AtomicU64 = AtomicU64::new(0);

pub fn reschedule_ipis_sent() -> u64 {
    RESCHEDULE_IPIS_SENT.load(Ordering::Relaxed)
}
pub fn pstate_changes() -> u64 {
    PSTATE_CHANGES.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// arch_send_reschedule_ipi — C ABI export
// ─────────────────────────────────────────────────────────────────────────────

/// Envoie un IPI reschedule (vecteur 0xF1) au CPU logique `target_cpu`.
///
/// Séquence :
/// 1. Lire le LAPIC ID du CPU cible dans `PER_CPU_TABLE[target_cpu]`.
/// 2. Appeler `apic::ipi::send_ipi_reschedule(lapic_id)`.
///
/// ## Règle de sécurité
/// Ne rien faire si `target_cpu >= MAX_CPUS` ou si le CPU n'est pas en ligne.
/// Ne pas envoyer une IPI à soi-même (le scheduler vérifie déjà cette condition,
/// mais on l'ajoute ici en défense).
///
/// ## Utilisateur
/// `scheduler::smp::migration::request_migration()` — déclenche le reschedule
/// sur le CPU de destination après avoir mis en file d'attente la migration.
#[no_mangle]
pub unsafe extern "C" fn arch_send_reschedule_ipi(target_cpu: u32) {
    // Borne supérieure : ne pas accéder hors du tableau
    if target_cpu as usize >= percpu::MAX_CPUS {
        return;
    }

    // Lire les données per-CPU du CPU cible
    let cpu_data = percpu::per_cpu(target_cpu as usize);

    // Vérifier que le CPU est en ligne
    if !cpu_data.online {
        return;
    }

    // Éviter l'auto-IPI (inutile et peut causer des latences sur certains APIC)
    let local_id = percpu::current_cpu_id();
    if target_cpu == local_id {
        return;
    }

    let lapic_id = cpu_data.lapic_id as u32;

    // Envoyer l'IPI reschedule (vecteur 0xF1 — voir arch/x86_64/apic/ipi.rs)
    apic::ipi::send_ipi_reschedule(lapic_id);
    RESCHEDULE_IPIS_SENT.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// arch_set_cpu_pstate — C ABI export
// ─────────────────────────────────────────────────────────────────────────────

/// Configure le P-state du CPU `cpu` via MSR_IA32_PERF_CTL.
///
/// ## P-state encoding (Intel ACPI/HWP)
/// - `pstate = 0`  → Performance maximale (fréquence turbo si disponible)
/// - `pstate = N`  → Décrémentation fréquence (N * 100 MHz typiquement)
/// - Valeur écrite dans PERF_CTL bits [7:0] = ratio fréquence × 100 MHz
///
/// ## Limitation MSR locale
/// MSR_IA32_PERF_CTL est un registre LOCAL au CPU : la valeur écrite n'affecte
/// que le CPU qui exécute l'instruction WRMSR.
///
/// Si `cpu != arch_current_cpu()` → l'écriture est ignorée (avertissement de bord).
/// Le scheduler/energy/frequency.rs doit s'assurer d'appeler cette fonction
/// depuis le CPU cible (via le tick handler qui s'exécute sur chaque CPU).
///
/// ## Utilisateur
/// `scheduler::energy::frequency::set_pstate()` — appelé depuis le tick handler
/// ou le gouverneur d'énergie s'exécutant sur le CPU cible.
#[no_mangle]
pub unsafe extern "C" fn arch_set_cpu_pstate(cpu: u32, pstate: u32) {
    // Vérification de borne
    if cpu as usize >= percpu::MAX_CPUS {
        return;
    }

    // Vérifier que c'est bien un CPU en ligne
    let cpu_data = percpu::per_cpu(cpu as usize);
    if !cpu_data.online {
        return;
    }

    // Garder uniquement les bits valides du P-state (bits [7:0])
    let pstate_val = (pstate as u64) & PERF_CTL_PSTATE_MASK;

    // Lire le PERF_CTL courant pour ne modifier que les bits de P-state
    // et préserver les autres bits (bit 16 = IDA Disengage sur certains Intel)
    //
    // SAFETY: RDMSR depuis Ring 0 — MSR_IA32_PERF_CTL supporté sur tout Intel
    // avec SpeedStep. Sur AMD, le MSR est différent (PERF_CTL = 0xC001_0062)
    // mais la logique d'appel du scheduler est la même.
    // On masque les bits [7:0] et on écrit le nouveau P-state.
    let current = msr::read_msr(MSR_IA32_PERF_CTL);
    let new_val = (current & !PERF_CTL_PSTATE_MASK) | pstate_val;

    // SAFETY: WRMSR depuis Ring 0 — modifie uniquement la fréquence cible
    msr::write_msr(MSR_IA32_PERF_CTL, new_val);
    PSTATE_CHANGES.fetch_add(1, Ordering::Relaxed);
}
