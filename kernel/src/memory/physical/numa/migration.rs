// kernel/src/memory/physical/numa/migration.rs
//
// Migration de pages NUMA — déplace les frames entre nœuds pour améliorer
// la localité des accès.
//
// Algorithme :
//   1. Allouer une frame destination sur le nœud cible.
//   2. Copier le contenu de la frame source (via physmap).
//   3. Mettre à jour la PTE de la VMA concernée (atomic TLB shootdown).
//   4. Libérer la frame source.
//
// Le caller fournit le contexte d'adresse (page table root) via trait.
// Pas d'appel direct au scheduler (couche 0).
//
// Couche 0 — aucune dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::types::{Frame, PhysAddr, AllocFlags};
use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::address::phys_to_virt;
use crate::memory::physical::allocator::buddy::alloc_pages;
use crate::memory::physical::allocator::buddy::free_pages;
use super::node::{NUMA_NODES, NUMA_NODE_INVALID, NumaNode};

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct MigrationStats {
    pub migrations_attempted: AtomicU64,
    pub migrations_succeeded: AtomicU64,
    pub migrations_failed:    AtomicU64,
    /// Nombre de pages déplacées entre nœuds distincts.
    pub pages_moved:          AtomicU64,
    /// Optimisations : page déjà sur le bon nœud.
    pub already_local:        AtomicU64,
    /// Échecs d'allocation sur le nœud cible.
    pub alloc_failures:       AtomicU64,
}

