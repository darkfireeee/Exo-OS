//! Zerocopy - Zero-copy path for large messages (>56 bytes)
//!
//! Uses shared memory for messages larger than inline threshold

use super::{ring::Ring, slot::Slot};
use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::PhysicalAddress;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Tracking structure for zerocopy mappings
#[derive(Debug, Clone)]
struct ZerocopyMapping {
    phys_addr: PhysicalAddress,
    virt_addr: usize,
    size: usize,
    ref_count: usize,
}

static ZEROCOPY_MAPPINGS: Mutex<BTreeMap<usize, ZerocopyMapping>> = Mutex::new(BTreeMap::new());

/// Increment ref count for shared mapping (when multiple processes access same region)
pub fn retain_shared(ptr: *mut u8) -> MemoryResult<()> {
    let virt_addr = ptr as usize;
    let mut mappings = ZEROCOPY_MAPPINGS.lock();
    
    if let Some(mapping) = mappings.get_mut(&virt_addr) {
        mapping.ref_count += 1;
        log::debug!("Zerocopy mapping at {:#x} ref_count now {}",
            virt_addr, mapping.ref_count);
        Ok(())
    } else {
        Err(crate::memory::MemoryError::NotFound)
    }
}

/// Get ref count for mapping
pub fn get_ref_count(ptr: *mut u8) -> usize {
    let virt_addr = ptr as usize;
    let mappings = ZEROCOPY_MAPPINGS.lock();
    mappings.get(&virt_addr).map(|m| m.ref_count).unwrap_or(0)
}

/// Send zero-copy message (shared memory path)
pub fn send_zerocopy(ring: &Ring, phys_addr: PhysicalAddress, size: usize) -> MemoryResult<()> {
    // Acquire write slot
    let slot = ring.acquire_write_slot()
        .ok_or(crate::memory::MemoryError::OutOfMemory)?;
    
    // Store physical address in inline data (8 bytes)
    unsafe {
        let dst = slot.data_mut_ptr() as *mut u64;
        *dst = phys_addr.value() as u64;
    }
    
    // Set size and flags
    unsafe {
        let slot_ptr = slot as *const Slot as *mut Slot;
        let size_ptr = core::ptr::addr_of!((*slot_ptr).size) as *mut u32;
        core::ptr::write_volatile(size_ptr, size as u32);
        
        let flags_ptr = core::ptr::addr_of!((*slot_ptr).flags) as *mut u32;
        core::ptr::write_volatile(flags_ptr, 1); // ZEROCOPY flag
    }
    
    slot.finish_write();
    Ok(())
}

/// Receive zero-copy message (shared memory path)
pub fn recv_zerocopy(ring: &Ring) -> MemoryResult<(PhysicalAddress, usize)> {
    // Acquire read slot
    let slot = ring.acquire_read_slot()
        .ok_or(crate::memory::MemoryError::NotFound)?;
    
    // Check zerocopy flag
    if slot.flags != 1 {
        slot.finish_read();
        return Err(crate::memory::MemoryError::InvalidParameter);
    }
    
    let size = slot.size as usize;
    
    // Read physical address from inline data
    let phys_addr = unsafe {
        let src = slot.data_ptr() as *const u64;
        PhysicalAddress::new(*src as usize)
    };
    
    slot.finish_read();
    Ok((phys_addr, size))
}

/// Map shared memory for zero-copy transfer
pub fn map_shared(phys_addr: PhysicalAddress, size: usize) -> MemoryResult<*mut u8> {
    use crate::ipc::shared_memory::{mapping, page};
    use crate::memory::address::VirtualAddress;
    
    // Get actual virtual address from process address space allocator
    // Use process-specific VM range: 0x5000_0000 - 0x6000_0000 for zerocopy
    let mut mappings = ZEROCOPY_MAPPINGS.lock();
    
    // Find free virtual address range
    let base = 0x5000_0000usize;
    let mut virt_addr = base;
    
    // Find gap in existing mappings
    for (&addr, mapping) in mappings.iter() {
        if virt_addr + size <= addr {
            break; // Found gap
        }
        virt_addr = addr + mapping.size;
    }
    
    // Ensure we stay within zerocopy range
    if virt_addr + size > 0x6000_0000 {
        return Err(crate::memory::MemoryError::OutOfMemory);
    }
    
    let virt_addr_typed = VirtualAddress::new(virt_addr);
    
    // Create mapping flags
    let flags = mapping::MappingFlags::READ_WRITE;
    
    // Map physical pages to virtual address space
    let _mapping = mapping::map_shared(phys_addr, size, virt_addr_typed, flags)?;
    
    // Track mapping in process context
    let zerocopy_mapping = ZerocopyMapping {
        phys_addr,
        virt_addr,
        size,
        ref_count: 1,
    };
    
    mappings.insert(virt_addr, zerocopy_mapping);
    
    log::debug!("Mapped zerocopy region: phys={:?} -> virt={:#x}, size={}",
        phys_addr, virt_addr, size);
    
    Ok(virt_addr as *mut u8)
}

/// Unmap shared memory
pub fn unmap_shared(ptr: *mut u8, size: usize) -> MemoryResult<()> {
    use crate::memory::address::VirtualAddress;
    use crate::ipc::shared_memory::mapping;
    
    let virt_addr = ptr as usize;
    
    // Retrieve mapping from tracking structure
    let mut mappings = ZEROCOPY_MAPPINGS.lock();
    
    if let Some(mut mapping) = mappings.get_mut(&virt_addr) {
        // Decrement ref count
        mapping.ref_count -= 1;
        
        if mapping.ref_count == 0 {
            // Remove from tracking
            let mapping = mappings.remove(&virt_addr)
                .ok_or(crate::memory::MemoryError::NotFound)?;
            
            log::debug!("Unmapping zerocopy region: virt={:#x}, size={}",
                virt_addr, mapping.size);
            
            // Unmap from page tables (stub - SharedMapping dropped automatically)
            // The actual SharedMapping would be stored and dropped here
        } else {
            log::debug!("Zerocopy mapping at {:#x} still has {} refs",
                virt_addr, mapping.ref_count);
        }
        
        Ok(())
    } else {
        log::warn!("Attempted to unmap unknown zerocopy region at {:p}", ptr);
        Err(crate::memory::MemoryError::NotFound)
    }
}
