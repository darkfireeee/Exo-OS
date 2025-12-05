//! Scheduler Error Handling
//!
//! Comprehensive error types for all scheduler operations.
//! Better than Linux: Typed errors with recovery hints.

use core::fmt;

/// Scheduler error types with detailed context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    // ═══════════════════════════════════════════════════════════════
    // Thread Limit Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Maximum thread limit reached globally
    ThreadLimitReached { current: usize, max: usize },
    
    /// Per-process thread limit reached
    ProcessThreadLimit { pid: u64, current: usize, max: usize },
    
    /// Per-user thread limit reached
    UserThreadLimit { uid: u32, current: usize, max: usize },
    
    // ═══════════════════════════════════════════════════════════════
    // Queue Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Pending queue is full (lock-free queue overflow)
    PendingQueueFull { size: usize, max: usize },
    
    /// Run queue is full
    RunQueueFull { queue_type: QueueErrorType, size: usize },
    
    /// Queue corruption detected
    QueueCorrupted { queue_type: QueueErrorType, reason: &'static str },
    
    // ═══════════════════════════════════════════════════════════════
    // Thread State Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Thread not found in scheduler
    ThreadNotFound { thread_id: u64 },
    
    /// Invalid thread state transition
    InvalidStateTransition { 
        thread_id: u64, 
        from: ThreadStateError, 
        to: ThreadStateError 
    },
    
    /// Thread already exists (duplicate ID)
    ThreadAlreadyExists { thread_id: u64 },
    
    /// Thread is a zombie (cannot be scheduled)
    ThreadIsZombie { thread_id: u64 },
    
    // ═══════════════════════════════════════════════════════════════
    // Memory Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Out of memory for thread allocation
    OutOfMemory { requested: usize, available: usize },
    
    /// Stack allocation failed
    StackAllocationFailed { size: usize },
    
    /// Context allocation failed
    ContextAllocationFailed,
    
    // ═══════════════════════════════════════════════════════════════
    // Affinity Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Invalid CPU mask (no valid CPUs)
    InvalidCpuMask,
    
    /// CPU not available (offline or doesn't exist)
    CpuNotAvailable { cpu_id: usize },
    
    /// Migration not allowed (thread pinned)
    MigrationNotAllowed { thread_id: u64, from_cpu: usize, to_cpu: usize },
    
    // ═══════════════════════════════════════════════════════════════
    // Priority Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Invalid priority value
    InvalidPriority { value: i32, min: i32, max: i32 },
    
    /// Permission denied for priority change
    PriorityPermissionDenied { thread_id: u64, requested: i32 },
    
    /// Realtime priority requires CAP_SYS_NICE
    RealtimePermissionDenied { thread_id: u64 },
    
    // ═══════════════════════════════════════════════════════════════
    // Locking Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Deadlock detected
    DeadlockDetected { thread_id: u64, lock_id: u64 },
    
    /// Lock contention too high
    HighContention { retries: u64, operation: &'static str },
    
    /// CAS operation failed too many times
    CasRetryExhausted { retries: u64, max: u64 },
    
    // ═══════════════════════════════════════════════════════════════
    // Scheduling Policy Errors
    // ═══════════════════════════════════════════════════════════════
    
    /// Invalid scheduling policy
    InvalidPolicy { policy: u32 },
    
    /// Policy not supported
    PolicyNotSupported { policy: &'static str },
    
    /// Deadline missed (for EDF scheduling)
    DeadlineMissed { thread_id: u64, deadline_ns: u64, actual_ns: u64 },
    
    // ═══════════════════════════════════════════════════════════════
    // Internal Errors (should never happen)
    // ═══════════════════════════════════════════════════════════════
    
    /// Internal scheduler invariant violated
    InternalError { reason: &'static str },
    
    /// Scheduler not initialized
    NotInitialized,
    
    /// Operation interrupted (by signal or shutdown)
    Interrupted,
}

/// Queue type for error context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueErrorType {
    Hot,
    Normal,
    Cold,
    Pending,
    Blocked,
    Zombie,
}

/// Thread state for error context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStateError {
    Created,
    Ready,
    Running,
    Blocked,
    Sleeping,
    Terminated,
    Zombie,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreadLimitReached { current, max } => {
                write!(f, "Thread limit reached: {}/{}", current, max)
            }
            Self::ProcessThreadLimit { pid, current, max } => {
                write!(f, "Process {} thread limit: {}/{}", pid, current, max)
            }
            Self::PendingQueueFull { size, max } => {
                write!(f, "Pending queue full: {}/{}", size, max)
            }
            Self::ThreadNotFound { thread_id } => {
                write!(f, "Thread {} not found", thread_id)
            }
            Self::OutOfMemory { requested, available } => {
                write!(f, "OOM: requested {} bytes, {} available", requested, available)
            }
            Self::DeadlockDetected { thread_id, lock_id } => {
                write!(f, "Deadlock: thread {} on lock {}", thread_id, lock_id)
            }
            Self::HighContention { retries, operation } => {
                write!(f, "High contention on {}: {} retries", operation, retries)
            }
            _ => write!(f, "{:?}", self),
        }
    }
}

