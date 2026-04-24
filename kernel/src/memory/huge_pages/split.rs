// kernel/src/memory/huge_pages/split.rs
//
// Split de huge pages au niveau page-table — 2 MiB → 512 × 4 KiB.
// Couche 0 — aucune dépendance externe sauf `spin`.
//
// Ce module gère le split côté page-table : il reconstruit les PTEs 4 KiB
// à partir de l'unique PDE (huge page) en itérant sur les 512 frames.
// La libération physique proprement dite est dans thp.rs.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::{Frame, PhysAddr, HUGE_PAGE_SIZE, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES SPLIT
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques de split de huge pages.
pub struct SplitStats {
    /// Nombre de splits réussis.
    pub splits_done: AtomicU64,
    /// Nombre de splits refusés (page non huge ou non splittable).
    pub splits_refused: AtomicU64,
    /// Nombre total de PTEs reconstruites (splits_done × 512).
    pub ptes_rebuilt: AtomicU64,
}

impl SplitStats {
    const fn new() -> Self {
        SplitStats {
            splits_done: AtomicU64::new(0),
            splits_refused: AtomicU64::new(0),
            ptes_rebuilt: AtomicU64::new(0),
        }
    }
}

pub static SPLIT_STATS: SplitStats = SplitStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// RÉSULTAT DE SPLIT
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un split de huge page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitResult {
    /// Split réussi — 512 frames remis dans le buddy.
    Done,
    /// Adresse non alignée sur 2 MiB.
    NotAligned,
    /// Page non marquée huge dans la PDE fournie.
    NotHugePage,
    /// Échec d'allocation de la table PT intermédiaire.
    PtAllocFailed,
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTION D'UNE PDE HUGE PAGE (2 MiB)
// ─────────────────────────────────────────────────────────────────────────────

/// Flags extraits d'une PDE (Page Directory Entry) pour une huge page.
///
/// Ces flags sont propagés aux 512 PTEs générées lors du split.
#[derive(Debug, Clone, Copy)]
pub struct HugePdeFlags {
    /// Écriture autorisée.
    pub writable: bool,
    /// Accès user-mode autorisé.
    pub user: bool,
    /// Bit NX actif.
    pub no_execute: bool,
    /// Cache désactivé (MMIO).
    pub no_cache: bool,
    /// Write-through activé.
    pub write_through: bool,
    /// Page globale (TLB global entry).
    pub global: bool,
    /// COW flag (bit OS disponible 9).
    pub cow: bool,
}

impl HugePdeFlags {
    /// Reconstruit les flags PDE depuis les bits raw d'une PDE x86_64.
    pub fn from_raw_pde(raw: u64) -> Self {
        HugePdeFlags {
            writable: raw & 0x2 != 0,
            user: raw & 0x4 != 0,
            no_execute: raw & (1u64 << 63) != 0,
            no_cache: raw & 0x10 != 0,
            write_through: raw & 0x8 != 0,
            global: raw & 0x100 != 0,
            cow: raw & 0x200 != 0, // bit 9 = COW (OS-defined)
        }
    }

    /// Encode en flags PTE 4 KiB (PRESENT | flags hérités).
    pub fn to_pte_flags(self) -> u64 {
        let mut f: u64 = 0x1; // PRESENT toujours
        if self.writable {
            f |= 0x2;
        }
        if self.user {
            f |= 0x4;
        }
        if self.write_through {
            f |= 0x8;
        }
        if self.no_cache {
            f |= 0x10;
        }
        if self.global {
            f |= 0x100;
        }
        if self.cow {
            f |= 0x200;
        }
        if self.no_execute {
            f |= 1u64 << 63;
        }
        f
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GÉNÉRATEUR DES 512 PTEs
// ─────────────────────────────────────────────────────────────────────────────

/// Génère les 512 descripteurs PTE pour remplacer une huge PDE.
///
/// `huge_phys` : adresse physique de base de la huge page (alignée 2 MiB).
/// `flags`     : flags propagés depuis la PDE.
///
/// Retourne un tableau de 512 (`phys_frame_start`, `pte_raw`) à écrire
/// dans la Page Table nouvellement allouée.
pub fn generate_split_ptes(huge_phys: PhysAddr, flags: HugePdeFlags) -> [u64; 512] {
    let pte_flags = flags.to_pte_flags();
    let base = huge_phys.as_u64();
    let mut ptes = [0u64; 512];
    for i in 0..512usize {
        let frame_phys = base + (i as u64 * PAGE_SIZE as u64);
        ptes[i] = frame_phys | pte_flags;
    }
    ptes
}

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT LOGIQUE (sans accès direct aux tables de pages)
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un split logique : les 512 (physique, flags) à insérer.
pub struct SplitOutput {
    /// Adresse physique de la table PT à créer (allouée par l'appelant).
    pub pt_phys: PhysAddr,
    /// Les 512 entrées PTE raw à écrire dans `pt_phys`.
    pub ptes: [u64; 512],
}

/// Effectue le split logique d'une huge page.
///
/// `huge_phys`  : adresse physique de la huge page (alignée 2 MiB).
/// `raw_pde`    : valeur raw de la PDE d'origine (pour extraire les flags).
/// `pt_frame`   : frame allouée par l'appelant pour la nouvelle PT.
///
/// L'appelant doit :
///   1. Allouer un frame (`pt_frame`) pour la PT 512 PTEs.
///   2. Appeler cette fonction.
///   3. Écrire `output.ptes` dans le frame `pt_frame` via la physmap.
///   4. Remplacer la PDE originale par une PDE pointant vers `pt_frame`
///      (sans le bit PSE / huge_page).
///   5. Vider le TLB pour la plage [huge_phys, huge_phys + 2 MiB].
///
/// # Safety
/// `huge_phys` doit être alignée sur 2 MiB. Le frame de la huge page doit
/// rester alloué jusqu'à la réécriture des PTEs (les pages restent utilisables).
pub unsafe fn split_huge_pde(
    huge_phys: PhysAddr,
    raw_pde: u64,
    pt_frame: Frame,
) -> Result<SplitOutput, SplitResult> {
    // Vérification alignement 2 MiB.
    if huge_phys.as_u64() % HUGE_PAGE_SIZE as u64 != 0 {
        SPLIT_STATS.splits_refused.fetch_add(1, Ordering::Relaxed);
        return Err(SplitResult::NotAligned);
    }

    // Vérification que c'est bien une PDE huge (bit 7 / PSE).
    if raw_pde & 0x80 == 0 {
        SPLIT_STATS.splits_refused.fetch_add(1, Ordering::Relaxed);
        return Err(SplitResult::NotHugePage);
    }

    // Extraire les flags de la PDE et générer les 512 PTEs.
    let flags = HugePdeFlags::from_raw_pde(raw_pde);
    let ptes = generate_split_ptes(huge_phys, flags);

    SPLIT_STATS.splits_done.fetch_add(1, Ordering::Relaxed);
    SPLIT_STATS.ptes_rebuilt.fetch_add(512, Ordering::Relaxed);

    Ok(SplitOutput {
        pt_phys: pt_frame.start_address(),
        ptes,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// SPLIT AVEC ÉCRITURE DIRECTE VIA PHYSMAP
// ─────────────────────────────────────────────────────────────────────────────

/// Effectue le split en écrivant directement les PTEs dans la physmap.
///
/// `pde_virt` : adresse virtuelle (en physmap) de la PDE à remplacer.
/// `huge_phys` : adresse physique de la huge page.
/// `pt_frame`  : frame allouée pour la nouvelle PT.
/// `phys_map_base` : base de la physmap (PHYS_MAP_BASE).
///
/// Cette fonction écrit atomiquement les 512 PTEs puis remplace la PDE.
///
/// # Safety
/// La physmap doit être accessible et la PDE doit pointer sur `huge_phys`.
pub unsafe fn split_huge_pde_in_place(
    pde_virt: *mut u64,
    huge_phys: PhysAddr,
    raw_pde: u64,
    pt_frame: Frame,
    phys_map_base: u64,
) -> SplitResult {
    let output = match split_huge_pde(huge_phys, raw_pde, pt_frame) {
        Ok(o) => o,
        Err(e) => return e,
    };

    // Écrire les 512 PTEs dans la nouvelle PT via la physmap.
    let pt_virt = (phys_map_base + output.pt_phys.as_u64()) as *mut u64;
    // SAFETY: La physmap est mappée linéairement, pt_frame est alloué.
    for i in 0..512usize {
        pt_virt.add(i).write_volatile(output.ptes[i]);
    }

    // Barrière de store avant la mise à jour de la PDE.
    core::sync::atomic::fence(Ordering::Release);

    // Remplacer la PDE par une PDE non-huge pointant vers la nouvelle PT.
    // Flags : PRESENT | WRITE | (USER si la PDE originale était user).
    let mut new_pde_flags: u64 = 0x3; // PRESENT + WRITE
    if raw_pde & 0x4 != 0 {
        new_pde_flags |= 0x4;
    } // USER
    if raw_pde & (1u64 << 63) != 0 {
        new_pde_flags |= 1u64 << 63;
    } // NX

    let new_pde = output.pt_phys.as_u64() | new_pde_flags;
    pde_virt.write_volatile(new_pde);

    SplitResult::Done
}
