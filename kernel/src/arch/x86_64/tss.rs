//! Task State Segment (TSS)
//! 
//! Manages interrupt stack tables and privilege transitions.

use spin::Mutex;

/// Task State Segment for x86_64
/// 
/// Note: We use `repr(C)` instead of `repr(C, packed)` because:
/// 1. The TSS is naturally aligned on x86_64
/// 2. Accessing packed fields creates unaligned references which is UB
#[repr(C)]
pub struct TaskStateSegment {
    _reserved1: u32,
    pub rsp0: u64,        // Stack pointer for ring 0
    pub rsp1: u64,        // Stack pointer for ring 1
    pub rsp2: u64,        // Stack pointer for ring 2
    _reserved2: u64,
    pub ist: [u64; 7],    // Interrupt Stack Table
    _reserved3: u64,
    _reserved4: u16,
    pub iomap_base: u16,
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        TaskStateSegment {
            _reserved1: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            _reserved2: 0,
            ist: [0; 7],
            _reserved3: 0,
            _reserved4: 0,
            iomap_base: 0,
        }
    }
}

static TSS: Mutex<TaskStateSegment> = Mutex::new(TaskStateSegment::new());

/// Default kernel stack for interrupt handling (64KB)
static mut KERNEL_INTERRUPT_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

/// Double fault stack (separate, 16KB)
static mut DOUBLE_FAULT_STACK: [u8; 16 * 1024] = [0; 16 * 1024];

pub fn init() {
    let mut tss = TSS.lock();
    
    unsafe {
        // Set RSP0 for privilege level transitions (Ring 3 -> Ring 0)
        let stack_top = KERNEL_INTERRUPT_STACK.as_ptr().add(KERNEL_INTERRUPT_STACK.len()) as u64;
        tss.rsp0 = stack_top;
        
        // Set IST1 for double fault handler
        let df_stack_top = DOUBLE_FAULT_STACK.as_ptr().add(DOUBLE_FAULT_STACK.len()) as u64;
        tss.ist[0] = df_stack_top;
    }
    
    log::info!("TSS initialized (RSP0: {:#x})", tss.rsp0);
}

/// Set RSP0 - the stack pointer used when transitioning to Ring 0
/// 
/// # Safety
/// The stack must be valid and large enough for kernel operations
pub unsafe fn set_rsp0(stack_ptr: u64) {
    let mut tss = TSS.lock();
    tss.rsp0 = stack_ptr;
}

/// Get the current RSP0 value
pub fn get_rsp0() -> u64 {
    TSS.lock().rsp0
}

/// Set an IST entry
/// 
/// # Safety
/// The stack must be valid and the index must be 0-6
pub unsafe fn set_ist(index: usize, stack_ptr: u64) {
    if index < 7 {
        TSS.lock().ist[index] = stack_ptr;
    }
}

/// Get TSS address for GDT
pub fn tss_address() -> u64 {
    let tss_guard = TSS.lock();
    &*tss_guard as *const TaskStateSegment as u64
}
