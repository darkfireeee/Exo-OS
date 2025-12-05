//! Scheduler V2 - Fork-Safe Implementation
//!
//! This scheduler is designed to be fork-safe by using:
//! 1. A separate pending queue for new threads (accessed atomically)
//! 2. Careful lock ordering to prevent deadlocks
//! 3. Interrupt guards around critical sections

use crate::logger;
use crate::scheduler::idle;
use crate::scheduler::switch::windowed;
use crate::scheduler::thread::{alloc_thread_id, Thread, ThreadContext, ThreadId, ThreadState};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use core::ptr;
use spin::Mutex;

/// Queue type for 3-queue EMA system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueTypeV2 {
    Hot,
    Normal,
    Cold,
}

/// Ready queues (3-tier EMA-based)
struct ReadyQueues {
    hot: VecDeque<Box<Thread>>,
    normal: VecDeque<Box<Thread>>,
    cold: VecDeque<Box<Thread>>,
}

impl ReadyQueues {
    fn new() -> Self {
        Self {
            hot: VecDeque::new(),
            normal: VecDeque::new(),
            cold: VecDeque::new(),
        }
    }
    
    fn classify(ema_ns: u64) -> QueueTypeV2 {
        if ema_ns < 1_000_000 { QueueTypeV2::Hot }
        else if ema_ns < 10_000_000 { QueueTypeV2::Normal }
        else { QueueTypeV2::Cold }
    }
    
    fn enqueue(&mut self, thread: Box<Thread>) {
        match Self::classify(thread.ema_runtime_ns()) {
            QueueTypeV2::Hot => self.hot.push_back(thread),
            QueueTypeV2::Normal => self.normal.push_back(thread),
            QueueTypeV2::Cold => self.cold.push_back(thread),
        }
    }
    
    fn dequeue(&mut self) -> Option<Box<Thread>> {
        self.hot.pop_front()
            .or_else(|| self.normal.pop_front())
            .or_else(|| self.cold.pop_front())
    }
    
    fn lengths(&self) -> (usize, usize, usize) {
        (self.hot.len(), self.normal.len(), self.cold.len())
    }
    
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.hot.is_empty() && self.normal.is_empty() && self.cold.is_empty()
    }
}

/// Pending thread entry (for lock-free additions)
struct PendingThread {
    thread: Box<Thread>,
    next: AtomicPtr<PendingThread>,
}

/// Lock-Free Scheduler V2
///
/// Key design:
/// - `pending_head`: Atomic linked list for new threads (NEVER locked)
/// - `ready_queues`: Protected by single lock
/// - `current_thread`: Separate lock, never held with ready_queues
pub struct SchedulerV2 {
    /// Pending threads (lock-free linked list)
    /// New threads are added here via CAS, processed during schedule()
    pending_head: AtomicPtr<PendingThread>,
    
    /// Ready queues (protected by lock)
    ready_queues: Mutex<ReadyQueues>,
    
    /// Current running thread
    current_thread: Mutex<Option<Box<Thread>>>,
    
    /// Blocked threads
    blocked_threads: Mutex<BTreeMap<ThreadId, Box<Thread>>>,
    
    /// Zombie threads
    zombie_threads: Mutex<BTreeMap<ThreadId, Box<Thread>>>,
    
    /// All thread IDs
    all_threads: Mutex<Vec<ThreadId>>,
    
    /// Statistics
    total_spawns: AtomicU64,
    total_switches: AtomicU64,
    
    /// Idle thread ID
    idle_thread_id: AtomicU64,
    
    /// Schedule count
    schedule_count: AtomicU64,
    
    /// Initialized flag
    initialized: AtomicBool,
}

impl SchedulerV2 {
    /// Create new scheduler (const-compatible)
    pub const fn new() -> Self {
        Self {
            pending_head: AtomicPtr::new(ptr::null_mut()),
            ready_queues: Mutex::new(ReadyQueues {
                hot: VecDeque::new(),
                normal: VecDeque::new(),
                cold: VecDeque::new(),
            }),
            current_thread: Mutex::new(None),
            blocked_threads: Mutex::new(BTreeMap::new()),
            zombie_threads: Mutex::new(BTreeMap::new()),
            all_threads: Mutex::new(Vec::new()),
            total_spawns: AtomicU64::new(0),
            total_switches: AtomicU64::new(0),
            idle_thread_id: AtomicU64::new(0),
            schedule_count: AtomicU64::new(0),
            initialized: AtomicBool::new(false),
        }
    }
    
