// kernel/src/memory/arch_iface.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// INTERFACE MÉMOIRE ↔ ARCHITECTURE — Côté memory/
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module expose les points d'enregistrement que l'architecture (arch/)
// utilise pour s'accrocher au sous-système mémoire au démarrage.
//
// ## Rôle
//
// - Convertit la carte mémoire physique fournie par arch/ boot en appels
//   vers les allocateurs de memory/ (buddy, slab, NUMA).
// - Fournit les types `PhysMemoryRegion` et `PhysRegionType` que arch/ boot
//   utilise pour décrire la RAM physique de façon uniforme (E820 ≡ UEFI).
//
// ## Points d'intégration
//
// `arch/x86_64/boot/memory_map.rs` → `memory::arch_iface::init_from_regions()`
// `arch/x86_64/memory_iface.rs`    → `memory::virt::address_space::tlb::register_tlb_ipi_sender()`
//
// ## Règles architecture (DOC2)
//
//   MEM-01 : memory/ est COUCHE 0 — ne dépend PAS de scheduler/, process/, etc.
//   MEM-02 : EmergencyPool initialisé EN PREMIER (avant cet appel).
//   MEM-04 : free_pages() n'est JAMAIS appelé avant TLB shootdown complet.

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES D'INTÉGRATION
// ─────────────────────────────────────────────────────────────────────────────

/// Vecteur IPI TLB shootdown (0xF2) — doit correspondre à arch/x86_64/idt.
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF2;

/// Vecteur IPI reschedule (0xF1).
pub const IPI_RESCHEDULE_VECTOR: u8 = 0xF1;

/// Nœuds NUMA maximaux supportés.
pub const MAX_NUMA_NODES: usize = 8;

/// CPUs maximaux supportés.
pub const MAX_CPUS: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// TYPES DE RÉGIONS MÉMOIRE
// ─────────────────────────────────────────────────────────────────────────────

/// Décrit une région de mémoire physique fournie par arch/ au boot.
///
/// Construit par `arch/x86_64/boot/memory_map.rs` depuis la E820 (Multiboot2)
/// ou la table UEFI Memory Map. Passé à `init_from_regions()`.
#[derive(Debug, Clone, Copy)]
pub struct PhysMemoryRegion {
    /// Adresse physique de début (alignée sur PAGE_SIZE).
    pub base: u64,
    /// Taille en octets (multiple de PAGE_SIZE).
    pub size: u64,
    /// Type de la région (usable, reserved, acpi...).
    pub region_type: PhysRegionType,
}

impl PhysMemoryRegion {
    /// Adresse physique de fin exclusive.
    #[inline]
    pub fn end(&self) -> u64 {
        self.base.wrapping_add(self.size)
    }

    /// Retourne `true` si c'est de la RAM utilisable par le buddy allocator.
    #[inline]
    pub fn is_usable(&self) -> bool {
        matches!(self.region_type, PhysRegionType::Usable)
    }

    /// Retourne `true` si c'est une région ACPI reclaimable.
    #[inline]
    pub fn is_acpi_reclaimable(&self) -> bool {
        matches!(self.region_type, PhysRegionType::AcpiReclaimable)
    }
}

/// Type de région mémoire physique (fusion des types E820 et UEFI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysRegionType {
    /// RAM conventionnelle — libérable par le buddy allocator.
    Usable,
    /// Réservée par le firmware / matériel — NE PAS toucher.
    Reserved,
    /// ACPI reclaimable — libérable après parsage des tables ACPI.
    AcpiReclaimable,
    /// ACPI NVS — ne jamais libérer ni modifier.
    AcpiNvs,
    /// Mémoire défectueuse (ECC error, BIOS marquée bad).
    Defective,
    /// Firmware runtime (UEFI runtime services) — ne pas toucher.
    FirmwareReserved,
}

// ─────────────────────────────────────────────────────────────────────────────
// INITIALISATION DE MÉMOIRE PHYSIQUE DEPUIS LA CARTE ARCH
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système de mémoire physique depuis les régions
/// détectées par le bootloader (E820 / UEFI).
///
/// Cette fonction est le point d'entrée unifié que `arch/x86_64/boot/memory_map.rs`
/// ou l'initialisation UEFI appelle après avoir analysé la carte mémoire.
///
/// ## Séquence (règle MEM-02 DOC2)
///
/// 1. Détecter `phys_start` / `phys_end` depuis les régions usables.
/// 2. `physical::allocator::init_phase1_bitmap(phys_start, phys_end)`.
/// 3. Pour chaque région usable : `init_phase2_free_region(start, end)`.
/// 4. `init_phase3_slab_slub()`.
/// 5. `init_phase4_numa(active_mask)`.
///
/// ## Prérequis
/// - `EmergencyPool` doit avoir été initialisé AVANT cet appel.
/// - Appelé UNE SEULE FOIS, depuis le BSP, avant le démarrage des APs.
/// - `regions` ne doit PAS être vide.
///
/// # Safety
/// - Mémoire non concurrent (single CPU au boot).
/// - Les plages dans `regions` ne doivent pas se chevaucher.
/// - Les adresses usables doivent être de la RAM physique réelle.
pub unsafe fn init_from_regions(regions: &[PhysMemoryRegion]) {
    use crate::memory::core::{PhysAddr, PAGE_SIZE};
    use crate::memory::physical::allocator::{
        init_phase1_bitmap, init_phase2_free_region, init_phase3_slab_slub, init_phase4_numa,
    };

    let page = PAGE_SIZE as u64;

    // ── Détecter la plage physique totale ─────────────────────────────────────
    let mut phys_start = u64::MAX;
    let mut phys_end = 0u64;

    for region in regions {
        if region.size == 0 {
            continue;
        }
        let base = region.base;
        let end = region.end();
        if base < phys_start {
            phys_start = base;
        }
        if end > phys_end {
            phys_end = end;
        }
    }

    if phys_start == u64::MAX || phys_end == 0 || phys_start >= phys_end {
        // Aucune région détectée — situation non récupérable
        return;
    }

    // Aligner les bornes totales sur PAGE_SIZE
    let ps = PhysAddr::new((phys_start + page - 1) & !(page - 1));
    let pe = PhysAddr::new(phys_end & !(page - 1));

    // ── Phase 1 : bitmap global sur toute la plage détectée ───────────────────
    init_phase1_bitmap(ps, pe);

    // ── Phase 2 : libérer les pages des régions usables ─────────────────────
    for region in regions {
        if !region.is_usable() {
            continue;
        }
        if region.size == 0 {
            continue;
        }

        let base_adj = (region.base + page - 1) & !(page - 1);
        let end_adj = region.end() & !(page - 1);
        if base_adj >= end_adj {
            continue;
        }

        init_phase2_free_region(PhysAddr::new(base_adj), PhysAddr::new(end_adj));
    }

    // ── Phase 3 : slab / slub — caches d'allocation objet ───────────────────
    init_phase3_slab_slub();

    // ── Phase 4 : NUMA — nœud 0 actif par défaut ─────────────────────────────
    // La topologie réelle sera chargée depuis ACPI/MADT après cette séquence.
    init_phase4_numa(0b0000_0001);
}
