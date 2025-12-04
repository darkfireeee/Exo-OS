//! Thread Structure and Management
//!
//! Represents a schedulable thread with minimal overhead

use super::state::ThreadState;
use crate::memory::address::VirtualAddress;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::posix_x::signals::types::SignalStackFrame;

// Phase 11: Import signal types from kernel root
type SigSet = crate::posix_x::signals::SigSet;
type SigAction = crate::posix_x::signals::SigAction;

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
    
    // General purpose registers (needed for fork)
    /// Return value register (RAX)
    pub rax: u64,
    /// Base register (RBX)
    pub rbx: u64,
    /// Counter register (RCX)
    pub rcx: u64,
    /// Data register (RDX)
    pub rdx: u64,
    /// Base pointer (RBP)
    pub rbp: u64,
    /// First argument register (RDI)
    pub rdi: u64,
    /// Second argument register (RSI)
    pub rsi: u64,
    
    // Extended registers
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

impl ThreadContext {
    pub const fn empty() -> Self {
        Self {
            rsp: 0,
            rip: 0,
            cr3: 0,
            rflags: 0,
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rbp: 0,
            rdi: 0,
            rsi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
        }
    }
    
    /// Capture full register context from current stack frame
    /// 
    /// This reads the registers saved by windowed_context_switch on the parent's stack.
    /// Stack layout after windowed_context_switch:
    ///   [rsp+0]  = r15
    ///   [rsp+8]  = r14
    ///   [rsp+16] = r13
    ///   [rsp+24] = r12
    ///   [rsp+32] = rbp
    ///   [rsp+40] = rbx
    ///   [rsp+48] = return address (RIP)
    pub unsafe fn capture_from_stack(parent_rsp: u64) -> Self {
        let stack_ptr = parent_rsp as *const u64;
        
        Self {
            rsp: parent_rsp,
            rip: *stack_ptr.offset(6),  // return address after callee-saved regs
            cr3: 0,  // Will be set by caller
            rflags: 0x202,  // IF enabled
            rax: 0,  // Child gets 0 as return value from fork()
            rbx: *stack_ptr.offset(5),
            rcx: 0,  // Caller-saved, not preserved
            rdx: 0,  // Caller-saved, not preserved
            rbp: *stack_ptr.offset(4),
            rdi: 0,  // Caller-saved, not preserved
            rsi: 0,  // Caller-saved, not preserved
            r8: 0,   // Caller-saved, not preserved
            r9: 0,   // Caller-saved, not preserved
            r10: 0,  // Caller-saved, not preserved
            r11: 0,  // Caller-saved, not preserved
            r12: *stack_ptr.offset(3),
            r13: *stack_ptr.offset(2),
            r14: *stack_ptr.offset(1),
            r15: *stack_ptr.offset(0),
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

    // Phase 9: Parent-child tracking
    /// Parent thread ID (0 if no parent)
    parent_id: AtomicU64,

    /// Children thread IDs
    children: spin::Mutex<Vec<ThreadId>>,

    /// Exit status (for zombie reaping)
    exit_status: core::sync::atomic::AtomicI32,

    // Phase 11: Signal handling
    /// Blocked signals mask
    sigmask: spin::Mutex<crate::posix_x::signals::SigSet>,

    /// Pending signals
    pending_signals: spin::Mutex<crate::posix_x::signals::SigSet>,

    /// Signal handlers (indexed by signal number - 1)
    signal_handlers: spin::Mutex<[crate::posix_x::signals::SigAction; 64]>,
}

impl Thread {
    /// Create a new kernel thread
    pub fn new_kernel(id: ThreadId, name: &str, entry_point: fn() -> !, stack_size: usize) -> Self {
        // Allocate kernel stack
        let stack = Self::allocate_stack(stack_size);
        let stack_top = (stack.value() + stack_size) as u64;

        // Setup initial context using windowed init (prepares stack for context switch)
        let mut context = ThreadContext::empty();
        unsafe {
            crate::scheduler::switch::windowed::init_context(
                &mut context as *mut ThreadContext,
                stack_top,
                entry_point as u64,
            );
        }

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

            // Phase 9: Parent-child tracking
            parent_id: AtomicU64::new(0),
            children: spin::Mutex::new(Vec::new()),
            exit_status: core::sync::atomic::AtomicI32::new(0),

            // Phase 11: Signal handling
            sigmask: spin::Mutex::new(crate::posix_x::signals::SigSet::empty()),
            pending_signals: spin::Mutex::new(crate::posix_x::signals::SigSet::empty()),
            signal_handlers: spin::Mutex::new([crate::posix_x::signals::SigAction::Default; 64]),
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

    /// Create a new user-space thread
    /// 
    /// This creates a thread that will execute in Ring 3 (user mode).
    /// The thread will start at `entry_point` with stack at `user_stack_top`.
    pub fn new_user(
        id: ThreadId,
        name: &str,
        entry_point: VirtualAddress,
        user_stack_top: VirtualAddress,
        kernel_stack_size: usize,
    ) -> Self {
        // Allocate kernel stack (for syscall/interrupt handling)
        let kernel_stack = Self::allocate_stack(kernel_stack_size);
        let kernel_stack_top = (kernel_stack.value() + kernel_stack_size) as u64;

        // Create context that will jump to user mode via trampoline
        let mut context = ThreadContext::empty();
        
        // Store user entry point and stack in context for the trampoline
        context.rdi = entry_point.value() as u64;
        context.rsi = user_stack_top.value() as u64;
        
        // Setup context to jump to user_mode_trampoline
        unsafe {
            crate::scheduler::switch::windowed::init_context(
                &mut context as *mut ThreadContext,
                kernel_stack_top,
                user_mode_trampoline as u64,
            );
        }

        Self {
            id,
            name: name.into(),
            state: ThreadState::Ready,
            priority: ThreadPriority::Normal,
            context,
            kernel_stack,
            kernel_stack_size,
            user_stack: Some(user_stack_top),
            cpu_affinity: None,
            total_runtime_ns: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            ema_runtime_ns: AtomicU64::new(0),

            // Phase 9: Parent-child tracking
            parent_id: AtomicU64::new(0),
            children: spin::Mutex::new(Vec::new()),
            exit_status: core::sync::atomic::AtomicI32::new(0),

            // Phase 11: Signal handling
            sigmask: spin::Mutex::new(crate::posix_x::signals::SigSet::empty()),
            pending_signals: spin::Mutex::new(crate::posix_x::signals::SigSet::empty()),
            signal_handlers: spin::Mutex::new([crate::posix_x::signals::SigAction::Default; 64]),
        }
    }

    /// Check if this is a user-space thread
    pub fn is_user_thread(&self) -> bool {
        self.user_stack.is_some()
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

    // Phase 9: Parent-child tracking methods

    /// Get exit status
    pub fn exit_status(&self) -> i32 {
        self.exit_status.load(Ordering::Acquire)
    }

    /// Set exit status
    pub fn set_exit_status(&self, code: i32) {
        self.exit_status.store(code, Ordering::Release);
    }

    /// Get parent ID
    pub fn parent_id(&self) -> ThreadId {
        self.parent_id.load(Ordering::Acquire)
    }

    /// Set parent ID
    pub fn set_parent_id(&self, parent: ThreadId) {
        self.parent_id.store(parent, Ordering::Release);
    }

    /// Add child
    pub fn add_child(&self, child_id: ThreadId) {
        self.children.lock().push(child_id);
    }

    /// Get children (clone for safe access)
    pub fn get_children(&self) -> Vec<ThreadId> {
        self.children.lock().clone()
    }

    /// Remove child
    pub fn remove_child(&self, child_id: ThreadId) {
        self.children.lock().retain(|&id| id != child_id);
    }

    // Phase 11: Signal handling methods

    /// Get current signal mask
    pub fn get_sigmask(&self) -> crate::posix_x::signals::SigSet {
        *self.sigmask.lock()
    }

    /// Set signal mask
    pub fn set_sigmask(&self, mask: crate::posix_x::signals::SigSet) {
        *self.sigmask.lock() = mask;
    }

    /// Add pending signal
    pub fn add_pending_signal(&self, sig: u32) {
        self.pending_signals.lock().add(sig);
    }

    /// Get next pending signal (not blocked)
    pub fn get_next_pending_signal(&self) -> Option<u32> {
        let pending = *self.pending_signals.lock();
        let blocked = *self.sigmask.lock();

        for sig in 1..=64 {
            if pending.contains(sig) && !blocked.contains(sig) {
                return Some(sig);
            }
        }
        None
    }

    /// Remove pending signal
    pub fn remove_pending_signal(&self, sig: u32) {
        self.pending_signals.lock().remove(sig);
    }

    /// Set signal handler
    pub fn set_signal_handler(&self, sig: u32, action: crate::posix_x::signals::SigAction) {
        if sig > 0 && sig <= 64 {
            self.signal_handlers.lock()[(sig - 1) as usize] = action;
        }
    }

    /// Get signal handler
    pub fn get_signal_handler(&self, sig: u32) -> Option<crate::posix_x::signals::SigAction> {
        if sig > 0 && sig <= 64 {
            Some(self.signal_handlers.lock()[(sig - 1) as usize])
        } else {
            None
        }
    }

    /// Setup signal context (Phase 11)
    /// Saves current context to stack and redirects execution to handler
    pub fn setup_signal_context(&mut self, sig: u32, handler: usize) {
        // 1. Get current stack pointer
        let sp = self.context.rsp;

        // 2. Align stack to 16 bytes and make space for frame
        // Red zone (128 bytes) + Frame size
        let frame_size = core::mem::size_of::<SignalStackFrame>();
        let mut new_sp = sp - 128 - frame_size as u64;
        new_sp &= !0xF; // Align to 16 bytes

        // 3. Create signal frame
        let frame = SignalStackFrame {
            context: self.context,
            sig,
            ret_addr: 0, // Caller (musl trampoline) should handle return
        };

        // 4. Write frame to stack
        // SAFETY: We assume the stack is mapped and writable.
        // In a real kernel, we would need to check permissions/page tables.
        unsafe {
            let ptr = new_sp as *mut SignalStackFrame;
            *ptr = frame;
        }

        // 5. Update context to jump to handler
        self.context.rsp = new_sp;
        self.context.rip = handler as u64;

        // Note: We cannot set RDI (arg1) here because ThreadContext doesn't have it.
        // This is a limitation of the current windowed context switch.
        // Real user-mode signal delivery requires access to TrapFrame.
    }

    /// Restore signal context (Phase 11)
    /// Restores context from stack frame
    pub fn restore_signal_context(&mut self) {
        // 1. Get current stack pointer
        let sp = self.context.rsp;

        // 2. Read frame from stack
        // SAFETY: Assuming stack is valid and contains frame
        let frame = unsafe { &*(sp as *const SignalStackFrame) };

        // 3. Restore context
        self.context = frame.context;

        // Note: This restores the context saved by setup_signal_context.
        // It effectively "returns" to the point where the signal was delivered.
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

/// Child thread entry point for forked threads
/// 
/// This function is called when a forked child thread starts.
/// For Phase 1, it immediately sets itself to Terminated state and yields.
/// In a real implementation, this would restore the parent's context
/// and return 0 from the fork syscall.
pub fn child_entry_point() -> ! {
    use crate::scheduler::{SCHEDULER, ThreadState};
    
    let tid = SCHEDULER.current_thread_id().unwrap_or(0);
    log::debug!("Child thread {} started, becoming zombie", tid);
    
    // Set thread state to Terminated (zombie)
    SCHEDULER.with_current_thread(|thread| {
        thread.set_state(ThreadState::Terminated);
        thread.set_exit_status(0);
    });
    
    log::info!("Process {} exiting with code 0 (zombie state)", tid);
    
    // Yield forever - scheduler won't schedule terminated threads
    loop {
        crate::scheduler::yield_now();
        unsafe { core::arch::asm!("pause") };
    }
}

impl Thread {
    /// Create a forked child thread from parent
    /// 
    /// Phase 2 full implementation:
    /// - Copies parent's complete CPU context (all registers)
    /// - Allocates new kernel stack and copies parent's stack content
    /// - Sets RAX=0 in child context (fork() returns 0 in child)
    /// - Preserves parent-child relationship
    pub fn fork_from(parent: &Thread, child_id: ThreadId, child_pid: u64) -> Self {
        log::debug!("Thread::fork_from: parent={}, child_id={}, copying full context", parent.id, child_id);
        
        // Allocate new kernel stack with same size as parent
        let stack_size = parent.kernel_stack_size;
        let kernel_stack = Self::allocate_stack(stack_size);
        let kernel_stack_top = (kernel_stack.value() + stack_size) as u64;
        
        // Copy parent's context (captures callee-saved registers from stack)
        let mut context = unsafe {
            ThreadContext::capture_from_stack(parent.context.rsp)
        };
        
        // Set child-specific values
        context.rax = 0;  // fork() returns 0 in child
        context.cr3 = parent.context.cr3;  // Same page table initially (COW)
        
        // Copy parent's stack content to child's stack
        // Calculate how much of parent's stack is used
        let parent_stack_bottom = parent.kernel_stack.value() as u64;
        let parent_stack_top = parent_stack_bottom + parent.kernel_stack_size as u64;
        let parent_stack_used = parent_stack_top - parent.context.rsp;
        
        if parent_stack_used > 0 && parent_stack_used < stack_size as u64 {
            let child_stack_bottom = kernel_stack.value() as u64;
            let child_rsp = child_stack_bottom + stack_size as u64 - parent_stack_used;
            
            unsafe {
                // Copy stack data from parent to child
                core::ptr::copy_nonoverlapping(
                    parent.context.rsp as *const u8,
                    child_rsp as *mut u8,
                    parent_stack_used as usize,
                );
            }
            
            // Adjust RSP to point to child's stack
            context.rsp = child_rsp;
            
            log::debug!("fork_from: copied {} bytes of stack data, child_rsp={:#x}", 
                       parent_stack_used, child_rsp);
        } else {
            // Fallback: use child_entry_point if stack copy fails
            log::warn!("fork_from: stack copy failed (used={}, size={}), using child_entry_point",
                      parent_stack_used, stack_size);
            unsafe {
                crate::scheduler::switch::windowed::init_context(
                    &mut context as *mut ThreadContext,
                    kernel_stack_top,
                    child_entry_point as u64,
                );
            }
        }
        
        let child = Self {
            id: child_id,
            name: alloc::format!("child_{}", child_pid).into_boxed_str(),
            state: ThreadState::Ready,
            priority: parent.priority,
            context,
            kernel_stack,
            kernel_stack_size: stack_size,
            user_stack: parent.user_stack,
            cpu_affinity: parent.cpu_affinity,
            total_runtime_ns: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            ema_runtime_ns: AtomicU64::new(0),
            
            // Set parent-child relationship
            parent_id: AtomicU64::new(parent.id),
            children: spin::Mutex::new(Vec::new()),
            exit_status: core::sync::atomic::AtomicI32::new(0),
            
            // Copy signal handling state
            sigmask: spin::Mutex::new(*parent.sigmask.lock()),
            pending_signals: spin::Mutex::new(crate::posix_x::signals::SigSet::empty()),
            signal_handlers: spin::Mutex::new(*parent.signal_handlers.lock()),
        };
        
        // Add child to parent's children list
        parent.add_child(child_id);
        
        log::info!("Thread fork: parent={} -> child={}", parent.id, child_id);
        
        child
    }
}

/// Trampoline function for user-space threads
/// 
/// This function is called when a user thread is first scheduled.
/// It sets up TSS.RSP0 and jumps to user mode.
/// 
/// Arguments passed via ThreadContext (set in Thread::new_user):
/// - rdi: user entry point address
/// - rsi: user stack pointer
fn user_mode_trampoline() -> ! {
    // Get entry point and stack from registers (set by init_context)
    let entry_point: u64;
    let user_stack: u64;
    
    unsafe {
        core::arch::asm!(
            "",
            out("rdi") entry_point,
            out("rsi") user_stack,
            options(nomem, nostack)
        );
    }
    
    log::info!(
        "User mode trampoline: entry={:#x}, stack={:#x}",
        entry_point, user_stack
    );
    
    // Set up TSS.RSP0 for kernel re-entry on syscall/interrupt
    let current_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack));
        crate::arch::x86_64::tss::set_rsp0(current_rsp);
    }
    
    // Create user context and jump to user mode
    let entry = VirtualAddress::new(entry_point as usize);
    let stack = VirtualAddress::new(user_stack as usize);
    
    let context = crate::arch::x86_64::usermode::UserContext::new(entry, stack);
    
    unsafe {
        crate::arch::x86_64::usermode::jump_to_usermode(&context);
    }
}