    /// Initialize the scheduler
    pub fn init(&self) {
        logger::info("[SCHED_V2] Initializing fork-safe scheduler...");
        windowed::init();
        idle::init();
        self.initialized.store(true, Ordering::Release);
        logger::info("[SCHED_V2] ✓ Scheduler initialized");
    }
    
    /// Add thread to pending queue (LOCK-FREE!)
    ///
    /// This is THE key function that makes fork work:
    /// - Uses CAS to add to atomic linked list
    /// - NEVER acquires any lock
    /// - Can be called from syscall, IRQ, anywhere
    fn add_to_pending(&self, thread: Box<Thread>) {
        let entry = Box::into_raw(Box::new(PendingThread {
            thread,
            next: AtomicPtr::new(ptr::null_mut()),
        }));
        
        loop {
            let head = self.pending_head.load(Ordering::Acquire);
            unsafe { (*entry).next.store(head, Ordering::Relaxed); }
            
            match self.pending_head.compare_exchange(
                head,
                entry,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue, // Retry
            }
        }
    }
    
    /// Process pending threads (called with ready_queues lock held)
    fn process_pending(&self, queues: &mut ReadyQueues) {
        // Atomically take the entire pending list
        let head = self.pending_head.swap(ptr::null_mut(), Ordering::AcqRel);
        
        if head.is_null() {
            return;
        }
        
        // Process all pending threads
        let mut count = 0;
        let mut current = head;
        
        while !current.is_null() {
            let entry = unsafe { Box::from_raw(current) };
            let next = entry.next.load(Ordering::Relaxed);
            
            queues.enqueue(entry.thread);
            count += 1;
            
            current = next;
        }
        
        if count > 0 {
            logger::debug(&format!("[SCHED_V2] Processed {} pending threads", count));
        }
    }
    
    /// Spawn a new kernel thread
    pub fn spawn(&self, name: &str, entry: fn() -> !, stack_size: usize) -> ThreadId {
        logger::debug(&format!("[SPAWN_V2] Creating thread '{}' with {}KB stack", name, stack_size / 1024));
        
        let id = alloc_thread_id();
        let thread = Box::new(Thread::new_kernel(id, name, entry, stack_size));
        
        // Register thread ID
        self.all_threads.lock().push(id);
        self.total_spawns.fetch_add(1, Ordering::Relaxed);
        
        // Add to pending queue (lock-free!)
        self.add_to_pending(thread);
        
        logger::info(&format!("[SPAWN_V2] ✓ Thread '{}' (TID {}) spawned", name, id));
        id
    }
    
    /// Add an existing thread (FORK-SAFE!)
    ///
    /// This function can be safely called from sys_fork() because it
    /// uses the lock-free pending queue.
    pub fn add_thread(&self, thread: Thread) {
        let id = thread.id();
        let name = thread.name().to_string();
        
        logger::debug(&format!("[ADD_V2] Adding thread '{}' (TID {}) - FORK-SAFE", name, id));
        
        // Register thread ID (quick lock, always released before schedule)
        self.all_threads.lock().push(id);
        self.total_spawns.fetch_add(1, Ordering::Relaxed);
        
        // Add to pending queue (LOCK-FREE!)
        self.add_to_pending(Box::new(thread));
        
        logger::info(&format!("[ADD_V2] ✓ Thread '{}' (TID {}) added", name, id));
    }
    
    /// Spawn idle thread
    pub fn spawn_idle(&self) {
        logger::info("[SCHED_V2] Spawning idle thread...");
        let id = self.spawn("idle", idle::idle_thread_entry as fn() -> !, 4096);
        self.idle_thread_id.store(id, Ordering::Release);
        idle::register_idle_thread(id);
        logger::info(&format!("[SCHED_V2] ✓ Idle thread spawned (TID {})", id));
    }
    
