// kernel/src/memory/virtual/address_space/user.rs
//
// Espace d'adressage utilisateur — un par processus.
// Couche 0 — aucune dépendance externe sauf `spin`.

use crate::memory::core::{
    layout::{USER_END, USER_STACK_TOP},
    AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker, WalkResult};
use crate::memory::virt::vma::{find_gap, mark_vma_cow, VmaDescriptor, VmaFlags, VmaTree};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// ESPACE D'ADRESSAGE UTILISATEUR
// ─────────────────────────────────────────────────────────────────────────────

/// Constantes de layout par défaut pour l'espace utilisateur.
pub const USER_MMAP_BASE: u64 = 0x0000_7F00_0000_0000;
pub const USER_STACK_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

/// Statistiques de l'espace d'adressage utilisateur.
pub struct UserAsStats {
    pub page_faults: AtomicU64,
    pub cow_breaks: AtomicU64,
    pub mmap_calls: AtomicU64,
    pub munmap_calls: AtomicU64,
    pub vma_count: AtomicU64,
}

impl UserAsStats {
    pub const fn new() -> Self {
        UserAsStats {
            page_faults: AtomicU64::new(0),
            cow_breaks: AtomicU64::new(0),
            mmap_calls: AtomicU64::new(0),
            munmap_calls: AtomicU64::new(0),
            vma_count: AtomicU64::new(0),
        }
    }
}

/// Espace d'adressage d'un processus utilisateur.
pub struct UserAddressSpace {
    inner: Mutex<UserAsInner>,
    pub stats: UserAsStats,
    pml4_phys: PhysAddr,
    /// ID de processus associé (pour le TLB shootdown ciblé).
    pub pid: u64,
    /// Base ELF du heap utilisateur. 0 signifie qu'aucun ELF n'a encore publié
    /// son break initial et que le fallback historique de brk() doit être utilisé.
    pub heap_start: AtomicU64,
    /// Break courant absolu du heap utilisateur.
    pub heap_end: AtomicU64,
}

#[allow(dead_code)]
struct UserAsInner {
    vma_tree: VmaTree,
    mmap_hint: VirtAddr, // Hint pour mmap (bump)
    stack_bottom: VirtAddr,
}

// SAFETY: UserAddressSpace est thread-safe via son Mutex interne.
unsafe impl Sync for UserAddressSpace {}
unsafe impl Send for UserAddressSpace {}

impl UserAddressSpace {
    /// Crée un nouvel espace d'adressage utilisateur vide.
    ///
    /// Le `pml4_phys` doit avoir déjà été construit (clone des entrées kernel).
    pub fn new(pml4_phys: PhysAddr, pid: u64) -> Self {
        let stack_bottom = VirtAddr::new(USER_STACK_TOP.as_u64() - USER_STACK_SIZE);
        UserAddressSpace {
            inner: Mutex::new(UserAsInner {
                vma_tree: VmaTree::new(),
                mmap_hint: VirtAddr::new(USER_MMAP_BASE),
                stack_bottom,
            }),
            stats: UserAsStats::new(),
            pml4_phys,
            pid,
            heap_start: AtomicU64::new(0),
            heap_end: AtomicU64::new(0),
        }
    }

    /// Addresse physique de la PML4 de cet espace.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Initialise les bornes brk de cet espace après chargement ELF.
    #[inline]
    pub fn init_heap_bounds(&self, brk_start: u64) {
        self.heap_start.store(brk_start, Ordering::Release);
        self.heap_end.store(brk_start, Ordering::Release);
    }

    /// Retourne la première adresse non couverte par des VMA HEAP dans
    /// `[start, end)`. `None` signale qu'une VMA non-heap occupe la plage.
    pub fn heap_covered_end_from(&self, start: VirtAddr, end: VirtAddr) -> Option<VirtAddr> {
        let inner = self.inner.lock();
        let mut cursor = start.as_u64();
        let end_raw = end.as_u64();

        while cursor < end_raw {
            let addr = VirtAddr::new(cursor);
            let Some(vma) = inner.vma_tree.find(addr) else {
                return Some(addr);
            };
            if !vma.flags.contains(VmaFlags::HEAP) {
                return None;
            }
            let next = vma.end.as_u64();
            if next <= cursor {
                return None;
            }
            cursor = next.min(end_raw);
        }

        Some(end)
    }

