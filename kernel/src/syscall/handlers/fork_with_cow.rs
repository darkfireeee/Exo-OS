//! sys_fork() with CoW Manager Integration - COMPLETE
//!
//! This is the complete implementation to replace the stub in
//! kernel/src/syscall/handlers/process.rs

use crate::memory::{self, PhysicalAddress, VirtualAddress, PAGE_SIZE};
use crate::memory::user_space::UserPageFlags;
use crate::memory::virtual_mem;
use crate::scheduler::thread::{Thread, ThreadContext};
use crate::scheduler::SCHEDULER;
use alloc::vec::Vec;

/// Fork - Create child process with Copy-on-Write
///
/// Full implementation with CoW Manager integration:
/// 1. Capture parent's address space (all mapped pages)
/// 2. Clone address space with CoW (marks pages RO in both parent+child)
/// 3. Create child thread with cloned address space
/// 4. Add child to scheduler
/// 5. Return child_pid to parent, 0 to child
pub fn sys_fork_with_cow() -> Result<u64, MemoryError> {
    log::info!("[FORK] Starting fork with CoW");
    
    // 1. Get current thread (parent)
    let parent_tid = SCHEDULER.with_current_thread(|t| t.id())
        .ok_or(MemoryError::InvalidAddress)?;
    
    log::info!("[FORK] Parent TID: {}", parent_tid);
    
    // 2. Capture parent's address space
    // Get all currently mapped pages from page table
    let parent_pages = capture_address_space()?;
    
    log::info!("[FORK] Captured {} pages from parent", parent_pages.len());
    
    // 3. Clone address space with CoW
    // This will:
    // - Mark all writable pages as read-only in both parent and child
    // - Set refcount=2 for each shared page
    // - Return new page list for child (same physical addresses)
    let child_pages = memory::clone_address_space(&parent_pages)
        .map_err(|_| MemoryError::OutOfMemory)?;
    
    log::info!("[FORK] Cloned address space: {} pages marked CoW", child_pages.len());
    
    // 4. Update parent's pages to read-only (for CoW)
    // When parent or child writes, page fault will trigger CoW
    for (virt, phys, flags) in &parent_pages {
        if flags.contains(UserPageFlags::WRITABLE) {
            // Remove writable flag
            let new_flags = flags.difference(UserPageFlags::WRITABLE);
            virtual_mem::protect_page(*virt, new_flags)?;
        }
    }
    
    log::info!("[FORK] Parent pages marked read-only");
    
    // 5. Allocate new PID for child
    let child_pid = allocate_pid();
    
    log::info!("[FORK] Allocated child PID: {}", child_pid);
    
    // 6. Capture parent's registers (RIP, RSP, etc.)
    let parent_context = SCHEDULER.with_current_thread(|t| {
        t.context().clone()
    }).ok_or(MemoryError::InvalidAddress)?;
    
    // 7. Create child thread with cloned context
    // Child will resume execution right after fork() with return value 0
    let mut child_context = parent_context.clone();
    child_context.set_return_value(0); // Child returns 0
    
    let child_thread = Thread::new_with_context(
        child_pid,
        "forked_child",
        child_context,
        child_pages,
    );
    
    log::info!("[FORK] Created child thread");
    
    // 8. Add child to scheduler
    SCHEDULER.add_thread(child_thread)
        .map_err(|_| MemoryError::OutOfMemory)?;
    
    log::info!("[FORK] Child added to scheduler");
    
    // 9. Return child_pid to parent
    Ok(child_pid)
}

/// Capture current address space
///
/// Returns list of (virtual_addr, physical_addr, flags) for all mapped pages
fn capture_address_space() -> Result<Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)>, MemoryError> {
    let mut pages = Vec::new();
    
    // Iterate over user space (0x0 - 0x7fff_ffff_ffff)
    let user_space_start = VirtualAddress::new(0x1000); // Skip null page
    let user_space_end = VirtualAddress::new(0x7fff_ffff_f000);
    
    let mut addr = user_space_start;
    while addr < user_space_end {
        // Check if page is mapped
        if let Ok(Some(phys)) = virtual_mem::get_physical_address(addr) {
            // Get page flags
            if let Ok(flags) = virtual_mem::get_page_flags(addr) {
                pages.push((addr, phys, flags));
            }
        }
        
        // Next page
        addr = VirtualAddress::new(addr.value() + PAGE_SIZE);
    }
    
    Ok(pages)
}

/// Allocate new PID
fn allocate_pid() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static NEXT_PID: AtomicU64 = AtomicU64::new(2);
    NEXT_PID.fetch_add(1, Ordering::SeqCst)
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fork_integration_compile() {
        // Just verify it compiles
    }
}
