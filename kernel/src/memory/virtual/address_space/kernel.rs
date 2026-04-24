// kernel/src/memory/virtual/address_space/kernel.rs
//
// Espace d'adressage kernel — singleton partagé par tous les processus.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::memory::core::{
    layout::VMALLOC_BASE, AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE,
};
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker};

// ─────────────────────────────────────────────────────────────────────────────
// ESPACE D'ADRESSAGE KERNEL
// ─────────────────────────────────────────────────────────────────────────────

/// Espace d'adressage kernel global.
///
/// Partagé par tous les processus (moitié haute de la PML4, indices 256..512).
/// Thread-safe via des spinlocks séparés pour la partie vmalloc et la partie
/// physmap (les deux sont les plus sollicitées concurrentiellement).
pub struct KernelAddressSpace {
    inner: Mutex<KernelAsInner>,
    initialized: AtomicBool,
    pml4_phys: PhysAddr,
}

struct KernelAsInner {
    /// Pointeur de bump pour les allocations vmalloc.
    vmalloc_ptr: VirtAddr,
    /// Statistiques.
    stats: KernelAsStats,
}

#[allow(dead_code)]
#[derive(Default)]
struct KernelAsStats {
    vmalloc_allocs: u64,
    vmalloc_frees: u64,
    map_calls: u64,
    unmap_calls: u64,
}

// SAFETY: KernelAddressSpace est thread-safe via son Mutex interne et des
//         accès atomiques pour les champs non protégés.
unsafe impl Sync for KernelAddressSpace {}
unsafe impl Send for KernelAddressSpace {}

impl KernelAddressSpace {
    pub const fn new() -> Self {
        KernelAddressSpace {
            inner: Mutex::new(KernelAsInner {
                vmalloc_ptr: VMALLOC_BASE,
                stats: KernelAsStats {
                    vmalloc_allocs: 0,
                    vmalloc_frees: 0,
                    map_calls: 0,
                    unmap_calls: 0,
                },
            }),
            initialized: AtomicBool::new(false),
            pml4_phys: PhysAddr::NULL,
        }
    }

    /// Initialise l'espace d'adressage kernel (boot-time, single-CPU).
    ///
    /// SAFETY: Doit être appelé UNE SEULE FOIS, avant tout autre thread.
    pub unsafe fn init(&self, pml4_phys: PhysAddr) {
        debug_assert!(!self.initialized.load(Ordering::Relaxed));
        // SAFETY: Accès non-concurrent en single-CPU — ptr::write évite &mut T depuis &T.
        let pml4_ptr = core::ptr::addr_of!(self.pml4_phys) as *mut PhysAddr;
        pml4_ptr.write(pml4_phys);
        self.initialized.store(true, Ordering::Release);
    }

    /// Adresse physique de la PML4 kernel.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Mappe `frame` → `virt` avec les flags donnés dans l'espace kernel.
    ///
    /// SAFETY: `virt` doit être dans la moitié haute (>= 0xFFFF800000000000).
    pub unsafe fn map<A: FrameAllocatorForWalk>(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
        alloc: &A,
    ) -> Result<(), AllocError> {
        debug_assert!(virt.is_kernel(), "KernelAS::map avec adresse user");
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.map(virt, frame, flags, alloc)?;
        flush_single(virt);
        self.inner.lock().stats.map_calls += 1;
        Ok(())
    }

    /// Démappe `virt` dans l'espace kernel.
    ///
    /// SAFETY: `virt` doit être dans la moitié haute.
    pub unsafe fn unmap(&self, virt: VirtAddr) -> Option<Frame> {
        debug_assert!(virt.is_kernel());
        let mut walker = PageTableWalker::new(self.pml4_phys);
        let result = walker.unmap(virt);
        if result.is_some() {
            flush_single(virt);
            self.inner.lock().stats.unmap_calls += 1;
        }
        result
    }

    /// Alloue une plage vmalloc et y mappe `n_pages` frames fournis.
    ///
    /// Retourne l'adresse virtuelle de début.
    pub unsafe fn vmalloc<A: FrameAllocatorForWalk>(
        &self,
        frames: &[Frame],
        flags: PageFlags,
        alloc: &A,
    ) -> Result<VirtAddr, AllocError> {
        let mut inner = self.inner.lock();
        let start = inner.vmalloc_ptr;
        let end = VirtAddr::new(start.as_u64() + (frames.len() * PAGE_SIZE) as u64);
        // Vérifier que la plage est dans l'espace vmalloc
        if end.as_u64() > crate::memory::core::layout::MODULES_BASE.as_u64() {
            return Err(AllocError::OutOfMemory);
        }
        inner.vmalloc_ptr = end;
        inner.stats.vmalloc_allocs += 1;
        drop(inner);

        let mut walker = PageTableWalker::new(self.pml4_phys);
        for (i, &frame) in frames.iter().enumerate() {
            let v = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            walker.map(v, frame, flags, alloc)?;
            flush_single(v);
        }
        Ok(start)
    }

    /// Traduit une adresse virtuelle kernel en adresse physique.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        let walker = PageTableWalker::new(self.pml4_phys);
        use crate::memory::virt::page_table::WalkResult;
        match walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let off = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + off))
            }
            _ => None,
        }
    }
}

/// Espace d'adressage kernel global (singleton).
pub static KERNEL_AS: KernelAddressSpace = KernelAddressSpace::new();
