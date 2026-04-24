// kernel/src/memory/heap/allocator/global.rs
//
// Allocateur global Rust (#[global_allocator]) pour le kernel.
// Délègue vers l'allocateur hybride (SLUB + large).
// Couche 0 — aucune dépendance externe sauf `spin`.

use super::hybrid;
use crate::memory::core::AllocFlags;
use core::alloc::{GlobalAlloc, Layout};

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur global Rust pour le kernel.
///
/// Enregistré via `#[global_allocator]` dans lib.rs.
/// Toutes les allocations Box, Vec, String (si utilisées) passeront ici.
///
/// Note : dans le kernel Exo-OS, l'utilisation de Box/Vec est interdite dans
/// les chemins no-alloc (IRQ, preempt-disabled). Ces allocations sont tolérées
/// uniquement dans les contextes de démarrage et de gestion de processus.
pub struct KernelAllocator;

// SAFETY: KernelAllocator est thread-safe — délègue à hybrid::alloc/free
//         qui sont eux-mêmes thread-safe (SLUB utilise des spinlocks internes).
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match hybrid::alloc(layout.size(), layout.align(), AllocFlags::NONE) {
            Ok(ptr) => ptr.as_ptr(),
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }
        if let Some(nn) = core::ptr::NonNull::new(ptr) {
            hybrid::free(nn, layout.size());
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let flags = AllocFlags::ZEROED;
        match hybrid::alloc(layout.size(), layout.align(), flags) {
            Ok(ptr) => ptr.as_ptr(),
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(layout);
        }
        // Allouer le nouveau bloc
        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(l) => l,
            Err(_) => return core::ptr::null_mut(),
        };
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() {
            return core::ptr::null_mut();
        }
        // Copier les données
        let copy_size = layout.size().min(new_size);
        // SAFETY: ptr est valide (alloué précédemment), copy_size <= les deux tailles.
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
        // Libérer l'ancien bloc
        self.dealloc(ptr, layout);
        new_ptr
    }
}

/// Instance globale de l'allocateur kernel.
#[cfg_attr(not(test), global_allocator)]
pub static KERNEL_ALLOCATOR: KernelAllocator = KernelAllocator;
