// kernel/src/memory/integrity/guard_pages.rs
//
// Guard pages — pages non-présentes insérées autour des stacks et des régions
// heap sensibles pour détecter les débordements au moment de l'accès.
//
// Architecture :
//   - Chaque stack kernel CPU : 1 page garde BASSE (sous-débordement) + 1 page
//     garde HAUTE (surdébordement).
//   - Chaque allocation vmalloc > VMALLOC_GUARD_THRESHOLD : une page garde
//     encadre la région.
//   - Une page garde est une PTE PRESENT=0 avec un tag spécial dans les bits
//     réservés (bits 11:9 = 0b111) pour la distinguer d'une page non-mappée.
//
// Détection : le fault handler appelle `is_guard_page_fault(virt)` pour
//             décider si la #PF est une violation de guard → panic kernel.
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.


use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Tag dans les bits 11:9 d'une PTE marquée guard.
/// bits 11:9 = 0b111 → valeur 7 décalée de 9 bits.
pub const GUARD_PTE_TAG: u64 = 0b111 << 9;
/// Masque des bits 11:9.
const GUARD_PTE_MASK: u64 = 0b111 << 9;
/// Une entrée de page garde : PRESENT=0, tag=0b111. Doit être non-présente.
pub const GUARD_PTE_VALUE: u64 = GUARD_PTE_TAG;

/// Seuil vmalloc au-dessus duquel des pages garde sont insérées.
pub const VMALLOC_GUARD_THRESHOLD: usize = 4 * PAGE_SIZE;

/// Nombre maximal de régions garde suivies.
const MAX_GUARD_REGIONS: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct GuardPageStats {
    pub regions_registered:  AtomicU64,
    pub regions_unregistered: AtomicU64,
    pub violations_detected: AtomicU64,
    pub stack_guards_placed: AtomicU64,
    pub vmalloc_guards_placed: AtomicU64,
    pub false_positives:     AtomicU64,
}