    /// Get current thread ID
    pub fn current_thread_id(&self) -> Option<ThreadId> {
        self.current_thread.lock().as_ref().map(|t| t.id())
    }
    
    /// Execute closure with current thread
    pub fn with_current_thread<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut Thread) -> R,
    {
        let _guard = InterruptGuard::new();
        self.current_thread.lock().as_mut().map(|t| f(t))
    }
    
    /// Main scheduling function (called from timer IRQ)
    pub fn schedule(&self) {
        let count = self.schedule_count.fetch_add(1, Ordering::Relaxed);
        
        // Context switch info
        let old_ctx: *mut ThreadContext;
        let new_ctx: *const ThreadContext;
        let switch_needed: bool;
        let mut zombie_entry: Option<(ThreadId, Box<Thread>)> = None;
        
        {
            // CRITICAL: Disable interrupts during scheduling
            let _guard = InterruptGuard::new();
            
            // Lock ready queues first
            let mut queues = self.ready_queues.lock();
            
            // Process any pending threads (from fork, etc.)
            self.process_pending(&mut queues);
            
            // Now lock current thread
            let mut current = self.current_thread.lock();
            
            if let Some(ref mut curr) = *current {
                let state = curr.state();
                
                match state {
                    ThreadState::Terminated => {
                        let tid = curr.id();
                        
                        if let Some(mut next) = queues.dequeue() {
                            new_ctx = next.context_ptr() as *const _;
                            next.set_state(ThreadState::Running);
                            next.inc_context_switches();
                            
                            let old = current.take().unwrap();
                            zombie_entry = Some((tid, old));
                            *current = Some(next);
                            
                            self.total_switches.fetch_add(1, Ordering::Relaxed);
                            old_ctx = ptr::null_mut();
                            switch_needed = true;
                        } else {
                            switch_needed = false;
                            old_ctx = ptr::null_mut();
                            new_ctx = ptr::null();
                        }
                    }
                    
                    ThreadState::Running => {
                        if let Some(mut next) = queues.dequeue() {
                            if curr.id() != next.id() {
                                old_ctx = curr.context_ptr();
                                new_ctx = next.context_ptr() as *const _;
                                
                                curr.set_state(ThreadState::Ready);
                                next.set_state(ThreadState::Running);
                                next.inc_context_switches();
                                
                                let old = current.take().unwrap();
                                queues.enqueue(old);
                                *current = Some(next);
                                
                                self.total_switches.fetch_add(1, Ordering::Relaxed);
                                switch_needed = true;
                            } else {
                                queues.enqueue(next);
                                switch_needed = false;
                                old_ctx = ptr::null_mut();
                                new_ctx = ptr::null();
                            }
                        } else {
                            switch_needed = false;
                            old_ctx = ptr::null_mut();
                            new_ctx = ptr::null();
                        }
                    }
                    
                    ThreadState::Blocked => {
                        if let Some(mut next) = queues.dequeue() {
                            new_ctx = next.context_ptr() as *const _;
                            next.set_state(ThreadState::Running);
                            
                            let old = current.take().unwrap();
                            let old_tid = old.id();
                            
                            // Release queues lock before blocking lock
                            drop(queues);
                            self.blocked_threads.lock().insert(old_tid, old);
                            
                            *current = Some(next);
                            old_ctx = ptr::null_mut();
                            switch_needed = true;
                        } else {
                            switch_needed = false;
                            old_ctx = ptr::null_mut();
                            new_ctx = ptr::null();
                        }
                    }
                    
                    _ => {
                        switch_needed = false;
                        old_ctx = ptr::null_mut();
                        new_ctx = ptr::null();
                    }
                }
            } else {
                // First schedule
                if let Some(mut next) = queues.dequeue() {
                    if count == 0 {
                        logger::info(&format!("[SCHED_V2] First switch! Launching TID {}", next.id()));
                    }
                    
                    new_ctx = next.context_ptr() as *const _;
                    next.set_state(ThreadState::Running);
                    *current = Some(next);
                    
                    old_ctx = ptr::null_mut();
                    switch_needed = true;
                } else {
                    switch_needed = false;
                    old_ctx = ptr::null_mut();
                    new_ctx = ptr::null();
                }
            }
            
            // Locks released here
        }
        
        // Add zombie after locks released
        if let Some((tid, thread)) = zombie_entry {
            self.zombie_threads.lock().insert(tid, thread);
        }
        
        // Context switch AFTER all locks released
        if switch_needed && !new_ctx.is_null() {
            unsafe {
                windowed::switch(old_ctx, new_ctx);
            }
        }
    }
    
    /// Yield current thread
    pub fn yield_now(&self) {
        self.schedule();
    }
    
    /// Block current thread
    pub fn block_current(&self) {
        {
            let _guard = InterruptGuard::new();
            if let Some(ref mut thread) = *self.current_thread.lock() {
                thread.set_state(ThreadState::Blocked);
            }
        }
        self.schedule();
    }
    
    /// Unblock a thread
    pub fn unblock_thread(&self, id: ThreadId) {
        let _guard = InterruptGuard::new();
        
        if let Some(mut thread) = self.blocked_threads.lock().remove(&id) {
            thread.set_state(ThreadState::Ready);
            self.add_to_pending(thread);
            logger::debug(&format!("[SCHED_V2] Thread {} unblocked", id));
        }
    }
    
    /// Get scheduler statistics
    pub fn stats(&self) -> SchedulerStatsV2 {
        let (hot, normal, cold) = self.ready_queues.lock().lengths();
        
        SchedulerStatsV2 {
            total_threads: self.all_threads.lock().len(),
            total_spawns: self.total_spawns.load(Ordering::Relaxed),
            total_switches: self.total_switches.load(Ordering::Relaxed),
            hot_queue_len: hot,
            normal_queue_len: normal,
            cold_queue_len: cold,
        }
    }
    
    /// Get thread state
    pub fn get_thread_state(&self, thread_id: ThreadId) -> Option<ThreadState> {
        // Check zombies
        if self.zombie_threads.lock().contains_key(&thread_id) {
            return Some(ThreadState::Terminated);
        }
        
        // Check current
        if let Some(ref current) = *self.current_thread.lock() {
            if current.id() == thread_id {
                return Some(current.state());
            }
        }
        
        // Check queues
        let queues = self.ready_queues.lock();
        for thread in queues.hot.iter().chain(queues.normal.iter()).chain(queues.cold.iter()) {
            if thread.id() == thread_id {
                return Some(thread.state());
            }
        }
        
        // Check blocked
        if let Some(thread) = self.blocked_threads.lock().get(&thread_id) {
            return Some(thread.state());
        }
        
        None
    }
    
    /// Get exit status
    pub fn get_exit_status(&self, thread_id: ThreadId) -> Option<i32> {
        self.zombie_threads.lock().get(&thread_id).map(|t| t.exit_status())
    }
    
    /// Start scheduling
    pub fn start(&self) {
        unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)); }
        logger::info("[SCHED_V2] ✓ Timer interrupts enabled");
        
        self.schedule();
        
        loop {
            unsafe { core::arch::asm!("hlt"); }
        }
    }
}

/// Scheduler statistics
#[derive(Debug, Clone, Copy)]
pub struct SchedulerStatsV2 {
    pub total_threads: usize,
    pub total_spawns: u64,
    pub total_switches: u64,
    pub hot_queue_len: usize,
    pub normal_queue_len: usize,
    pub cold_queue_len: usize,
}

/// RAII guard for disabling/restoring interrupts
pub struct InterruptGuard {
    was_enabled: bool,
}

impl InterruptGuard {
    pub fn new() -> Self {
        let rflags: u64;
        unsafe {
            core::arch::asm!("pushfq; pop {}", out(reg) rflags);
        }
        let was_enabled = (rflags & 0x200) != 0;
        
        if was_enabled {
            unsafe { core::arch::asm!("cli", options(nomem, nostack)); }
        }
        
        Self { was_enabled }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            unsafe { core::arch::asm!("sti", options(nomem, nostack)); }
        }
    }
}

impl Default for InterruptGuard {
    fn default() -> Self {
        Self::new()
    }
}

// Global instance
pub static SCHEDULER_V2: SchedulerV2 = SchedulerV2::new();
