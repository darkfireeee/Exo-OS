//! Zerocopy - Zero-copy path for large messages (>56 bytes)
//!
//! Uses shared memory for messages larger than inline threshold.
//! True zero-copy: data is written directly to shared pages,
//! receiver gets direct access to the same physical memory.
//!
//! ## Performance Target: ~400 cycles for large messages

use super::ring::{Ring, RingSlot};
use super::slot::Slot;
use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use crate::ipc::shared_memory::{page, mapping};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// Slot flag indicating zerocopy message
pub const ZEROCOPY_FLAG: u32 = 0x0001;

/// Maximum zerocopy message size (1MB)
pub const MAX_ZEROCOPY_SIZE: usize = 1024 * 1024;

/// Zerocopy mapping entry
#[derive(Debug)]
struct ZerocopyMapping {
    /// Physical address of shared pages
    phys_addr: PhysicalAddress,
    /// Virtual address (mapped into current address space)
    virt_addr: usize,
    /// Size in bytes
    size: usize,
    /// Reference count (how many processes have this mapped)
    ref_count: usize,
    /// Shared pages backing this mapping
    pages: Vec<page::SharedPage>,
}

/// Global zerocopy mapping tracker
static ZEROCOPY_MAPPINGS: Mutex<Option<BTreeMap<usize, ZerocopyMapping>>> = Mutex::new(None);

fn ensure_mappings_init() -> spin::MutexGuard<'static, Option<BTreeMap<usize, ZerocopyMapping>>> {
    let mut guard = ZEROCOPY_MAPPINGS.lock();
    if guard.is_none() {
        *guard = Some(BTreeMap::new());
    }
    guard
}

/// Allocate zerocopy buffer for sending large data
///
/// # Returns
/// (physical_address, virtual_pointer, size) tuple
pub fn allocate_zerocopy_buffer(size: usize) -> MemoryResult<(PhysicalAddress, *mut u8, usize)> {
    if size > MAX_ZEROCOPY_SIZE {
        return Err(MemoryError::InvalidSize);
    }
    
    let page_count = (size + page::PAGE_SIZE - 1) / page::PAGE_SIZE;
    let aligned_size = page_count * page::PAGE_SIZE;
    
    // Allocate physical pages
    let pages = page::alloc_shared_pages(page_count, page::PageFlags::default())?;
    
    if pages.is_empty() {
        return Err(MemoryError::OutOfMemory);
    }
    
    let phys_addr = pages[0].phys_addr();
    
    // Map into current address space
    let shared_mapping = mapping::map_shared_auto(
        phys_addr,
        aligned_size,
        mapping::MappingFlags::READ_WRITE,
    )?;
    
    let virt_addr = shared_mapping.virt_addr().value();
    
    // Track mapping (keep SharedMapping alive by storing pages)
    let mut mappings = ensure_mappings_init();
    if let Some(ref mut map) = *mappings {
        map.insert(virt_addr, ZerocopyMapping {
            phys_addr,
            virt_addr,
            size: aligned_size,
            ref_count: 1,
            pages,
        });
    }
    
    // Forget shared_mapping to prevent Drop from unmapping
    core::mem::forget(shared_mapping);
    
    log::debug!("Allocated zerocopy buffer: phys={:?}, virt={:#x}, size={}",
        phys_addr, virt_addr, aligned_size);
    
    Ok((phys_addr, virt_addr as *mut u8, aligned_size))
}