    /// Mappe `virt` → `frame` directement (sans VMA — pour le loader ELF).
    ///
    /// SAFETY: `virt` doit être dans l'espace user (< USER_END).
    pub unsafe fn map_page<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        debug_assert!(
            virt.as_u64() < USER_END.as_u64(),
            "map_page : adresse hors user"
        );
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        flush_single(virt);
        Ok(())
    }

    /// Mappe `virt` -> `frame` sans invalider le TLB.
    ///
    /// SAFETY: a utiliser uniquement pour une plage userspace fraichement
    /// reservee, avant publication de la VMA et avant tout acces utilisateur a
    /// cette plage. Les chemins qui remplacent ou demappent une page doivent
    /// continuer a utiliser `map_page()` ou faire leur propre invalidation.
    pub unsafe fn map_page_unflushed<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        // PATCH-MEM-01: assert dur (pas debug_assert) — en release la corruption
        // serait silencieuse si virt >= USER_END (ecrase l'espace noyau).
        assert!(
            virt.as_u64() < USER_END.as_u64(),
            "map_page_unflushed: adresse hors espace utilisateur: {:#x} >= USER_END {:#x}",
            virt.as_u64(), USER_END.as_u64()
        );
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        Ok(())
    }

    /// Démappe `virt` (sans VMA).
    pub unsafe fn unmap_page(&self, virt: VirtAddr) -> Option<Frame> {
        let mut walker = PageTableWalker::new(self.pml4_phys);
        let result = walker.unmap(virt);
        if result.is_some() {
            flush_single(virt);
        }
        result
    }

    /// Traduit une adresse virtuelle user en physique.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        let walker = PageTableWalker::new(self.pml4_phys);
        match walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let off = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + off))
            }
            _ => None,
        }
    }

    /// Trouve la VMA contenant `addr`.
    pub fn find_vma(&self, addr: VirtAddr) -> Option<*const VmaDescriptor> {
        let inner = self.inner.lock();
        inner.vma_tree.find(addr).map(|v| v as *const _)
    }

    /// Ajoute des flags à la VMA contenant `addr` (PROC-VMA / V-17).
    pub fn set_vma_flags(&self, addr: VirtAddr, extra: VmaFlags) -> bool {
        let mut inner = self.inner.lock();
        if let Some(vma) = inner.vma_tree.find_mut(addr) {
            vma.flags |= extra;
            true
        } else {
            false
        }
    }

    /// Marque toutes les VMAs privées writables comme CoW après fork().
    pub fn mark_all_writable_vmas_cow(&self) {
        let mut inner = self.inner.lock();
        inner.vma_tree.for_each_mut(|vma| mark_vma_cow(vma));
    }

    /// Clone les métadonnées internes du parent vers l'enfant de fork().
    pub fn clone_inner_for_fork(&self, child: &UserAddressSpace) -> bool {
        let src = self.inner.lock();
        let Some(cloned_tree) = src.vma_tree.clone_for_fork() else {
            return false;
        };
        let cloned_count = cloned_tree.len() as u64;

        let mut dst = child.inner.lock();
        dst.vma_tree = cloned_tree;
        dst.mmap_hint = src.mmap_hint;
        dst.stack_bottom = src.stack_bottom;
        child.stats.vma_count.store(cloned_count, Ordering::Relaxed);
        true
    }

    /// Enregistre une nouvelle VMA dans l'espace d'adressage.
    ///
    /// SAFETY: Le descripteur doit avoir été alloué par le slab et l'appelant
    ///         cède la propriété au VmaTree.
    pub unsafe fn insert_vma(&self, vma: *mut VmaDescriptor) -> bool {
        let mut inner = self.inner.lock();
        let result = inner.vma_tree.insert(vma);
        if result {
            self.stats.vma_count.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Retire la VMA commençant à `start`.
    pub fn remove_vma(&self, start: VirtAddr) -> Option<*mut VmaDescriptor> {
        let mut inner = self.inner.lock();
        let result = inner.vma_tree.remove(start);
        if result.is_some() {
            self.stats.vma_count.fetch_sub(1, Ordering::Relaxed);
        }
        result
    }

    /// Cherche un gap libre de `size` octets dans l'espace user.
    pub fn find_free_gap(&self, size: usize, hint: Option<VirtAddr>) -> Option<VirtAddr> {
        let inner = self.inner.lock();
        let hint_addr = hint.unwrap_or(inner.mmap_hint);
        find_gap(
            &inner.vma_tree,
            size,
            hint_addr,
            VirtAddr::new(PAGE_SIZE as u64), // min = 4 KiB (éviter NULL)
            USER_END,
        )
    }

    /// Incrémente le compteur de page faults.
    pub fn record_fault(&self) {
        self.stats.page_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrémente le compteur de CoW breaks.
    pub fn record_cow_break(&self) {
        self.stats.cow_breaks.fetch_add(1, Ordering::Relaxed);
    }

    /// Nombre de VMAs dans cet espace.
    pub fn vma_count(&self) -> usize {
        self.inner.lock().vma_tree.len()
    }
}
