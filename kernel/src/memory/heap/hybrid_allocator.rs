//! Hybrid Allocator - 3-Level Strategy
//! 
//! Combines thread-local cache, CPU slab, and buddy allocator
//! for optimal performance across different allocation sizes

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use super::{thread_cache, cpu_slab, size_class::SizeClass, statistics::ALLOCATOR_STATS};
use crate::memory::physical::buddy_allocator;

/// Hybrid allocator implementation
pub struct HybridAllocator;

impl HybridAllocator {
    /// Allocate with 3-level strategy
    unsafe fn alloc_hybrid(&self, size: usize) -> *mut u8 {
        let size_class = SizeClass::classify(size);
        let actual_size = size_class.size();

        match size_class {
            SizeClass::ThreadLocal(_) => {
                // Level 1: Try thread-local cache (~8 cycles)
                if let Some(ptr) = thread_cache::thread_alloc(actual_size) {
                    ALLOCATOR_STATS.record_thread_alloc(actual_size);
                    return ptr;
                }

                // Fallback to CPU slab
                if let Some(ptr) = cpu_slab::cpu_alloc(actual_size) {
                    ALLOCATOR_STATS.record_cpu_alloc(actual_size);
                    return ptr;
                }

                // Last resort: allocate new memory from buddy
                self.allocate_from_buddy(actual_size)
            }

            SizeClass::CpuSlab(_) => {
                // Level 2: Try CPU slab (~50 cycles)
                if let Some(ptr) = cpu_slab::cpu_alloc(actual_size) {
                    ALLOCATOR_STATS.record_cpu_alloc(actual_size);
                    return ptr;
                }

                // Fallback to buddy
                self.allocate_from_buddy(actual_size)
            }

            SizeClass::Buddy(_) => {
                // Level 3: Use buddy allocator directly (~200 cycles)
                self.allocate_from_buddy(actual_size)
            }
        }
    }

    /// Allocate from buddy allocator
    unsafe fn allocate_from_buddy(&self, size: usize) -> *mut u8 {
        // Calculate number of frames needed
        let frames = (size + 4095) / 4096;
        
        match buddy_allocator::alloc_contiguous(frames) {
            Ok(phys_addr) => {
                ALLOCATOR_STATS.record_buddy_alloc(size);
                phys_addr.value() as *mut u8
            }
            Err(_) => ptr::null_mut(),
        }
    }

    /// Deallocate with 3-level strategy
    unsafe fn dealloc_hybrid(&self, ptr: *mut u8, size: usize) {
        if ptr.is_null() {
            return;
        }

        let size_class = SizeClass::classify(size);
        let actual_size = size_class.size();

        match size_class {
            SizeClass::ThreadLocal(_) => {
                // Try to return to thread-local cache
                if thread_cache::thread_dealloc(ptr, actual_size) {
                    ALLOCATOR_STATS.record_thread_dealloc(actual_size);
                    return;
                }

                // If cache full, try CPU slab
                if cpu_slab::cpu_dealloc(ptr, actual_size) {
                    ALLOCATOR_STATS.record_cpu_dealloc(actual_size);
                    return;
                }

                // Last resort: return to buddy
                self.deallocate_to_buddy(ptr, actual_size);
            }

            SizeClass::CpuSlab(_) => {
                // Return to CPU slab
                if cpu_slab::cpu_dealloc(ptr, actual_size) {
                    ALLOCATOR_STATS.record_cpu_dealloc(actual_size);
                    return;
                }

                // Fallback to buddy
                self.deallocate_to_buddy(ptr, actual_size);
            }

            SizeClass::Buddy(_) => {
                // Return directly to buddy
                self.deallocate_to_buddy(ptr, actual_size);
            }
        }
    }

    /// Deallocate to buddy allocator
    unsafe fn deallocate_to_buddy(&self, ptr: *mut u8, size: usize) {
        let frames = (size + 4095) / 4096;
        let phys_addr = crate::memory::address::PhysicalAddress::new(ptr as usize);
        
        if let Ok(_) = buddy_allocator::free_contiguous(phys_addr, frames) {
            ALLOCATOR_STATS.record_buddy_dealloc(size);
        }
    }
}

unsafe impl GlobalAlloc for HybridAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(layout.align());
        self.alloc_hybrid(size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(layout.align());
        self.dealloc_hybrid(ptr, size);
    }
}

/// Hybrid allocator instance (not global allocator - use existing LockedHeap)
pub static HYBRID_ALLOCATOR: HybridAllocator = HybridAllocator;

/// Get allocator statistics
pub fn get_allocator_stats() -> crate::memory::heap::statistics::AllocatorStatsSnapshot {
    ALLOCATOR_STATS.snapshot()
}
