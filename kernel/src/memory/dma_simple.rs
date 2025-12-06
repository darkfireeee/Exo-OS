//! DMA (Direct Memory Access) Support
//!
//! Provides physically contiguous memory allocation for device drivers.

use crate::memory::physical::{allocate_contiguous_frames, deallocate_frames};
use alloc::collections::BTreeMap;
use spin::Mutex;
use lazy_static::lazy_static;

/// DMA allocation tracking
lazy_static! {
    static ref DMA_ALLOCATIONS: Mutex<BTreeMap<u64, DmaAlloc>> = Mutex::new(BTreeMap::new());
}

#[derive(Debug, Clone, Copy)]
struct DmaAlloc {
    virt_addr: u64,
    phys_addr: u64,
    size: usize,
    frame_count: usize,
}

/// Allocate DMA memory (physically contiguous)
pub fn dma_alloc_coherent(size: usize, zero: bool) -> Result<(u64, u64), &'static str> {
    if size == 0 {
        return Err("Size cannot be zero");
    }
    
    let page_size = 4096;
    let aligned_size = (size + page_size - 1) & !(page_size - 1);
    let frame_count = aligned_size / page_size;
    
    // Allocate frames below 4GB for compatibility
    let phys_addr = allocate_contiguous_frames(frame_count, true)?;
    
    // Use identity mapping for kernel
    let virt_addr = phys_addr;
    
    if zero {
        unsafe {
            core::ptr::write_bytes(virt_addr as *mut u8, 0, aligned_size);
        }
    }
    
    DMA_ALLOCATIONS.lock().insert(virt_addr, DmaAlloc {
        virt_addr,
        phys_addr,
        size: aligned_size,
        frame_count,
    });
    
    Ok((virt_addr, phys_addr))
}

/// Free DMA memory
pub fn dma_free_coherent(virt_addr: u64) -> Result<(), &'static str> {
    let mut allocs = DMA_ALLOCATIONS.lock();
    let alloc = allocs.remove(&virt_addr).ok_or("Address not found")?;
    deallocate_frames(alloc.phys_addr, alloc.frame_count);
    Ok(())
}

/// Get physical address for virtual DMA address
pub fn virt_to_phys_dma(virt_addr: u64) -> Option<u64> {
    let allocs = DMA_ALLOCATIONS.lock();
    for (base, alloc) in allocs.iter() {
        if virt_addr >= *base && virt_addr < (*base + alloc.size as u64) {
            let offset = virt_addr - base;
            return Some(alloc.phys_addr + offset);
        }
    }
    None
}

/// Initialize DMA subsystem
pub fn init() {
    log::info!("DMA subsystem initialized");
}
