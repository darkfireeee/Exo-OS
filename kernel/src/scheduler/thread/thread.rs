//! Thread Structure and Management
//! 
//! Represents a schedulable thread with minimal overhead

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::memory::address::VirtualAddress;
use super::state::ThreadState;

/// Thread ID type
pub type ThreadId = u64;

/// Thread priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadPriority {
    Idle = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    Realtime = 4,
}

/// Saved thread context (windowed - minimal saves)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ThreadContext {
    /// Stack pointer (RSP)
    pub rsp: u64,
    /// Instruction pointer (RIP)
    pub rip: u64,
    /// Page table (CR3)
    pub cr3: u64,
    /// Flags register (RFLAGS)
    pub rflags: u64,
}

impl ThreadContext {
    pub const fn empty() -> Self {
        Self {
            rsp: 0,
            rip: 0,
            cr3: 0,
            rflags: 0,
        }
    }
}

/// Thread Control Block (TCB)
pub struct Thread {
    /// Unique thread ID
    id: ThreadId,
    
    /// Thread name (for debugging)
    name: Box<str>,
    
    /// Current state
    state: ThreadState,
    
    /// Priority
    priority: ThreadPriority,
    
    /// Saved context (for windowed context switch)
    context: ThreadContext,
    
    /// Kernel stack base
    kernel_stack: VirtualAddress,
    
    /// Kernel stack size
    kernel_stack_size: usize,
    
    /// User stack (if user-space thread)
    user_stack: Option<VirtualAddress>,
    
    /// CPU affinity (which CPU this thread prefers)
    cpu_affinity: Option<usize>,
    
    /// Runtime statistics
    total_runtime_ns: AtomicU64,
    context_switches: AtomicU64,
    
    /// Prediction: Exponential Moving Average of runtime
    ema_runtime_ns: AtomicU64,
}

impl Thread {
    /// Create a new kernel thread
    pub fn new_kernel(
        id: ThreadId,
        name: &str,
        entry_point: fn() -> !,
        stack_size: usize,
    ) -> Self {
        // Allocate kernel stack
        let stack = Self::allocate_stack(stack_size);
        
        // Setup initial context
        let context = ThreadContext {
            rsp: (stack.value() + stack_size) as u64,
            rip: entry_point as u64,
            cr3: 0, // Will be set by scheduler
            rflags: 0x202, // IF=1 (interrupts enabled)
        };

        Self {
            id,
            name: name.into(),
            state: ThreadState::Ready,
            priority: ThreadPriority::Normal,
            context,
            kernel_stack: stack,
            kernel_stack_size: stack_size,
            user_stack: None,
            cpu_affinity: None,
            total_runtime_ns: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            ema_runtime_ns: AtomicU64::new(0),
        }
    }

    /// Allocate stack memory
    fn allocate_stack(size: usize) -> VirtualAddress {
        // Create a Vec and leak it to get stable memory
        // This is safe because thread stacks are never deallocated
        let stack_vec: Vec<u8> = vec![0u8; size];
        let boxed = stack_vec.into_boxed_slice();
        let ptr = Box::into_raw(boxed) as *mut u8;
        
        VirtualAddress::new(ptr as usize)
    }

    /// Get thread ID
    pub fn id(&self) -> ThreadId {
        self.id
    }

    /// Get thread name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get thread state
    pub fn state(&self) -> ThreadState {
        self.state
    }

    /// Set thread state
    pub fn set_state(&mut self, state: ThreadState) {
        self.state = state;
    }

    /// Get priority
    pub fn priority(&self) -> ThreadPriority {
        self.priority
    }

    /// Set priority
    pub fn set_priority(&mut self, priority: ThreadPriority) {
        self.priority = priority;
    }

    /// Get context pointer (for context switch)
    pub fn context_ptr(&mut self) -> *mut ThreadContext {
        &mut self.context as *mut ThreadContext
    }

    /// Record runtime
    pub fn add_runtime(&self, ns: u64) {
        self.total_runtime_ns.fetch_add(ns, Ordering::Relaxed);
        
        // Update EMA: ema = alpha * new + (1 - alpha) * old
        // alpha = 0.25 (shift by 2)
        let old_ema = self.ema_runtime_ns.load(Ordering::Relaxed);
        let new_ema = (ns / 4) + (old_ema * 3 / 4);
        self.ema_runtime_ns.store(new_ema, Ordering::Relaxed);
    }

    /// Record context switch
    pub fn inc_context_switches(&self) {
        self.context_switches.fetch_add(1, Ordering::Relaxed);
    }

    /// Get EMA runtime (for prediction)
    pub fn ema_runtime_ns(&self) -> u64 {
        self.ema_runtime_ns.load(Ordering::Relaxed)
    }

    /// Get statistics
    pub fn stats(&self) -> ThreadStats {
        ThreadStats {
            id: self.id,
            name: self.name.clone(),
            state: self.state,
            priority: self.priority,
            total_runtime_ns: self.total_runtime_ns.load(Ordering::Relaxed),
            context_switches: self.context_switches.load(Ordering::Relaxed),
            ema_runtime_ns: self.ema_runtime_ns.load(Ordering::Relaxed),
        }
    }
}

/// Thread statistics snapshot
#[derive(Debug, Clone)]
pub struct ThreadStats {
    pub id: ThreadId,
    pub name: Box<str>,
    pub state: ThreadState,
    pub priority: ThreadPriority,
    pub total_runtime_ns: u64,
    pub context_switches: u64,
    pub ema_runtime_ns: u64,
}

/// Global thread ID counter
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new thread ID
pub fn alloc_thread_id() -> ThreadId {
    NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)
}