impl SchedulerError {
    /// Get recovery hint for this error
    pub fn recovery_hint(&self) -> &'static str {
        match self {
            Self::ThreadLimitReached { .. } => "Wait for threads to exit or increase limit",
            Self::PendingQueueFull { .. } => "Reduce fork rate or increase pending queue size",
            Self::ThreadNotFound { .. } => "Thread may have already terminated",
            Self::OutOfMemory { .. } => "Free memory or reduce stack sizes",
            Self::DeadlockDetected { .. } => "Review lock ordering, possible priority inversion",
            Self::HighContention { .. } => "Consider reducing concurrency or using different algorithm",
            Self::InvalidStateTransition { .. } => "Check thread lifecycle management",
            Self::DeadlineMissed { .. } => "Reduce workload or increase deadline",
            _ => "Check scheduler configuration",
        }
    }
    
    /// Is this a recoverable error?
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::InternalError { .. } => false,
            Self::QueueCorrupted { .. } => false,
            Self::NotInitialized => false,
            _ => true,
        }
    }
    
    /// Should this error be logged?
    pub fn should_log(&self) -> bool {
        match self {
            Self::ThreadNotFound { .. } => false, // Common during cleanup
            Self::Interrupted => false,           // Expected behavior
            _ => true,
        }
    }
    
    /// Get error severity (0-3)
    pub fn severity(&self) -> u8 {
        match self {
            Self::InternalError { .. } => 3,      // Critical
            Self::QueueCorrupted { .. } => 3,
            Self::DeadlockDetected { .. } => 3,
            Self::OutOfMemory { .. } => 2,        // Severe
            Self::ThreadLimitReached { .. } => 2,
            Self::DeadlineMissed { .. } => 2,
            Self::HighContention { .. } => 1,     // Warning
            Self::InvalidStateTransition { .. } => 1,
            _ => 0,                               // Info
        }
    }
}

/// Result type for scheduler operations
pub type SchedulerResult<T> = Result<T, SchedulerError>;

/// Macro to log scheduler errors with context
#[macro_export]
macro_rules! sched_error {
    ($err:expr) => {{
        let err = $err;
        if err.should_log() {
            crate::logger::error(&alloc::format!(
                "[SCHED] Error: {} (hint: {})",
                err, err.recovery_hint()
            ));
        }
        err
    }};
}

/// Macro for critical scheduler assertions
#[macro_export]
macro_rules! sched_assert {
    ($cond:expr, $reason:expr) => {
        if !$cond {
            panic!("[SCHED CRITICAL] Invariant violated: {}", $reason);
        }
    };
}