impl GuardPageStats {
    const fn new() -> Self {
        Self {
            regions_registered:   AtomicU64::new(0),
            regions_unregistered: AtomicU64::new(0),
            violations_detected:  AtomicU64::new(0),
            stack_guards_placed:  AtomicU64::new(0),
            vmalloc_guards_placed: AtomicU64::new(0),
            false_positives:      AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for GuardPageStats {}
pub static GUARD_STATS: GuardPageStats = GuardPageStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Annuaire des régions garde
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'une région garde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardRegionId(pub u32);

/// Type de région gardée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRegionKind {
    /// Stack d'un CPU (kernel).
    KernelStack { cpu_id: u32 },
    /// Stack d'un thread user.
    UserStack { tid: u64 },
    /// Allocation vmalloc.
    Vmalloc,
    /// Région générique (heap, zone MMIO, etc.).
    Generic,
}

/// Descripteur d'une région garde.
#[derive(Debug, Clone, Copy)]
pub struct GuardRegion {
    /// Adresse virtuelle de début de la page garde inférieure.
    pub low_guard:  u64,
    /// Adresse virtuelle de début de la page garde supérieure.
    pub high_guard: u64,
    /// Adresse de la région protégée (pour diagnostics).
    pub region_start: u64,
    pub region_size:  u64,
    pub kind: GuardRegionKind,
    pub active: bool,
}

impl GuardRegion {
    #[allow(dead_code)]
    const fn inactive() -> Self {
        Self {
            low_guard:    0,
            high_guard:   0,
            region_start: 0,
            region_size:  0,
            kind: GuardRegionKind::Generic,
            active: false,
        }
    }
}

struct GuardRegionTable {
    regions: [GuardRegion; MAX_GUARD_REGIONS],
    count:   usize,
}

impl GuardRegionTable {
    const fn new() -> Self {
        Self {
            regions: [GuardRegion {
                low_guard: 0, high_guard: 0, region_start: 0, region_size: 0,
                kind: GuardRegionKind::Generic, active: false,
            }; MAX_GUARD_REGIONS],
            count: 0,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        for i in 0..MAX_GUARD_REGIONS {
            if !self.regions[i].active {
                return Some(i);
            }
        }
        None
    }

    fn find_by_id(&mut self, id: GuardRegionId) -> Option<&mut GuardRegion> {
        self.regions.get_mut(id.0 as usize)
    }

    /// Recherche si `virt` tombe dans une page garde enregistrée.
    fn lookup(&self, virt: u64) -> Option<usize> {
        for i in 0..MAX_GUARD_REGIONS {
            let r = &self.regions[i];
            if !r.active {
                continue;
            }
            let low_end  = r.low_guard  + PAGE_SIZE as u64;
            let high_end = r.high_guard + PAGE_SIZE as u64;
            if (virt >= r.low_guard && virt < low_end)
                || (virt >= r.high_guard && virt < high_end)
            {
                return Some(i);
            }
        }
        None
    }
}

static GUARD_TABLE: Mutex<GuardRegionTable> = Mutex::new(GuardRegionTable::new());

// ─────────────────────────────────────────────────────────────────────────────
// Manipulation de PTEs pour les pages garde
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit une PTE guard (non-présente avec tag) dans `pte_ptr`.
///
/// # Safety
/// `pte_ptr` doit pointer vers l'entrée PTE niveau 1 valide (non OOM).
pub unsafe fn write_guard_pte(pte_ptr: *mut u64) {
    pte_ptr.write_volatile(GUARD_PTE_VALUE);
    // Invalider le TLB pour cette entrée.
    // On ne connaît pas l'adresse virtuelle ici ; le caller doit appeler
    // invlpg sur les pages concercées.
}

/// Retourne `true` si `pte` est une entrée guard (non-présente + tag correct).
#[inline]
pub fn is_guard_pte(pte: u64) -> bool {
    // PRESENT = 0 ET bits 11:9 = 0b111.
    (pte & 1 == 0) && (pte & GUARD_PTE_MASK == GUARD_PTE_TAG)
}

/// Efface une PTE guard (la remet à 0).
///
/// # Safety : `pte_ptr` doit être valide.
pub unsafe fn clear_guard_pte(pte_ptr: *mut u64) {
    pte_ptr.write_volatile(0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Enregistrement de régions garde
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre une région garde encadrant [`region_start`, `region_start + region_size`).
///
/// Les pages garde sont placées immédiatement AVANT `region_start` (garde basse)
/// et immédiatement APRÈS `region_start + region_size` (garde haute).
///
/// Le caller est responsable d'appeler le page table walker pour écrire les PTEs.
/// Cette fonction ne touche pas les PTEs ; elle maintient seulement l'annuaire.
///
/// Retourne `Some(GuardRegionId)` ou `None` si table pleine.
pub fn register_guard_region(
    region_start: u64,
    region_size:  u64,
    kind:         GuardRegionKind,
) -> Option<GuardRegionId> {
    let mut table = GUARD_TABLE.lock();
    let slot = table.alloc_slot()?;

    let low_guard  = region_start.wrapping_sub(PAGE_SIZE as u64);
    let high_guard = region_start + region_size;

    table.regions[slot] = GuardRegion {
        low_guard,
        high_guard,
        region_start,
        region_size,
        kind,
        active: true,
    };
    table.count += 1;
    GUARD_STATS.regions_registered.fetch_add(1, Ordering::Relaxed);

    match kind {
        GuardRegionKind::KernelStack { .. } | GuardRegionKind::UserStack { .. } =>
            GUARD_STATS.stack_guards_placed.fetch_add(1, Ordering::Relaxed),
        GuardRegionKind::Vmalloc =>
            GUARD_STATS.vmalloc_guards_placed.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };

    Some(GuardRegionId(slot as u32))
}

/// Désenregistre une région garde par son ID.
/// Le caller doit nettoyer les PTEs correspondantes.
pub fn unregister_guard_region(id: GuardRegionId) -> bool {
    let mut table = GUARD_TABLE.lock();
    if let Some(region) = table.find_by_id(id) {
        if region.active {
            region.active = false;
            table.count -= 1;
            GUARD_STATS.regions_unregistered.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Détection au moment du page fault
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une vérification guard au fault handler.
#[derive(Debug, Clone, Copy)]
pub enum GuardFaultResult {
    /// Page guard confirmée → kernel panic.
    GuardViolation { region_idx: usize, kind: GuardRegionKind },
    /// Pas une page guard → fault normal.
    NotGuard,
}

/// Appelé par le fault handler pour toute #PF.
/// Si la page fautive est une page garde → `GuardViolation`.
///
/// `fault_virt` : CR2 (adresse linéaire fautive).
/// `pte_value`  : valeur PTE lue depuis la page table (0 si absence).
pub fn check_guard_fault(fault_virt: u64, pte_value: u64) -> GuardFaultResult {
    // Vérification rapide sur le tag PTE.
    if pte_value != 0 && !is_guard_pte(pte_value) {
        return GuardFaultResult::NotGuard;
    }
    let table = GUARD_TABLE.lock();
    if let Some(idx) = table.lookup(fault_virt) {
        let kind = table.regions[idx].kind;
        drop(table);
        GUARD_STATS.violations_detected.fetch_add(1, Ordering::Relaxed);
        GuardFaultResult::GuardViolation { region_idx: idx, kind }
    } else {
        GuardFaultResult::NotGuard
    }
}

/// Gestionnaire de violation de page garde — déclenche un kernel panic.
pub fn guard_page_violation_handler(virt: u64, kind: GuardRegionKind) -> ! {
    GUARD_STATS.violations_detected.fetch_add(1, Ordering::Relaxed);
    match kind {
        GuardRegionKind::KernelStack { cpu_id } =>
            panic!("GUARD PAGE VIOLATION: kernel stack overflow cpu={} virt={:#x}", cpu_id, virt),
        GuardRegionKind::UserStack { tid } =>
            panic!("GUARD PAGE VIOLATION: user stack overflow tid={} virt={:#x}", tid, virt),
        GuardRegionKind::Vmalloc =>
            panic!("GUARD PAGE VIOLATION: vmalloc overflow virt={:#x}", virt),
        GuardRegionKind::Generic =>
            panic!("GUARD PAGE VIOLATION: generic region virt={:#x}", virt),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers stacks CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse canonique d'une stack kernel pour `cpu_id`.
/// Convention : stacks à 0xFFFF_FF80_0000_0000 + cpu_id × 64 KiB.
const KERNEL_STACK_BASE: u64 = 0xFFFF_FF80_0000_0000;
const KERNEL_STACK_SIZE: u64 = 64 * 1024; // 64 KiB par CPU

/// Retourne (stack_base, stack_size) pour `cpu_id`.
pub fn cpu_stack_range(cpu_id: u32) -> (u64, u64) {
    let base = KERNEL_STACK_BASE + cpu_id as u64 * KERNEL_STACK_SIZE;
    (base, KERNEL_STACK_SIZE)
}

/// Enregistre les pages garde pour la stack du CPU `cpu_id`.
/// Returns `GuardRegionId` ou `None` si table pleine.
pub fn register_cpu_stack_guards(cpu_id: u32) -> Option<GuardRegionId> {
    let (base, size) = cpu_stack_range(cpu_id);
    register_guard_region(base, size, GuardRegionKind::KernelStack { cpu_id })
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init guard pages — enregistre les pages garde pour le BSP (cpu_id=0).
pub fn init() {
    register_cpu_stack_guards(0);
}
