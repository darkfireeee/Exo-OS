// kernel/src/memory/protection/nx.rs
//
// NX (No-Execute) bit management — EFER.NXE activation + application
// stricte par zone mémoire.
//
// Références : Intel SDM Vol.3A § 4.6 — "Access Rights" ; AMD APM Vol.2 § 5.6.
//
// Règles architecture Exo-OS :
//   COUCHE 0 — aucune dépendance vers scheduler/, process/, ipc/, fs/.
//   Tous les accès MSRL passent par `asm!` inline.
//   Pas de std ; spin = "0.9" uniquement pour les locks.


use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory::core::types::PageFlags;
use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::layout::{PHYS_MAP_BASE, PHYS_MAP_END, VMALLOC_BASE, VMALLOC_END, KERNEL_HEAP_START};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes internes
// ─────────────────────────────────────────────────────────────────────────────

/// Bit 11 du MSR_EFER = NXE (No-Execute Enable).
const EFER_NXE_BIT: u64 = 1 << 11;
/// Numéro du MSR EFER.
const MSR_EFER: u32 = 0xC000_0080;
/// Bit 63 d'une entrée de page table = XD/NX.
pub const PAGE_TABLE_NX_BIT: u64 = 1u64 << 63;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques NX
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs opérationnels du sous-système NX.
#[repr(C)]
pub struct NxStats {
    /// Nombre de fois où EFER.NXE a été activé (doit rester à 1 après init).
    pub efer_nxe_enable_count: AtomicU64,
    /// Violations NX capturées (#PF avec bit I/D = 1, execute = 1).
    pub violation_count: AtomicU64,
    /// Pages balisées NX au cours de l'initialisation.
    pub pages_marked_nx: AtomicU64,
    /// Pages dont le flag NX a été levé explicitement (text segments).
    pub pages_cleared_nx: AtomicU64,
    /// Nombre d'appels à `nx_enforce_region`.
    pub region_enforce_calls: AtomicU64,
    /// Tentatives de toggle NX alors qu'EFER.NXE est déjà actif.
    pub redundant_enable: AtomicU64,
}

impl NxStats {
    const fn new() -> Self {
        Self {
            efer_nxe_enable_count: AtomicU64::new(0),
            violation_count:       AtomicU64::new(0),
            pages_marked_nx:       AtomicU64::new(0),
            pages_cleared_nx:      AtomicU64::new(0),
            region_enforce_calls:  AtomicU64::new(0),
            redundant_enable:      AtomicU64::new(0),
        }
    }
}

// SAFETY : tous les champs sont AtomicU64 → safe à partager entre CPUs.
unsafe impl Sync for NxStats {}

pub static NX_STATS: NxStats = NxStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// État du sous-système
// ─────────────────────────────────────────────────────────────────────────────

/// `true` une fois EFER.NXE activé sur le BSP.
static NX_ENABLED: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Politique régionale NX
// ─────────────────────────────────────────────────────────────────────────────

/// Politique NX pour une plage d'adresses virtuelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NxPolicy {
    /// La plage est exécutable (code kernel / module).
    Executable,
    /// La plage est non-exécutable (données, stack, heap, physmap, vmalloc data).
    NonExecutable,
    /// La plage est non-présente / non concernée.
    Absent,
}

/// Règle régionale statique : [start_virt, end_virt) → NxPolicy.
#[derive(Debug, Clone, Copy)]
pub struct NxRegionRule {
    pub start: u64,
    pub end:   u64,
    pub policy: NxPolicy,
}

/// Table de politiques NX compilée statiquement (.rodata).
/// RÈGLE IA-KERNEL-01 : table .rodata, pas de génération runtime.
static NX_REGION_RULES: [NxRegionRule; 6] = [
    // Physmap — données mappées, jamais exécutables.
    NxRegionRule { start: PHYS_MAP_BASE.as_u64(), end: PHYS_MAP_END.as_u64(), policy: NxPolicy::NonExecutable },
    // Vmalloc — allocations dynamiques noyau, données.
    NxRegionRule { start: VMALLOC_BASE.as_u64(),  end: VMALLOC_END.as_u64(), policy: NxPolicy::NonExecutable },
    // Heap noyau — jamais exécutable.
    NxRegionRule { start: KERNEL_HEAP_START.as_u64(), end: KERNEL_HEAP_START.as_u64() + 0x0000_0100_0000_0000, policy: NxPolicy::NonExecutable },
    // Stacks CPU (fixe 64 KiB par CPU, 256 CPUs max = 16 MiB total).
    NxRegionRule { start: 0xFFFF_FF80_0000_0000, end: 0xFFFF_FF80_0100_0000, policy: NxPolicy::NonExecutable },
    // Code noyau (segments .text/.init).
    NxRegionRule { start: 0xFFFF_FFFF_8000_0000, end: 0xFFFF_FFFF_A000_0000, policy: NxPolicy::Executable },
    // Données noyau (.data/.bss/.rodata).
    NxRegionRule { start: 0xFFFF_FFFF_A000_0000, end: 0xFFFF_FFFF_C000_0000, policy: NxPolicy::NonExecutable },
];

