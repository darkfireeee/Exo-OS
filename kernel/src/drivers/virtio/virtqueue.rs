//! VirtIO Virtqueue Implementation
//!
//! Virtqueues are the fundamental data structure for VirtIO communication.
//! They consist of three parts:
//! - Descriptor Table: Describes buffers (address, length, flags)
//! - Available Ring: Driver tells device which descriptors are ready
//! - Used Ring: Device tells driver which descriptors have been processed

use crate::memory::dma_simple::{dma_alloc_coherent, dma_free_coherent, virt_to_phys_dma};
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;

/// Virtqueue descriptor flags
pub const VIRTQ_DESC_F_NEXT: u16 = 1;       // Descriptor continues in next
pub const VIRTQ_DESC_F_WRITE: u16 = 2;      // Buffer is write-only (device writes)
pub const VIRTQ_DESC_F_INDIRECT: u16 = 4;   // Buffer contains list of descriptors

/// Virtqueue descriptor
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct VirtqDesc {
    pub addr: u64,      // Physical address
    pub len: u32,       // Length
    pub flags: u16,     // Flags (NEXT, WRITE, INDIRECT)
    pub next: u16,      // Next descriptor if NEXT flag set
}

/// Virtqueue available ring
#[repr(C, align(2))]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; 0],  // Variable-length array
    // Used event follows after ring
}

/// Virtqueue used element
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqUsedElem {
    pub id: u32,        // Descriptor chain index
    pub len: u32,       // Bytes written
}

/// Virtqueue used ring
#[repr(C, align(4))]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; 0],  // Variable-length array
    // Available event follows after ring
}

/// Virtqueue structure
pub struct Virtqueue {
    /// Queue size (must be power of 2)
    queue_size: u16,
    
    /// Descriptor table
    desc_table: *mut VirtqDesc,
    
    /// Available ring
    avail_ring: *mut VirtqAvail,
    
    /// Used ring
    used_ring: *mut VirtqUsed,
    
    /// Next descriptor index
    next_desc: AtomicU16,
    
    /// Last seen used index
    last_used_idx: AtomicU16,
    
    /// Free descriptor list
    free_desc: Mutex<alloc::vec::Vec<u16>>,
    
    /// Physical addresses for cleanup
    desc_phys: u64,
    avail_phys: u64,
    used_phys: u64,
    
    /// Virtual addresses for cleanup
    desc_virt: u64,
    avail_virt: u64,
    used_virt: u64,
}

impl Virtqueue {
    /// Create a new virtqueue
    pub fn new(queue_size: u16) -> Result<Self, &'static str> {
        if !queue_size.is_power_of_two() || queue_size == 0 || queue_size > 32768 {
            return Err("Invalid queue size (must be power of 2, max 32768)");
        }
        
        // Calculate sizes
        let desc_table_size = core::mem::size_of::<VirtqDesc>() * queue_size as usize;
        let avail_ring_size = 6 + 2 * queue_size as usize; // flags(2) + idx(2) + ring + used_event(2)
        let used_ring_size = 6 + 8 * queue_size as usize;  // flags(2) + idx(2) + ring + avail_event(2)
        
        // Align sizes to page boundaries
        let desc_pages = (desc_table_size + 4095) / 4096;
        let avail_pages = (avail_ring_size + 4095) / 4096;
        let used_pages = (used_ring_size + 4095) / 4096;
        
        // Allocate DMA memory
        let (desc_virt, desc_phys) = dma_alloc_coherent(desc_pages * 4096, true)?;
        let (avail_virt, avail_phys) = dma_alloc_coherent(avail_pages * 4096, true)?;
        let (used_virt, used_phys) = dma_alloc_coherent(used_pages * 4096, true)?;
        
        // Initialize free descriptor list
        let mut free_desc = alloc::vec::Vec::new();
        for i in 0..queue_size {
            free_desc.push(i);
        }
        
