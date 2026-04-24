use core::ptr::NonNull;
use virtio_drivers::{BufferDirection, Hal, PhysAddr, PAGE_SIZE};

/// L'implémentation du Hardware Abstraction Layer (HAL) pour `virtio_drivers`.
/// Ce module fait le lien entre la librairie universelle VirtIO et le Kernel Exo-OS.
pub struct ExoHal;

// Simulation de l'allocateur DMA pour satisfaire virtio_drivers (en vrai `no_std`).
// Dans le noyau Exo-OS complet, `kernel::memory::physical::allocator::allocate_pages`
// sera pointé ici.
unsafe impl Hal for ExoHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        // En vrai: palloc(pages)
        // Ici, alloue bêtement sur le tas pour l'instant (la translation MMU s'en chargera).
        let layout = core::alloc::Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        let paddr = ptr as usize; // Identité Mapping = physique (mock)
        (paddr, NonNull::new(ptr).unwrap())
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let layout = core::alloc::Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        unsafe { alloc::alloc::dealloc(vaddr.as_ptr(), layout) };
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        // En vrai: ptr = kernel_phys_to_virt(paddr)
        NonNull::new(paddr as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        // LMAP => mem(virt) = mem(phys)
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        vaddr
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Rien à faire si Identity Mapping.
    }
}