// ─────────────────────────────────────────────────────────────────────────────
// Lecture / écriture MSR EFER via asm! inline
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le MSR identifié par `msr`.
///
/// # Safety
/// Doit être exécuté en CPL 0 (ring 0).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Écrit `val` dans le MSR identifié par `msr`.
///
/// # Safety
/// Doit être exécuté en CPL 0.  Modifier EFER mal peut rendre le CPU inutilisable.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Active `EFER.NXE` sur le CPU courant.
///
/// Doit être appelé sur **chaque** CPU (BSP puis APs) avant l'activation des
/// page tables NX.  Idempotent : si déjà activé, incrémente `redundant_enable`.
///
/// # Safety
/// Doit être exécuté en CPL 0 avec les interruptions désactivées si le switch
/// de page table suit immédiatement.
pub unsafe fn enable_nx() {
    let efer = rdmsr(MSR_EFER);
    if efer & EFER_NXE_BIT != 0 {
        NX_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    wrmsr(MSR_EFER, efer | EFER_NXE_BIT);
    NX_STATS.efer_nxe_enable_count.fetch_add(1, Ordering::Relaxed);
    NX_ENABLED.store(true, Ordering::Release);
}

/// Vérifie qu'EFER.NXE est actif sur le CPU courant.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn is_nx_active() -> bool {
    rdmsr(MSR_EFER) & EFER_NXE_BIT != 0
}

/// Retourne `true` si le sous-système NX a été initialisé.
#[inline]
pub fn nx_enabled() -> bool {
    NX_ENABLED.load(Ordering::Acquire)
}

/// Détermine la politique NX d'une adresse virtuelle selon la table régionale.
#[inline]
pub fn nx_policy_for(virt: u64) -> NxPolicy {
    for rule in &NX_REGION_RULES {
        if virt >= rule.start && virt < rule.end {
            return rule.policy;
        }
    }
    // Par défaut : données — non exécutable (secure by default).
    NxPolicy::NonExecutable
}

/// Retourne les flags `PageFlags` appropriés pour l'adresse `virt` :
/// - NX activé sauf si la région est exécutable et NX est actif.
#[inline]
pub fn nx_page_flags(virt: u64, flags: PageFlags) -> PageFlags {
    if !nx_enabled() {
        return flags;
    }
    match nx_policy_for(virt) {
        NxPolicy::NonExecutable => flags | PageFlags::NO_EXECUTE,
        NxPolicy::Executable    => flags & !PageFlags::NO_EXECUTE,
        NxPolicy::Absent        => flags,
    }
}

/// Applique la politique NX sur une plage de pages en page-table walk.
///
/// Cette fonction parcourt [`count`] pages à partir de `base_virt` et force
/// le bit NX selon `NX_REGION_RULES`.  Le caller fournit un closure qui reçoit
/// l'adresse virtuelle et retourne `Option<*mut u64>` pointant sur l'entrée
/// PTE (niveau 1) en mémoire.
///
/// # Safety
/// - Les PTEs doivent être valides et en mémoire non-paginée.
/// - Doit être appelé avec EFER.NXE actif.
pub unsafe fn nx_enforce_region<F>(base_virt: u64, count: usize, pte_resolver: F)
where
    F: Fn(u64) -> Option<*mut u64>,
{
    NX_STATS.region_enforce_calls.fetch_add(1, Ordering::Relaxed);

    for i in 0..count as u64 {
        let vaddr = base_virt + i * PAGE_SIZE as u64;
        if let Some(pte_ptr) = pte_resolver(vaddr) {
            let pte = pte_ptr.read_volatile();
            // Si l'entrée n'est pas présente, skip.
            if pte & 1 == 0 {
                continue;
            }
            let new_pte = match nx_policy_for(vaddr) {
                NxPolicy::NonExecutable => {
                    NX_STATS.pages_marked_nx.fetch_add(1, Ordering::Relaxed);
                    pte | PAGE_TABLE_NX_BIT
                }
                NxPolicy::Executable => {
                    NX_STATS.pages_cleared_nx.fetch_add(1, Ordering::Relaxed);
                    pte & !PAGE_TABLE_NX_BIT
                }
                NxPolicy::Absent => pte,
            };
            if new_pte != pte {
                pte_ptr.write_volatile(new_pte);
                // Invalider TLB pour cette page.
                core::arch::asm!(
                    "invlpg [{addr}]",
                    addr = in(reg) vaddr as usize,
                    options(nostack, nomem, preserves_flags),
                );
            }
        }
    }
}

/// Gestionnaire appelé par le fault handler (#PF) lorsqu'une violation NX
/// est détectée (fault_flags indique un accès exécution sur page NX).
///
/// Incrémente `violation_count` et retourne `false` pour signaler au handler
/// principal qu'il doit déclencher un kernel panic ou tuer le processus.
#[inline]
pub fn nx_handle_violation(fault_virt: u64, fault_ip: u64) -> bool {
    NX_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    // Log (via serial / ring-buffer) sans appel scheduler.
    // Format minimal pour ring 0 pré-console.
    let _ = fault_virt;
    let _ = fault_ip;
    false // non récupérable → caller doit panic!
}

/// Initialisation du sous-système NX.
/// Doit être la première fonction appelée sur le BSP avant le mapping des
/// page tables définitives.
///
/// # Safety
/// CPL 0, interruptions désactivées recommandé.
pub unsafe fn init() {
    enable_nx();
    // Vérification de cohérence post-activation.
    debug_assert!(is_nx_active(), "NX: EFER.NXE non actif après enable_nx()");
}
