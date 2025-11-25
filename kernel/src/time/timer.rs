//! Software timers subsystem
//! 
//! Provides one-shot and periodic timers with callbacks

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Timer ID (unique identifier)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(pub u64);

/// Timer callback function
pub type TimerCallback = Box<dyn FnMut() + Send>;

/// Timer type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerType {
    /// One-shot timer (fires once)
    OneShot,
    /// Periodic timer (fires repeatedly)
    Periodic,
}

/// Timer state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    /// Timer is pending (waiting to fire)
    Pending,
    /// Timer has fired and is being processed
    Firing,
    /// Timer is cancelled
    Cancelled,
    /// Timer is completed (one-shot only)
    Completed,
}

/// Timer structure
struct TimerEntry {
    id: TimerId,
    timer_type: TimerType,
    state: TimerState,
    deadline_ns: u64,
    interval_ns: u64,
    callback: Option<TimerCallback>,
}

impl TimerEntry {
    fn new(
        id: TimerId,
        timer_type: TimerType,
        deadline_ns: u64,
        interval_ns: u64,
        callback: TimerCallback,
    ) -> Self {
        Self {
            id,
            timer_type,
            state: TimerState::Pending,
            deadline_ns,
            interval_ns,
            callback: Some(callback),
        }
    }
    
    fn is_ready(&self, now_ns: u64) -> bool {
        self.state == TimerState::Pending && now_ns >= self.deadline_ns
    }
    
    fn fire(&mut self) {
        self.state = TimerState::Firing;
        
        // Call callback
        if let Some(mut callback) = self.callback.take() {
            callback();
            self.callback = Some(callback);
        }
        
        // Update state
        match self.timer_type {
            TimerType::OneShot => {
                self.state = TimerState::Completed;
            }
            TimerType::Periodic => {
                // Reschedule
                self.deadline_ns += self.interval_ns;
                self.state = TimerState::Pending;
            }
        }
    }
}

/// Timer manager
struct TimerManager {
    timers: Vec<TimerEntry>,
    next_id: u64,
}

impl TimerManager {
    fn new() -> Self {
        Self {
            timers: Vec::new(),
            next_id: 1,
        }
    }
    
    fn alloc_id(&mut self) -> TimerId {
        let id = TimerId(self.next_id);
        self.next_id += 1;
        id
    }
    
    fn add_timer(
        &mut self,
        timer_type: TimerType,
        delay_ns: u64,
        interval_ns: u64,
        callback: TimerCallback,
    ) -> TimerId {
        let id = self.alloc_id();
        let now_ns = super::uptime_ns();
        let deadline_ns = now_ns + delay_ns;
        
        let timer = TimerEntry::new(id, timer_type, deadline_ns, interval_ns, callback);
        self.timers.push(timer);
        
        id
    }
    
    fn cancel_timer(&mut self, id: TimerId) -> bool {
        if let Some(timer) = self.timers.iter_mut().find(|t| t.id == id) {
            timer.state = TimerState::Cancelled;
            true
        } else {
            false
        }
    }
    
    fn tick(&mut self, now_ns: u64) {
        // Find ready timers
        let ready_indices: Vec<usize> = self.timers
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_ready(now_ns))
            .map(|(i, _)| i)
            .collect();
        
        // Fire ready timers
        for idx in ready_indices {
            self.timers[idx].fire();
        }
        
        // Remove completed/cancelled timers
        self.timers.retain(|t| {
            t.state != TimerState::Completed && t.state != TimerState::Cancelled
        });
    }
}

/// Global timer manager
static TIMER_MANAGER: Mutex<Option<TimerManager>> = Mutex::new(None);

/// Initialize timer subsystem
pub fn init() {
    let mut manager = TIMER_MANAGER.lock();
    *manager = Some(TimerManager::new());
}

/// Create a one-shot timer
pub fn set_timer_once<F>(delay_ns: u64, callback: F) -> TimerId
where
    F: FnMut() + Send + 'static,
{
    let mut manager = TIMER_MANAGER.lock();
    let manager = manager.as_mut().expect("Timer manager not initialized");
    
    manager.add_timer(
        TimerType::OneShot,
        delay_ns,
        0,
        Box::new(callback),
    )
}

/// Create a periodic timer
pub fn set_timer_periodic<F>(interval_ns: u64, callback: F) -> TimerId
where
    F: FnMut() + Send + 'static,
{
    let mut manager = TIMER_MANAGER.lock();
    let manager = manager.as_mut().expect("Timer manager not initialized");
    
    manager.add_timer(
        TimerType::Periodic,
        interval_ns,
        interval_ns,
        Box::new(callback),
    )
}

/// Cancel a timer
pub fn cancel_timer(id: TimerId) -> bool {
    let mut manager = TIMER_MANAGER.lock();
    if let Some(manager) = manager.as_mut() {
        manager.cancel_timer(id)
    } else {
        false
    }
}

/// Process timers (called from timer interrupt)
pub fn tick() {
    let now_ns = super::uptime_ns();
    let mut manager = TIMER_MANAGER.lock();
    if let Some(manager) = manager.as_mut() {
        manager.tick(now_ns);
    }
}

/// Timer convenience wrapper
pub struct Timer {
    id: TimerId,
}

impl Timer {
    /// Create one-shot timer
    pub fn once<F>(delay_ns: u64, callback: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        let id = set_timer_once(delay_ns, callback);
        Self { id }
    }
    
    /// Create periodic timer
    pub fn periodic<F>(interval_ns: u64, callback: F) -> Self
    where
        F: FnMut() + Send + 'static,
    {
        let id = set_timer_periodic(interval_ns, callback);
        Self { id }
    }
    
    /// Cancel timer
    pub fn cancel(self) -> bool {
        cancel_timer(self.id)
    }
    
    /// Get timer ID
    pub fn id(&self) -> TimerId {
        self.id
    }
}

/// Set timer (shorthand for one-shot)
pub fn set_timer<F>(delay_ns: u64, callback: F) -> TimerId
where
    F: FnMut() + Send + 'static,
{
    set_timer_once(delay_ns, callback)
}
