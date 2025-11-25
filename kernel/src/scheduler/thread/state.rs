//! State - Thread state machine
//!
//! Manages thread lifecycle and state transitions

use core::sync::atomic::{AtomicU64, Ordering};
use core::fmt;

/// Thread state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ThreadState {
    /// Thread is ready to run
    Ready = 0,
    
    /// Thread is currently running
    Running = 1,
    
    /// Thread is blocked (waiting for I/O, lock, etc.)
    Blocked = 2,
    
    /// Thread is sleeping
    Sleeping = 3,
    
    /// Thread has terminated
    Terminated = 4,
    
    /// Thread is being created
    Creating = 5,
    
    /// Thread is suspended
    Suspended = 6,
}

impl ThreadState {
    /// Convert from u64
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Ready),
            1 => Some(Self::Running),
            2 => Some(Self::Blocked),
            3 => Some(Self::Sleeping),
            4 => Some(Self::Terminated),
            5 => Some(Self::Creating),
            6 => Some(Self::Suspended),
            _ => None,
        }
    }
    
    /// Convert to u64
    pub fn to_u64(self) -> u64 {
        self as u64
    }
    
    /// Check if state is schedulable
    pub fn is_schedulable(self) -> bool {
        matches!(self, Self::Ready)
    }
    
    /// Check if state is active
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Ready)
    }
}

impl fmt::Display for ThreadState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Ready => write!(f, "Ready"),
            Self::Running => write!(f, "Running"),
            Self::Blocked => write!(f, "Blocked"),
            Self::Sleeping => write!(f, "Sleeping"),
            Self::Terminated => write!(f, "Terminated"),
            Self::Creating => write!(f, "Creating"),
            Self::Suspended => write!(f, "Suspended"),
        }
    }
}

/// Atomic thread state
pub struct AtomicThreadState {
    state: AtomicU64,
}

impl AtomicThreadState {
    /// Create new atomic state
    pub const fn new(state: ThreadState) -> Self {
        Self {
            state: AtomicU64::new(state as u64),
        }
    }
    
    /// Load current state
    pub fn load(&self) -> ThreadState {
        let value = self.state.load(Ordering::Acquire);
        ThreadState::from_u64(value).unwrap_or(ThreadState::Ready)
    }
    
    /// Store new state
    pub fn store(&self, state: ThreadState) {
        self.state.store(state as u64, Ordering::Release);
    }
    
    /// Compare and exchange state
    pub fn compare_exchange(
        &self,
        current: ThreadState,
        new: ThreadState,
    ) -> Result<ThreadState, ThreadState> {
        match self.state.compare_exchange(
            current as u64,
            new as u64,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(old) => Ok(ThreadState::from_u64(old).unwrap()),
            Err(actual) => Err(ThreadState::from_u64(actual).unwrap()),
        }
    }
}

/// State transition result
pub enum TransitionResult {
    /// Transition succeeded
    Success,
    
    /// Invalid transition
    Invalid,
    
    /// Race condition (retry)
    Retry,
}

/// Validate state transition
pub fn validate_transition(from: ThreadState, to: ThreadState) -> bool {
    use ThreadState::*;
    
    match (from, to) {
        // Creating -> Ready
        (Creating, Ready) => true,
        
        // Ready -> Running
        (Ready, Running) => true,
        
        // Running -> Ready (preemption)
        (Running, Ready) => true,
        
        // Running -> Blocked
        (Running, Blocked) => true,
        
        // Running -> Sleeping
        (Running, Sleeping) => true,
        
        // Running -> Terminated
        (Running, Terminated) => true,
        
        // Blocked -> Ready
        (Blocked, Ready) => true,
        
        // Sleeping -> Ready
        (Sleeping, Ready) => true,
        
        // Any -> Suspended
        (_, Suspended) => true,
        
        // Suspended -> Ready
        (Suspended, Ready) => true,
        
        // All other transitions invalid
        _ => false,
    }
}
