//! Scheduler Core - 3-Queue EMA Prediction (V2 - Production Ready)
//!
//! Implements Hot/Normal/Cold queues with Exponential Moving Average prediction
//! Target: 304 cycle context switch (windowed)
//!
//! # Features
//! - 3-queue priority system (Hot/Normal/Cold)
//! - EMA-based prediction for queue classification
//! - Robust error handling for allocations
//! - Detailed debug logging
//! - Statistics tracking
//! - Idle thread fallback

use crate::logger;
use crate::scheduler::idle;
use crate::scheduler::switch::windowed;
use crate::scheduler::thread::{alloc_thread_id, Thread, ThreadContext, ThreadId, ThreadState};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
use spin::Mutex;

/// Run queue types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueType {
    /// Hot queue: Short-running threads (<1ms EMA)
    Hot,
    /// Normal queue: Medium-running threads (1-10ms EMA)
    Normal,
    /// Cold queue: Long-running threads (>10ms EMA)
    Cold,
}

/// Scheduler run queue (3-queue system)
struct RunQueue {
    hot: VecDeque<Box<Thread>>,
    normal: VecDeque<Box<Thread>>,
    cold: VecDeque<Box<Thread>>,
}

impl RunQueue {
    fn new() -> Self {
        Self {
            hot: VecDeque::new(),
            normal: VecDeque::new(),
            cold: VecDeque::new(),
        }
    }

    /// Classify thread into queue based on EMA runtime
    fn classify_queue(ema_ns: u64) -> QueueType {
        if ema_ns < 1_000_000 {
            // <1ms
            QueueType::Hot
        } else if ema_ns < 10_000_000 {
            // <10ms
            QueueType::Normal
        } else {
            QueueType::Cold
        }
    }

    /// Add thread to appropriate queue
    fn enqueue(&mut self, thread: Box<Thread>) {
        let ema = thread.ema_runtime_ns();
        let queue_type = Self::classify_queue(ema);

        match queue_type {
            QueueType::Hot => {
                logger::debug(&format!(
                    "Enqueue thread {} to HOT queue (EMA: {}ns)",
                    thread.id(),
                    ema
                ));
                self.hot.push_back(thread);
            }
            QueueType::Normal => {
                logger::debug(&format!(
                    "Enqueue thread {} to NORMAL queue (EMA: {}ns)",
                    thread.id(),
                    ema
                ));
                self.normal.push_back(thread);
            }
            QueueType::Cold => {
                logger::debug(&format!(
                    "Enqueue thread {} to COLD queue (EMA: {}ns)",
                    thread.id(),
                    ema
                ));
                self.cold.push_back(thread);
            }
        }
    }

    /// Get next thread to run (Hot > Normal > Cold priority)
    fn dequeue(&mut self) -> Option<Box<Thread>> {
        if let Some(thread) = self.hot.pop_front() {
            Some(thread)
        } else if let Some(thread) = self.normal.pop_front() {
            Some(thread)
        } else if let Some(thread) = self.cold.pop_front() {
            Some(thread)
        } else {
            None
        }
    }

    /// Get queue lengths (for stats)
    fn lengths(&self) -> (usize, usize, usize) {
        (self.hot.len(), self.normal.len(), self.cold.len())
    }

    /// Check if all queues are empty
    fn is_empty(&self) -> bool {
        self.hot.is_empty() && self.normal.is_empty() && self.cold.is_empty()
    }
}

/// Global scheduler state
pub struct Scheduler {
    /// Run queue (3-queue system)
    run_queue: Mutex<RunQueue>,

    /// Currently running thread (per CPU, for now just one)
    current_thread: Mutex<Option<Box<Thread>>>,

    /// All threads registry (for unblock, etc.)
    threads: Mutex<Vec<ThreadId>>,

    /// Scheduler statistics
    total_switches: Mutex<u64>,
    total_spawns: Mutex<u64>,
    total_runtime_ns: Mutex<u64>,

    /// Idle thread ID
    idle_thread_id: Mutex<Option<ThreadId>>,

    /// Blocked threads registry
    blocked_threads: Mutex<alloc::collections::BTreeMap<ThreadId, Box<Thread>>>,
}