impl MigrationStats {
    const fn new() -> Self {
        Self {
            migrations_attempted: AtomicU64::new(0),
            migrations_succeeded: AtomicU64::new(0),
            migrations_failed:    AtomicU64::new(0),
            pages_moved:          AtomicU64::new(0),
            already_local:        AtomicU64::new(0),
            alloc_failures:       AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for MigrationStats {}
pub static MIGRATION_STATS: MigrationStats = MigrationStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Trait d'injection page-table — évite dépendance vers virtual/
// ─────────────────────────────────────────────────────────────────────────────

/// Trait implémenté par `virtual::address_space::user::UserAddressSpace`.
/// memory/ définit l'interface, virtual/ l'implémente.
pub trait MigrationPageTableOps {
    /// Recherche et retourne la PTE courante pour `virt_addr` dans ce context.
    /// Retourne `None` si non mappé.
    fn get_pte(&self, virt_addr: u64) -> Option<u64>;

    /// Remplace atomiquement la PTE pour `virt_addr` par `new_pte`.
    /// Retourne `Ok(old_pte)` ou `Err(())` si impossible.
    fn swap_pte(&self, virt_addr: u64, new_pte: u64) -> Result<u64, ()>;

    /// Port de TLB shootdown pour cette zone d'adressage.
    fn flush_tlb(&self, virt_addr: u64);
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat de migration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateResult {
    /// Migration réussie — `new_frame` est la frame destination.
    Success { new_frame: Frame },
    /// Page déjà sur le bon nœud.
    AlreadyLocal,
    /// Allocation échouée sur le nœud cible.
    AllocFailed,
    /// Impossible de mettre à jour la PTE.
    PteUpdateFailed,
    /// Paramètres invalides.
    InvalidArgs,
}

// ─────────────────────────────────────────────────────────────────────────────
// Copie de frame via physmap
// ─────────────────────────────────────────────────────────────────────────────

/// Copie exactement une frame (PAGE_SIZE octets) de `src_frame` vers `dst_frame`
/// en passant par le physmap noyau.
///
/// # Safety
/// Les deux frames doivent être valides et non partagées (pas d'alias actif).
unsafe fn copy_frame(src: Frame, dst: Frame) {
    let src_virt = phys_to_virt(PhysAddr::new(src.start_address().as_u64())).as_ptr::<u8>();
    let dst_virt = phys_to_virt(PhysAddr::new(dst.start_address().as_u64())).as_mut_ptr::<u8>();
    core::ptr::copy_nonoverlapping(src_virt, dst_virt, PAGE_SIZE);
}

// ─────────────────────────────────────────────────────────────────────────────
// Nœud d'une frame
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nœud NUMA propriétaire de `frame`.
#[inline]
pub fn frame_node(frame: Frame) -> u32 {
    NUMA_NODES.node_for_phys(frame.start_address().as_u64())
}

// ─────────────────────────────────────────────────────────────────────────────
// migrate_page — déplace une frame vers `target_node`
// ─────────────────────────────────────────────────────────────────────────────

/// Migre le contenu de `src_frame` vers un frame allouée sur `target_node`.
///
/// Si `pt_ops` est `Some`, met à jour la PTE correspondant à `virt_addr` dans
/// l'espace d'adressage fourni et flush le TLB.
///
/// # Safety
/// - La frame source ne doit pas être accédée pendant la copie.
/// - La PTE doit être verrouillée / protégée par le caller.
pub unsafe fn migrate_page<P: MigrationPageTableOps>(
    src_frame:   Frame,
    target_node: u32,
    virt_addr:   Option<u64>,
    pt_ops:      Option<&P>,
) -> MigrateResult {
    MIGRATION_STATS.migrations_attempted.fetch_add(1, Ordering::Relaxed);

    // Vérifier si déjà local.
    let src_node = frame_node(src_frame);
    if src_node == target_node {
        MIGRATION_STATS.already_local.fetch_add(1, Ordering::Relaxed);
        return MigrateResult::AlreadyLocal;
    }

    // Allouer frame destination sur target_node.
    let dst_frame = match alloc_pages_on_node(target_node, 0, AllocFlags::NONE) {
        Some(f) => f,
        None => {
            MIGRATION_STATS.alloc_failures.fetch_add(1, Ordering::Relaxed);
            MIGRATION_STATS.migrations_failed.fetch_add(1, Ordering::Relaxed);
            return MigrateResult::AllocFailed;
        }
    };

    // Copier le contenu.
    copy_frame(src_frame, dst_frame);

    // Mettre à jour la PTE si contexte fourni.
    if let (Some(virt), Some(ops)) = (virt_addr, pt_ops) {
        // Construire la nouvelle PTE avec les mêmes flags que l'ancienne.
        if let Some(old_pte) = ops.get_pte(virt) {
            // Masque des flags : conserver tous les bits sauf la pfn.
            const PFN_MASK: u64 = !0x0FFF_FFFF_FFFF_F000; // bits 11:0 + 63:48
            let flags   = old_pte & PFN_MASK;
            let new_pfn = (dst_frame.start_address().as_u64() >> 12) << 12;
            let new_pte = new_pfn | flags;

            if ops.swap_pte(virt, new_pte).is_err() {
                // Rollback : libérer la frame destination.
                let _ = free_pages(dst_frame, 0);
                MIGRATION_STATS.migrations_failed.fetch_add(1, Ordering::Relaxed);
                return MigrateResult::PteUpdateFailed;
            }
            ops.flush_tlb(virt);
        }
    }

    // Libérer la frame source.
    let _ = free_pages(src_frame, 0);

    // Mise à jour des stats des nœuds.
    if let Some(src_n) = NUMA_NODES.get(src_node) {
        src_n.stats.migrated_out.fetch_add(1, Ordering::Relaxed);
        src_n.stats.record_free();
    }
    if let Some(dst_n) = NUMA_NODES.get(target_node) {
        dst_n.stats.migrated_in.fetch_add(1, Ordering::Relaxed);
        dst_n.stats.record_alloc_local();
    }

    MIGRATION_STATS.migrations_succeeded.fetch_add(1, Ordering::Relaxed);
    MIGRATION_STATS.pages_moved.fetch_add(1, Ordering::Relaxed);
    MigrateResult::Success { new_frame: dst_frame }
}

/// Helper : allocation de page ordonnée sur un nœud NUMA spécifique.
/// Délègue à `BUDDY.alloc_on_node(order, flags, numa_node)`.
///
/// # Safety : appelé depuis migrate_page, CPL 0.
unsafe fn alloc_pages_on_node(node_id: u32, order: u32, flags: AllocFlags) -> Option<Frame> {
    use crate::memory::physical::BUDDY;
    // alloc_on_node(order: usize, flags: AllocFlags, numa_node: u8) → Result
    BUDDY.alloc_on_node(order as usize, flags, node_id as u8).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Migration en lot (batch)
// ─────────────────────────────────────────────────────────────────────────────

/// Migre jusqu'à `max_pages` frames vers `target_node` depuis la liste fournie.
/// Retourne le nombre de pages migrées avec succès.
///
/// # Safety : frames doivent être valides, non aliasées pendant l'opération.
pub unsafe fn migrate_pages_batch<P: MigrationPageTableOps>(
    frames:      &[(Frame, Option<u64>)], // (frame, virt_addr optionnelle)
    target_node: u32,
    max_pages:   usize,
    pt_ops:      Option<&P>,
) -> usize {
    let mut count = 0;
    for &(frame, virt) in frames.iter().take(max_pages) {
        match migrate_page(frame, target_node, virt, pt_ops) {
            MigrateResult::Success { .. } => count += 1,
            MigrateResult::AlreadyLocal   => count += 1, // déjà optimal
            _ => {}
        }
    }
    count
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Aucune structure à initialiser pour ce module.
}
