//! Stack - Thread stack allocation and management
//!
//! Manages kernel and user thread stacks with proper deallocation.

use crate::memory::address::VirtualAddress;
use crate::memory::MemoryResult;
use alloc::alloc::{alloc, dealloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Default kernel stack size (16KB)
pub const DEFAULT_KERNEL_STACK_SIZE: usize = 16 * 1024;

/// Default user stack size (1MB)
pub const DEFAULT_USER_STACK_SIZE: usize = 1024 * 1024;

/// Stack guard page size (4KB)
pub const STACK_GUARD_SIZE: usize = 4096;

/// Stack alignment (16 bytes for x86_64 ABI)
pub const STACK_ALIGNMENT: usize = 16;

/// Statistics for debugging
static STACKS_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static STACKS_DEALLOCATED: AtomicUsize = AtomicUsize::new(0);
static TOTAL_STACK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Thread stack
pub struct Stack {
    /// Stack base (lowest address)
    base: VirtualAddress,
    
    /// Stack size (bytes)
    size: usize,
    
    /// Stack top (highest address, initial RSP)
    top: VirtualAddress,
    
    /// Whether this is a kernel stack
    is_kernel: bool,
    
    /// Original allocation pointer (for deallocation)
    alloc_ptr: *mut u8,
    
    /// Allocation layout (for deallocation)
    layout: Layout,
}

// Stack is Send + Sync because we own the memory
unsafe impl Send for Stack {}
unsafe impl Sync for Stack {}

impl Stack {
    /// Allocate new stack with proper alignment
    pub fn new(size: usize, is_kernel: bool) -> MemoryResult<Self> {
        // Ensure minimum size and alignment
        let size = size.max(4096);
        let aligned_size = (size + STACK_ALIGNMENT - 1) & !(STACK_ALIGNMENT - 1);
        
        // Create layout for allocation
        let layout = Layout::from_size_align(aligned_size, STACK_ALIGNMENT)
            .map_err(|_| crate::memory::MemoryError::InvalidSize)?;
        
        // Allocate memory
        let alloc_ptr = unsafe { alloc(layout) };
        
        if alloc_ptr.is_null() {
            return Err(crate::memory::MemoryError::OutOfMemory);
        }
        
        // Zero the stack for security
        unsafe {
            core::ptr::write_bytes(alloc_ptr, 0, aligned_size);
        }
        
        let base_ptr = alloc_ptr as usize;
        let top_ptr = base_ptr + aligned_size;
        
        // Update statistics
        STACKS_ALLOCATED.fetch_add(1, Ordering::Relaxed);
        TOTAL_STACK_BYTES.fetch_add(aligned_size, Ordering::Relaxed);
        
        log::trace!("Stack allocated: base={:#x}, top={:#x}, size={}", 
            base_ptr, top_ptr, aligned_size);
        
        Ok(Self {
            base: VirtualAddress::new(base_ptr),
            size: aligned_size,
            top: VirtualAddress::new(top_ptr),
            is_kernel,
            alloc_ptr,
            layout,
        })
    }
    
    /// Get stack base address
    pub fn base(&self) -> VirtualAddress {
        self.base
    }
    
    /// Get stack top address (initial RSP)
    pub fn top(&self) -> VirtualAddress {
        self.top
    }
    
    /// Get stack size
    pub fn size(&self) -> usize {
        self.size
    }
    
    /// Check if address is within stack
    pub fn contains(&self, addr: VirtualAddress) -> bool {
        let addr_val = addr.value();
        addr_val >= self.base.value() && addr_val < self.top.value()
    }
    
    /// Check if this is a kernel stack
    pub fn is_kernel(&self) -> bool {
        self.is_kernel
    }
    
    /// Calculate used stack space
    pub fn used(&self, current_rsp: VirtualAddress) -> usize {
        if current_rsp.value() < self.top.value() {
            self.top.value() - current_rsp.value()
        } else {
            0
        }
    }
    
    /// Calculate remaining stack space
    pub fn remaining(&self, current_rsp: VirtualAddress) -> usize {
        if current_rsp.value() > self.base.value() {
            current_rsp.value() - self.base.value()
        } else {
            0
        }
    }
    
    /// Check for stack overflow
    pub fn check_overflow(&self, current_rsp: VirtualAddress) -> bool {
        current_rsp.value() < self.base.value() + STACK_GUARD_SIZE
    }
    
    /// Get the raw pointer (for context switch)
    pub fn as_ptr(&self) -> *mut u8 {
        self.alloc_ptr
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        if !self.alloc_ptr.is_null() {
            log::trace!("Stack deallocated: base={:#x}, size={}", 
                self.base.value(), self.size);
            
            unsafe {
                // Zero memory before freeing (security)
                core::ptr::write_bytes(self.alloc_ptr, 0, self.size);
                
                // Deallocate
                dealloc(self.alloc_ptr, self.layout);
            }
            
            // Update statistics
            STACKS_DEALLOCATED.fetch_add(1, Ordering::Relaxed);
            TOTAL_STACK_BYTES.fetch_sub(self.size, Ordering::Relaxed);
            
            self.alloc_ptr = core::ptr::null_mut();
        }
    }
}

/// Get stack statistics
pub fn get_stats() -> (usize, usize, usize) {
    (
        STACKS_ALLOCATED.load(Ordering::Relaxed),
        STACKS_DEALLOCATED.load(Ordering::Relaxed),
        TOTAL_STACK_BYTES.load(Ordering::Relaxed),
    )
}

/// Stack allocator
pub struct StackAllocator {
    kernel_stack_size: usize,
    user_stack_size: usize,
}

impl StackAllocator {
    pub const fn new() -> Self {
        Self {
            kernel_stack_size: DEFAULT_KERNEL_STACK_SIZE,
            user_stack_size: DEFAULT_USER_STACK_SIZE,
        }
    }
    
    /// Allocate kernel stack
    pub fn alloc_kernel_stack(&self) -> MemoryResult<Stack> {
        Stack::new(self.kernel_stack_size, true)
    }
    
    /// Allocate user stack
    pub fn alloc_user_stack(&self) -> MemoryResult<Stack> {
        Stack::new(self.user_stack_size, false)
    }
    
    /// Allocate stack with custom size
    pub fn alloc_custom(&self, size: usize, is_kernel: bool) -> MemoryResult<Stack> {
        Stack::new(size, is_kernel)
    }
    
    /// Set kernel stack size
    pub fn set_kernel_stack_size(&mut self, size: usize) {
        self.kernel_stack_size = size.max(4096);
    }
    
    /// Set user stack size
    pub fn set_user_stack_size(&mut self, size: usize) {
        self.user_stack_size = size.max(4096);
    }
}

impl Default for StackAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Alias for Stack (for compatibility)
pub type ThreadStack = Stack;