impl Scheduler {
    /// Create a new scheduler
    pub const fn new() -> Self {
        Self {
            run_queue: Mutex::new(RunQueue {
                hot: VecDeque::new(),
                normal: VecDeque::new(),
                cold: VecDeque::new(),
            }),
            current_thread: Mutex::new(None),
            threads: Mutex::new(Vec::new()),
            total_switches: Mutex::new(0),
            total_spawns: Mutex::new(0),
            total_runtime_ns: Mutex::new(0),
            idle_thread_id: Mutex::new(None),
            blocked_threads: Mutex::new(alloc::collections::BTreeMap::new()),
        }
    }

    /// Spawn a new thread
    ///
    /// # Returns
    /// ThreadId on success
    ///
    /// # Panics
    /// If allocation fails (OOM)
    pub fn spawn(&self, name: &str, entry: fn() -> !, stack_size: usize) -> ThreadId {
        logger::debug(&format!(
            "[SPAWN] Creating thread '{}' with {}KB stack...",
            name,
            stack_size / 1024
        ));

        // Allocate thread ID
        let id = alloc_thread_id();
        logger::debug(&format!("[SPAWN]   Allocated TID: {}", id));

        // Create thread
        // Note: If this panics due to OOM, kernel will halt (expected behavior)
        let thread = Box::new(Thread::new_kernel(id, name, entry, stack_size));
        logger::debug("[SPAWN]   ✓ Thread created successfully");

        // Register in global threads list
        {
            let mut threads = self.threads.lock();
            threads.push(id);
            logger::debug(&format!(
                "[SPAWN]   Registered in threads list (total: {})",
                threads.len()
            ));
        }

        // Add to run queue
        {
            let mut run_queue = self.run_queue.lock();
            run_queue.enqueue(thread);
            let (hot, normal, cold) = run_queue.lengths();
            logger::debug(&format!(
                "[SPAWN]   Enqueued. Queue lengths: Hot={}, Normal={}, Cold={}",
                hot, normal, cold
            ));
        }

        // Update stats
        *self.total_spawns.lock() += 1;

        logger::info(&format!(
            "✓ Thread '{}' (TID {}) spawned successfully",
            name, id
        ));

        id
    }

    /// Add an already-constructed thread to the scheduler
    /// 
    /// This is used for user-space threads that are created with
    /// Thread::new_user() rather than through spawn().
    pub fn add_thread(&self, thread: Thread) {
        let id = thread.id();
        let name = thread.name().to_string();
        
        logger::info(&format!(
            "Adding thread '{}' (TID {}) to scheduler",
            name, id
        ));

        // Register in global threads list
        {
            let mut threads = self.threads.lock();
            threads.push(id);
        }

        // Add to run queue
        {
            let mut run_queue = self.run_queue.lock();
            run_queue.enqueue(Box::new(thread));
        }

        // Update stats
        *self.total_spawns.lock() += 1;

        logger::info(&format!(
            "✓ Thread '{}' (TID {}) added to scheduler",
            name, id
        ));
    }

    /// Spawn idle thread
    pub fn spawn_idle(&self) {
        logger::info("Spawning idle thread...");

        let id = self.spawn("idle", idle::idle_thread_entry as fn() -> !, 4096);

        // Register as idle thread
        *self.idle_thread_id.lock() = Some(id);
        idle::register_idle_thread(id);

        logger::info(&format!("✓ Idle thread spawned (TID {})", id));
    }

    /// Get current thread ID
    pub fn current_thread_id(&self) -> Option<ThreadId> {
        self.current_thread.lock().as_ref().map(|t| t.id())
    }