/// Send zero-copy message through ring
///
/// The physical address is stored in the slot, receiver will map it
pub fn send_zerocopy(ring: &Ring, phys_addr: PhysicalAddress, size: usize) -> MemoryResult<()> {
    if size > MAX_ZEROCOPY_SIZE {
        return Err(MemoryError::InvalidSize);
    }
    
    // Acquire write slot
    let slot = ring.acquire_write_slot()
        .ok_or(MemoryError::QueueFull)?;
    
    // Encode physical address and size into slot data
    // Layout: [u64 phys_addr][u32 size][u32 reserved]
    unsafe {
        let data_ptr = slot.data.data_mut_ptr();
        let phys_ptr = data_ptr as *mut u64;
        *phys_ptr = phys_addr.value() as u64;
        
        let size_ptr = (data_ptr as *mut u32).add(2);
        *size_ptr = size as u32;
    }
    
    // Set zerocopy flag in slot
    // Note: We're using the RingSlot's data.flags field
    unsafe {
        let slot_ptr = &slot.data as *const Slot as *mut Slot;
        let flags_ptr = core::ptr::addr_of_mut!((*slot_ptr).flags);
        core::ptr::write_volatile(flags_ptr, ZEROCOPY_FLAG);
        
        let size_field_ptr = core::ptr::addr_of_mut!((*slot_ptr).size);
        core::ptr::write_volatile(size_field_ptr, size as u32);
    }
    
    // Complete write
    ring.complete_write(slot);
    
    log::trace!("Sent zerocopy message: phys={:?}, size={}", phys_addr, size);
    Ok(())
}

/// Receive zero-copy message from ring
///
/// # Returns
/// (physical_address, size) - caller should map the physical address
pub fn recv_zerocopy(ring: &Ring) -> MemoryResult<(PhysicalAddress, usize)> {
    // Acquire read slot
    let slot = ring.acquire_read_slot()
        .ok_or(MemoryError::NotFound)?;
    
    // Check zerocopy flag
    if slot.data.flags != ZEROCOPY_FLAG {
        ring.complete_read(slot);
        return Err(MemoryError::InvalidParameter);
    }
    
    let size = slot.data.size as usize;
    
    // Read physical address from slot data
    let phys_addr = unsafe {
        let data_ptr = slot.data.data_ptr();
        let phys_ptr = data_ptr as *const u64;
        PhysicalAddress::new(*phys_ptr as usize)
    };
    
    // Complete read
    ring.complete_read(slot);
    
    log::trace!("Received zerocopy message: phys={:?}, size={}", phys_addr, size);
    Ok((phys_addr, size))
}

/// Map shared memory for reading received zerocopy message
pub fn map_shared(phys_addr: PhysicalAddress, size: usize) -> MemoryResult<*mut u8> {
    // Check if already mapped
    {
        let mappings = ensure_mappings_init();
        if let Some(ref map) = *mappings {
            for (_, mapping) in map.iter() {
                if mapping.phys_addr == phys_addr {
                    // Already mapped, store virt_addr before releasing lock
                    let virt_addr = mapping.virt_addr;
                    drop(mappings);
                    let _ = retain_shared(virt_addr as *mut u8);
                    return Ok(virt_addr as *mut u8);
                }
            }
        }
    }
    
    // Not mapped, create new mapping
    let page_count = (size + page::PAGE_SIZE - 1) / page::PAGE_SIZE;
    
    // Create page descriptors for existing physical memory
    let mut pages = Vec::with_capacity(page_count);
    for i in 0..page_count {
        let page_phys = PhysicalAddress::new(phys_addr.value() + i * page::PAGE_SIZE);
        pages.push(page::SharedPage::new(page_phys, page::PageFlags::default()));
    }
    
    // Map into address space (read-only for receiver)
    let shared_mapping = mapping::map_shared_auto(
        phys_addr,
        size,
        mapping::MappingFlags::READ_ONLY,
    )?;
    
    let virt_addr = shared_mapping.virt_addr().value();
    
    // Track mapping
    {
        let mut mappings = ensure_mappings_init();
        if let Some(ref mut map) = *mappings {
            map.insert(virt_addr, ZerocopyMapping {
                phys_addr,
                virt_addr,
                size,
                ref_count: 1,
                pages,
            });
        }
    }
    
    // Forget to prevent Drop
    core::mem::forget(shared_mapping);
    
    log::debug!("Mapped zerocopy region: phys={:?} -> virt={:#x}, size={}",
        phys_addr, virt_addr, size);
    
    Ok(virt_addr as *mut u8)
}