        Ok(Self {
            queue_size,
            desc_table: desc_virt as *mut VirtqDesc,
            avail_ring: avail_virt as *mut VirtqAvail,
            used_ring: used_virt as *mut VirtqUsed,
            next_desc: AtomicU16::new(0),
            last_used_idx: AtomicU16::new(0),
            free_desc: Mutex::new(free_desc),
            desc_phys,
            avail_phys,
            used_phys,
            desc_virt,
            avail_virt,
            used_virt,
        })
    }
    
    /// Get physical addresses for device configuration
    pub fn addresses(&self) -> (u64, u64, u64) {
        (self.desc_phys, self.avail_phys, self.used_phys)
    }
    
    /// Allocate a descriptor
    fn alloc_desc(&self) -> Option<u16> {
        self.free_desc.lock().pop()
    }
    
    /// Free a descriptor
    fn free_desc(&self, idx: u16) {
        self.free_desc.lock().push(idx);
    }
    
    /// Add buffer to virtqueue
    pub fn add_buffer(&mut self, buffers: &[(u64, u32, bool)]) -> Result<u16, &'static str> {
        if buffers.is_empty() {
            return Err("No buffers provided");
        }
        
        // Allocate descriptor chain
        let mut desc_indices = alloc::vec::Vec::new();
        for _ in 0..buffers.len() {
            if let Some(idx) = self.alloc_desc() {
                desc_indices.push(idx);
            } else {
                // Free allocated descriptors
                for &idx in &desc_indices {
                    self.free_desc(idx);
                }
                return Err("No free descriptors");
            }
        }
        
        // Build descriptor chain
        for (i, (addr, len, device_write)) in buffers.iter().enumerate() {
            let idx = desc_indices[i];
            
            unsafe {
                let desc = &mut *self.desc_table.add(idx as usize);
                desc.addr = *addr;
                desc.len = *len;
                desc.flags = if *device_write { VIRTQ_DESC_F_WRITE } else { 0 };
                
                // Link to next descriptor if not last
                if i < buffers.len() - 1 {
                    desc.flags |= VIRTQ_DESC_F_NEXT;
                    desc.next = desc_indices[i + 1];
                }
            }
        }
        
        // Add to available ring
        let head_idx = desc_indices[0];
        unsafe {
            let avail_idx = (*self.avail_ring).idx;
            let ring_ptr = (self.avail_ring as usize + 4 + 2 * (avail_idx as usize % self.queue_size as usize)) as *mut u16;
            *ring_ptr = head_idx;
            
            // Memory barrier
            core::sync::atomic::fence(Ordering::Release);
            
            // Update index
            (*self.avail_ring).idx = avail_idx.wrapping_add(1);
        }
        
        Ok(head_idx)
    }
    
    /// Check if there are used buffers
    pub fn has_used(&self) -> bool {
        let last_used = self.last_used_idx.load(Ordering::Acquire);
        let device_idx = unsafe { (*self.used_ring).idx };
        last_used != device_idx
    }
    
    /// Get next used buffer
    pub fn get_used(&mut self) -> Option<(u16, u32)> {
        let last_used = self.last_used_idx.load(Ordering::Acquire);
        let device_idx = unsafe { (*self.used_ring).idx };
        
        if last_used == device_idx {
            return None;
        }
        
        // Get used element
        let elem = unsafe {
            let ring_ptr = (self.used_ring as usize + 4 + 8 * (last_used as usize % self.queue_size as usize)) as *const VirtqUsedElem;
            *ring_ptr
        };
        
        // Free descriptor chain
        let mut idx = elem.id as u16;
        loop {
            let desc = unsafe { &*self.desc_table.add(idx as usize) };
            let next_idx = desc.next;
            let has_next = (desc.flags & VIRTQ_DESC_F_NEXT) != 0;
            
            self.free_desc(idx);
            
            if !has_next {
                break;
            }
            idx = next_idx;
        }
        
        // Update last used index
        self.last_used_idx.store(last_used.wrapping_add(1), Ordering::Release);
        
        Some((elem.id as u16, elem.len))
    }
    
    /// Notify device (kick)
    pub fn kick(&self, notify_addr: u64) {
        unsafe {
            core::ptr::write_volatile(notify_addr as *mut u16, 0);
        }
    }
}

impl Drop for Virtqueue {
    fn drop(&mut self) {
        // Free DMA memory
        let _ = dma_free_coherent(self.desc_virt);
        let _ = dma_free_coherent(self.avail_virt);
        let _ = dma_free_coherent(self.used_virt);
    }
}

unsafe impl Send for Virtqueue {}
unsafe impl Sync for Virtqueue {}
