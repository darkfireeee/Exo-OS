//! Stack - Thread stack allocation and management
//!
//! Manages kernel and user thread stacks

use crate::memory::address::VirtualAddress;
use crate::memory::MemoryResult;
use alloc::vec::Vec;

/// Default kernel stack size (16KB)
pub const DEFAULT_KERNEL_STACK_SIZE: usize = 16 * 1024;

/// Default user stack size (1MB)
pub const DEFAULT_USER_STACK_SIZE: usize = 1024 * 1024;

/// Stack guard page size (4KB)
pub const STACK_GUARD_SIZE: usize = 4096;

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
}

impl Stack {
    /// Allocate new stack
    pub fn new(size: usize, is_kernel: bool) -> MemoryResult<Self> {
        // Allocate stack memory (simplified - would use page allocator)
        let mut buffer = Vec::with_capacity(size);
        buffer.resize(size, 0);
        
        let base_ptr = buffer.as_ptr() as usize;
        let top_ptr = base_ptr + size;
        
        // Leak the buffer so it stays allocated
        core::mem::forget(buffer);
        
        Ok(Self {
            base: VirtualAddress::new(base_ptr),
            size,
            top: VirtualAddress::new(top_ptr),
            is_kernel,
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
            (self.top.value() - current_rsp.value()) as usize
        } else {
            0
        }
    }
    
    /// Calculate remaining stack space
    pub fn remaining(&self, current_rsp: VirtualAddress) -> usize {
        if current_rsp.value() > self.base.value() {
            (current_rsp.value() - self.base.value()) as usize
        } else {
            0
        }
    }
    
    /// Check for stack overflow
    pub fn check_overflow(&self, current_rsp: VirtualAddress) -> bool {
        current_rsp.value() < self.base.value() + STACK_GUARD_SIZE
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // TODO: Deallocate stack pages
        // For now, we leak the memory (not production ready)
    }
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
}

/// Alias for Stack (for compatibility)
pub type ThreadStack = Stack;