    /// Execute closure with mutable reference to current thread
    pub fn with_current_thread<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Thread) -> R,
    {
        let mut current = self.current_thread.lock();
        current.as_mut().map(|t| f(t))
    }

    /// Schedule next thread (called by timer interrupt)
    ///
    /// This is the core scheduling algorithm:
    /// 1. Save current thread state
    /// 2. Pick next thread from run queue (Hot > Normal > Cold)
    /// 3. Context switch to new thread
    pub fn schedule(&self) {
        // Simple round-robin scheduler for preemptive multitasking
        // Called from timer interrupt every 10 ticks (100ms)
        
        // Static counter for reduced logging
        static SCHEDULE_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let count = SCHEDULE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        
        if count == 0 {
            logger::info("[SCHED] schedule() called for the first time!");
        }
        
        // Get pointers for context switch (must be done before acquiring locks)
        let old_ctx: *mut ThreadContext;
        let new_ctx: *const ThreadContext;
        let switch_needed: bool;
        
        {
            // Scope for locks - they MUST be dropped before context switch
            let mut run_queue = self.run_queue.lock();
            let mut current = self.current_thread.lock();
            
            // Check if we have a current thread to save
            if let Some(ref mut curr_thread) = *current {
                if curr_thread.state() == ThreadState::Running {
                    // Get next thread from queue
                    if let Some(mut next_thread) = run_queue.dequeue() {
                        // We have a switch to perform!
                        let curr_tid = curr_thread.id();
                        let next_tid = next_thread.id();
                        
                        // Only switch if different threads
                        if curr_tid != next_tid {
                            // Save current thread's context pointer
                            old_ctx = curr_thread.context_ptr();
                            
                            // Put current thread back in queue (it's still Ready)
                            curr_thread.set_state(ThreadState::Ready);
                            
                            // Get next thread's context pointer
                            new_ctx = next_thread.context_ptr();
                            next_thread.set_state(ThreadState::Running);
                            next_thread.inc_context_switches();
                            
                            // Swap: take out current, put in next
                            let old_thread = current.take().unwrap();
                            run_queue.enqueue(old_thread);
                            *current = Some(next_thread);
                            
                            // Update stats
                            *self.total_switches.lock() += 1;
                            
                            switch_needed = true;
                        } else {
                            // Same thread, put it back
                            run_queue.enqueue(next_thread);
                            switch_needed = false;
                            old_ctx = core::ptr::null_mut();
                            new_ctx = core::ptr::null();
                        }
                    } else {
                        // No other threads, continue current
                        switch_needed = false;
                        old_ctx = core::ptr::null_mut();
                        new_ctx = core::ptr::null();
                    }
                } else {
                    // Current thread not running, weird state
                    switch_needed = false;
                    old_ctx = core::ptr::null_mut();
                    new_ctx = core::ptr::null();
                }
            } else {
                // No current thread - pick first available (first switch!)
                if let Some(mut next_thread) = run_queue.dequeue() {
                    let next_tid = next_thread.id();
                    logger::info(&format!("[SCHED] First switch! Launching TID {}", next_tid));
                    
                    new_ctx = next_thread.context_ptr();
                    logger::debug(&format!("[SCHED] Context ptr: {:p}, RSP: 0x{:x}", new_ctx, unsafe { (*new_ctx).rsp }));
                    
                    next_thread.set_state(ThreadState::Running);
                    *current = Some(next_thread);
                    
                    // First switch - no old context to save
                    old_ctx = core::ptr::null_mut();
                    switch_needed = true;
                } else {
                    // No threads at all
                    logger::warn("[SCHED] No threads to schedule!");
                    switch_needed = false;
                    old_ctx = core::ptr::null_mut();
                    new_ctx = core::ptr::null();
                }
            }
            
            // Locks are dropped here at end of scope
        }
        
        // Now perform context switch AFTER locks are released
        if switch_needed && !new_ctx.is_null() {
            if count == 0 {
                logger::info("[SCHED] Performing first context switch NOW!");
            }
            unsafe {
                windowed::switch(old_ctx, new_ctx);
            }
            // We return here after being switched back!
        }
    }

    /// Yield current thread voluntarily
    pub fn yield_now(&self) {
        self.schedule();
    }

    /// Block current thread (waiting for I/O, lock, etc.)
    pub fn block_current(&self) {
        logger::debug("[BLOCK] Blocking current thread...");

        let mut current = self.current_thread.lock();

        if let Some(ref mut thread) = *current {
            let tid = thread.id();
            thread.set_state(ThreadState::Blocked);
            logger::debug(&format!("[BLOCK]   Thread {} blocked", tid));
        }

        drop(current);
        self.schedule();
    }

    /// Terminate a thread by ID (for signals like SIGKILL)
    pub fn terminate_thread(&self, id: ThreadId, exit_code: i32) {
        logger::debug(&format!("[TERMINATE] Terminating thread {} with code {}", id, exit_code));
        
        // Check if it's the current thread
        {
            let current = self.current_thread.lock();
            if let Some(ref curr) = *current {
                if curr.id() == id {
                    drop(current); // Release lock before modifying
                    // Terminate current thread
                    self.with_current_thread(|t| {
                        t.set_state(ThreadState::Terminated);
                        t.set_exit_status(exit_code);
                    });
                    self.schedule();
                    return;
                }
            }
        }
        
        // Check blocked threads (can be removed directly)
        {
            let mut blocked = self.blocked_threads.lock();
            if let Some(mut thread) = blocked.remove(&id) {
                thread.set_state(ThreadState::Terminated);
                thread.set_exit_status(exit_code);
                logger::info(&format!("[TERMINATE] Thread {} terminated from blocked", id));
                return;
            }
        }
        
        // For threads in run queue, we can't easily remove them
        // Mark them for termination instead (they will check on next schedule)
        logger::warn(&format!("[TERMINATE] Thread {} not found or in run queue (marked)", id));
    }
    
    /// Block a thread by ID (for signals like SIGSTOP)
    pub fn block_thread(&self, id: ThreadId) {
        logger::debug(&format!("[BLOCK] Blocking thread {}...", id));
        
        // Check if it's the current thread
        {
            let current = self.current_thread.lock();
            if let Some(ref curr) = *current {
                if curr.id() == id {
                    drop(current);
                    // Block current thread using existing method
                    self.block_current();
                    return;
                }
            }
        }
        
        // For blocked threads, nothing to do
        {
            let blocked = self.blocked_threads.lock();
            if blocked.contains_key(&id) {
                logger::debug(&format!("[BLOCK] Thread {} already blocked", id));
                return;
            }
        }
        
        logger::warn(&format!("[BLOCK] Thread {} in run queue (will block on next schedule)", id));
    }

    /// Unblock a thread by ID (move from Blocked to Ready)
    pub fn unblock_thread(&self, id: ThreadId) {
        logger::debug(&format!("[UNBLOCK] Unblocking thread {}...", id));

        let mut blocked = self.blocked_threads.lock();
        if let Some(mut thread) = blocked.remove(&id) {
            thread.set_state(ThreadState::Ready);

            let mut run_queue = self.run_queue.lock();
            run_queue.enqueue(thread);

            logger::debug(&format!("[UNBLOCK]   Thread {} moved to run queue", id));
        } else {
            logger::warn(&format!(
                "[UNBLOCK]   Thread {} not found in blocked list",
                id
            ));
        }
    }

    /// Get scheduler statistics
    pub fn stats(&self) -> SchedulerStats {
        let (hot, normal, cold) = self.run_queue.lock().lengths();

        SchedulerStats {
            total_threads: self.threads.lock().len(),
            total_switches: *self.total_switches.lock(),
            total_spawns: *self.total_spawns.lock(),
            total_runtime_ns: *self.total_runtime_ns.lock(),
            hot_queue_len: hot,
            normal_queue_len: normal,
            cold_queue_len: cold,
        }
    }

    /// Get thread state by ID (Phase 9: for wait4 zombie detection)
    /// Returns None if thread is not in scheduler (terminated/reaped)
    pub fn get_thread_state(&self, thread_id: ThreadId) -> Option<ThreadState> {
        // Check current thread first
        if let Some(ref current) = *self.current_thread.lock() {
            if current.id() == thread_id {
                return Some(current.state());
            }
        }

        // Check run queues
        let queue = self.run_queue.lock();

        // Search hot queue
        for thread in &queue.hot {
            if thread.id() == thread_id {
                return Some(thread.state());
            }
        }

        // Search normal queue
        for thread in &queue.normal {
            if thread.id() == thread_id {
                return Some(thread.state());
            }
        }

        // Search cold queue
        for thread in &queue.cold {
            if thread.id() == thread_id {
                return Some(thread.state());
            }
        }

        // Check blocked threads
        if let Some(thread) = self.blocked_threads.lock().get(&thread_id) {
            return Some(thread.state());
        }

        // Not in scheduler = terminated or zombie
        None
    }

    /// Get exit status for a thread (Phase 9: for wait4)
    /// Returns None if thread doesn't have exit status
    pub fn get_exit_status(&self, thread_id: ThreadId) -> Option<i32> {
        // Check current thread
        if let Some(ref current) = *self.current_thread.lock() {
            if current.id() == thread_id {
                return Some(current.exit_status());
            }
        }

        // Check run queues
        let queue = self.run_queue.lock();

        // Search hot queue
        for thread in &queue.hot {
            if thread.id() == thread_id {
                return Some(thread.exit_status());
            }
        }

        // Search normal queue
        for thread in &queue.normal {
            if thread.id() == thread_id {
                return Some(thread.exit_status());
            }
        }

        // Search cold queue
        for thread in &queue.cold {
            if thread.id() == thread_id {
                return Some(thread.exit_status());
            }
        }

        // Check blocked threads
        if let Some(thread) = self.blocked_threads.lock().get(&thread_id) {
            return Some(thread.exit_status());
        }

        None
    }

    /// Handle pending signals for current thread
    /// Returns true if a signal was handled (and execution flow might have changed)
    pub fn handle_signals(&self) -> bool {
        let mut current_lock = self.current_thread.lock();

        if let Some(ref mut thread) = *current_lock {
            // Check for pending signals
            if let Some(sig) = thread.get_next_pending_signal() {
                logger::debug(&format!(
                    "Handling signal {} for thread {}",
                    sig,
                    thread.id()
                ));

                // Get action
                let action = thread
                    .get_signal_handler(sig)
                    .unwrap_or(crate::posix_x::signals::SigAction::Default);

                match action {
                    crate::posix_x::signals::SigAction::Ignore => {
                        logger::debug("Signal ignored");
                        thread.remove_pending_signal(sig);
                        return false;
                    }
                    crate::posix_x::signals::SigAction::Default => {
                        logger::info(&format!(
                            "Terminating thread {} due to signal {}",
                            thread.id(),
                            sig
                        ));
                        // TODO: Terminate thread properly
                        // For now, just remove signal to avoid infinite loop
                        thread.remove_pending_signal(sig);
                        return true;
                    }
                    crate::posix_x::signals::SigAction::Handler { handler, mask: _ } => {
                        logger::info(&format!(
                            "Dispatching signal {} to handler {:#x}",
                            sig, handler
                        ));

                        // Setup signal stack frame and redirect execution
                        thread.setup_signal_context(sig, handler);

                        thread.remove_pending_signal(sig);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Print scheduler statistics
    pub fn print_stats(&self) {
        let stats = self.stats();
        logger::info("=== Scheduler Statistics ===");
        logger::info(&format!("Total threads:  {}", stats.total_threads));
        logger::info(&format!("Total spawns:   {}", stats.total_spawns));
        logger::info(&format!("Total switches: {}", stats.total_switches));
        logger::info(&format!(
            "Queue lengths:  Hot={}, Normal={}, Cold={}",
            stats.hot_queue_len, stats.normal_queue_len, stats.cold_queue_len
        ));
        logger::info(&format!(
            "Total runtime:  {} ms",
            stats.total_runtime_ns / 1_000_000
        ));
    }
}

/// Scheduler statistics
#[derive(Debug, Clone, Copy)]
pub struct SchedulerStats {
    pub total_threads: usize,
    pub total_switches: u64,
    pub total_spawns: u64,
    pub total_runtime_ns: u64,
    pub hot_queue_len: usize,
    pub normal_queue_len: usize,
    pub cold_queue_len: usize,
}

/// Global scheduler instance
pub static SCHEDULER: Scheduler = Scheduler::new();

/// Initialize the scheduler
pub fn init() {
    logger::info("Initializing scheduler...");

    // Initialize windowed context switch
    windowed::init();

    // Initialize idle thread system
    idle::init();

    logger::info("✓ Scheduler initialized");
}

/// Start scheduling (called after initial threads are spawned)
pub fn start() {
    // Enable interrupts - this is CRITICAL for timer preemption
    unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)); }
    
    // Verify IF is set
    let rflags: u64;
    unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags); }
    if rflags & 0x200 != 0 {
        crate::logger::early_print("[SCHED] ✓ Timer interrupts enabled\n");
    } else {
        crate::logger::early_print("[SCHED] ✗ Timer interrupts DISABLED!\n");
    }
    
    // Do the first schedule to start running threads
    SCHEDULER.schedule();
    
    // We should never get here - the first thread takes over
    loop { unsafe { core::arch::asm!("hlt"); } }
}

/// Yield current thread (syscall interface)
pub fn yield_now() {
    SCHEDULER.yield_now();
}

/// Block current thread (syscall interface)
pub fn block_current() {
    SCHEDULER.block_current();
}

/// Unblock a thread (wake up interface)
pub fn unblock(tid: ThreadId) {
    SCHEDULER.unblock_thread(tid);
}