/// Unmap shared memory when done with zerocopy message
pub fn unmap_shared(ptr: *mut u8, size: usize) -> MemoryResult<()> {
    let virt_addr = ptr as usize;
    
    let mut mappings = ensure_mappings_init();
    if let Some(ref mut map) = *mappings {
        // First check if exists and decrement ref count
        let should_remove = {
            if let Some(mapping) = map.get_mut(&virt_addr) {
                mapping.ref_count -= 1;
                mapping.ref_count == 0
            } else {
                log::warn!("Attempted to unmap unknown zerocopy region at {:p}", ptr);
                return Err(MemoryError::NotFound);
            }
        };
        
        if should_remove {
            log::debug!("Unmapping zerocopy region: virt={:#x}", virt_addr);
            
            // Remove and drop pages
            if let Some(m) = map.remove(&virt_addr) {
                drop(m.pages);
            }
            
            // Release lock before unmapping from page tables
            drop(mappings);
            
            // Actually unmap from page tables
            let page_count = (size + page::PAGE_SIZE - 1) / page::PAGE_SIZE;
            for i in 0..page_count {
                let page_virt = VirtualAddress::new(virt_addr + i * page::PAGE_SIZE);
                let _ = crate::memory::virtual_mem::unmap_page(page_virt);
            }
        } else {
            log::debug!("Zerocopy mapping at {:#x} still has refs", virt_addr);
        }
        
        Ok(())
    } else {
        Err(MemoryError::NotFound)
    }
}

/// Increment reference count for shared mapping
pub fn retain_shared(ptr: *mut u8) -> MemoryResult<()> {
    let virt_addr = ptr as usize;
    
    let mut mappings = ensure_mappings_init();
    if let Some(ref mut map) = *mappings {
        if let Some(mapping) = map.get_mut(&virt_addr) {
            mapping.ref_count += 1;
            log::debug!("Zerocopy mapping at {:#x} ref_count now {}",
                virt_addr, mapping.ref_count);
            Ok(())
        } else {
            Err(MemoryError::NotFound)
        }
    } else {
        Err(MemoryError::NotFound)
    }
}

/// Get reference count for mapping
pub fn get_ref_count(ptr: *mut u8) -> usize {
    let virt_addr = ptr as usize;
    
    let mappings = ensure_mappings_init();
    if let Some(ref map) = *mappings {
        map.get(&virt_addr).map(|m| m.ref_count).unwrap_or(0)
    } else {
        0
    }
}

/// High-level zerocopy send: allocate + copy + send
///
/// For callers who have data in a buffer and want zerocopy transfer
pub fn send_zerocopy_data(ring: &Ring, data: &[u8]) -> MemoryResult<()> {
    let (phys_addr, ptr, _size) = allocate_zerocopy_buffer(data.len())?;
    
    // Copy data to shared buffer
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
    }
    
    // Send physical address through ring
    send_zerocopy(ring, phys_addr, data.len())
}

/// High-level zerocopy receive: receive + map + read
///
/// Returns pointer valid until unmap_shared is called
pub fn recv_zerocopy_data(ring: &Ring) -> MemoryResult<(*mut u8, usize)> {
    let (phys_addr, size) = recv_zerocopy(ring)?;
    let ptr = map_shared(phys_addr, size)?;
    Ok((ptr, size))
}

/// Statistics for zerocopy subsystem
pub fn zerocopy_stats() -> ZerocopyStats {
    let mappings = ensure_mappings_init();
    if let Some(ref map) = *mappings {
        let total_mappings = map.len();
        let total_bytes: usize = map.values().map(|m| m.size).sum();
        let total_refs: usize = map.values().map(|m| m.ref_count).sum();
        
        ZerocopyStats {
            active_mappings: total_mappings,
            total_bytes_mapped: total_bytes,
            total_references: total_refs,
        }
    } else {
        ZerocopyStats::default()
    }
}

/// Zerocopy subsystem statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct ZerocopyStats {
    /// Number of active mappings
    pub active_mappings: usize,
    /// Total bytes currently mapped
    pub total_bytes_mapped: usize,
    /// Total reference count across all mappings
    pub total_references: usize,
}
