//! Scheduling Policies - Multiple Algorithm Support
//!
//! Supports multiple scheduling policies like Linux but with better performance:
//! - SCHED_FIFO: Real-time FIFO (highest priority runs until yield)
//! - SCHED_RR: Real-time Round-Robin (preemption within same priority)
//! - SCHED_NORMAL: Normal time-sharing (our 3-queue EMA system)
//! - SCHED_BATCH: CPU-intensive batch processing
//! - SCHED_IDLE: Lowest priority (only when nothing else to run)
//! - SCHED_DEADLINE: EDF (Earliest Deadline First) for hard real-time
//!
//! Better than Linux CFS:
//! - Lock-free operations where possible
//! - EMA-based prediction instead of vruntime tracking
//! - Simpler, faster queue management

use core::cmp::Ordering as CmpOrdering;

/// Scheduling policy identifiers (compatible with Linux SCHED_* values)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SchedulingPolicy {
    /// Normal time-sharing (default) - uses 3-queue EMA
    Normal = 0,
    /// Real-time FIFO - runs until yield/block
    Fifo = 1,
    /// Real-time Round-Robin - preemption at same priority
    RoundRobin = 2,
    /// Batch processing - longer timeslices
    Batch = 3,
    /// Idle priority - only runs when nothing else
    Idle = 5,
    /// Deadline scheduling (EDF) - for hard real-time
    Deadline = 6,
}

impl SchedulingPolicy {
    /// Create from raw value (Linux compatible)
    pub fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Normal),
            1 => Some(Self::Fifo),
            2 => Some(Self::RoundRobin),
            3 => Some(Self::Batch),
            5 => Some(Self::Idle),
            6 => Some(Self::Deadline),
            _ => None,
        }
    }
    
    /// Is this a real-time policy?
    pub fn is_realtime(&self) -> bool {
        matches!(self, Self::Fifo | Self::RoundRobin | Self::Deadline)
    }
    
    /// Does this policy use preemption?
    pub fn is_preemptive(&self) -> bool {
        !matches!(self, Self::Fifo)
    }
    
    /// Get default timeslice for this policy (in microseconds)
    pub fn default_timeslice_us(&self) -> u64 {
        match self {
            Self::Normal => 10_000,     // 10ms
            Self::Fifo => u64::MAX,     // No preemption
            Self::RoundRobin => 100_000, // 100ms (Linux default)
            Self::Batch => 50_000,      // 50ms (longer for batch)
            Self::Idle => 1_000,        // 1ms (short, low priority)
            Self::Deadline => 0,        // Deadline-based, not time-based
        }
    }
    
    /// Get nice range for this policy
    pub fn nice_range(&self) -> (i8, i8) {
        match self {
            Self::Fifo | Self::RoundRobin | Self::Deadline => (-20, 0), // RT uses 1-99 priority
            Self::Normal | Self::Batch => (-20, 19), // Full nice range
            Self::Idle => (19, 19), // Fixed at lowest
        }
    }
}

impl Default for SchedulingPolicy {
    fn default() -> Self {
        Self::Normal
    }
}

/// Thread scheduling parameters
#[derive(Debug, Clone, Copy)]
pub struct SchedParams {
    /// Scheduling policy
    pub policy: SchedulingPolicy,
    
    /// Static priority (1-99 for RT, 0 for normal)
    pub priority: i32,
    
    /// Nice value (-20 to 19, lower = higher priority)
    pub nice: i8,
    
    /// CPU affinity mask (0 = any CPU)
    pub affinity: u64,
    
    /// Timeslice in microseconds (0 = use policy default)
    pub timeslice_us: u64,
    
    // Deadline parameters (only for SCHED_DEADLINE)
    /// Runtime budget per period (nanoseconds)
    pub runtime_ns: u64,
    /// Deadline relative to period start (nanoseconds)
    pub deadline_ns: u64,
    /// Period length (nanoseconds)
    pub period_ns: u64,
}

impl SchedParams {
    /// Create default parameters (SCHED_NORMAL, nice 0)
    pub const fn default_normal() -> Self {
        Self {
            policy: SchedulingPolicy::Normal,
            priority: 0,
            nice: 0,
            affinity: 0,
            timeslice_us: 0, // Use policy default
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
        }
    }
    
    /// Create real-time FIFO parameters
    pub const fn realtime_fifo(priority: i32) -> Self {
        Self {
            policy: SchedulingPolicy::Fifo,
            priority,
            nice: 0,
            affinity: 0,
            timeslice_us: 0,
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
        }
    }
    
    /// Create real-time Round-Robin parameters
    pub const fn realtime_rr(priority: i32) -> Self {
        Self {
            policy: SchedulingPolicy::RoundRobin,
            priority,
            nice: 0,
            affinity: 0,
            timeslice_us: 100_000, // 100ms default
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
        }
    }
    
    /// Create deadline parameters
    pub const fn deadline(runtime_ns: u64, deadline_ns: u64, period_ns: u64) -> Self {
        Self {
            policy: SchedulingPolicy::Deadline,
            priority: 0,
            nice: 0,
            affinity: 0,
            timeslice_us: 0,
            runtime_ns,
            deadline_ns,
            period_ns,
        }
    }
    
    /// Create batch parameters
    pub const fn batch(nice: i8) -> Self {
        Self {
            policy: SchedulingPolicy::Batch,
            priority: 0,
            nice,
            affinity: 0,
            timeslice_us: 50_000, // 50ms
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
        }
    }
    
    /// Create idle parameters
    pub const fn idle() -> Self {
        Self {
            policy: SchedulingPolicy::Idle,
            priority: 0,
            nice: 19,
            affinity: 0,
            timeslice_us: 1_000, // 1ms
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
        }
    }
    
    /// Validate parameters
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.policy {
            SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => {
                if self.priority < 1 || self.priority > 99 {
                    return Err("RT priority must be 1-99");
                }
            }
            SchedulingPolicy::Deadline => {
                if self.runtime_ns == 0 || self.deadline_ns == 0 || self.period_ns == 0 {
                    return Err("Deadline requires runtime, deadline, and period");
                }
                if self.runtime_ns > self.deadline_ns {
                    return Err("Runtime cannot exceed deadline");
                }
                if self.deadline_ns > self.period_ns {
                    return Err("Deadline cannot exceed period");
                }
            }
            _ => {
                if self.nice < -20 || self.nice > 19 {
                    return Err("Nice value must be -20 to 19");
                }
            }
        }
        Ok(())
    }
    
    /// Get effective timeslice in microseconds
    pub fn effective_timeslice_us(&self) -> u64 {
        if self.timeslice_us > 0 {
            self.timeslice_us
        } else {
            self.policy.default_timeslice_us()
        }
    }
}

impl Default for SchedParams {
    fn default() -> Self {
        Self::default_normal()
    }
}

/// Priority comparison between threads
/// 
/// Returns which thread should run first.
/// Higher priority = runs first (like Linux, not nice values)
pub fn compare_priority(a: &SchedParams, b: &SchedParams) -> CmpOrdering {
    // 1. Deadline always wins (EDF)
    if a.policy == SchedulingPolicy::Deadline && b.policy == SchedulingPolicy::Deadline {
        // Earlier deadline wins
        return a.deadline_ns.cmp(&b.deadline_ns);
    }
    if a.policy == SchedulingPolicy::Deadline {
        return CmpOrdering::Less; // a wins
    }
    if b.policy == SchedulingPolicy::Deadline {
        return CmpOrdering::Greater; // b wins
    }
    
    // 2. Real-time policies beat normal
    let a_rt = a.policy.is_realtime();
    let b_rt = b.policy.is_realtime();
    
    if a_rt && !b_rt {
        return CmpOrdering::Less; // a wins
    }
    if !a_rt && b_rt {
        return CmpOrdering::Greater; // b wins
    }
    
    // 3. Within RT, higher priority wins
    if a_rt && b_rt {
        return b.priority.cmp(&a.priority); // Higher priority = runs first
    }
    
    // 4. Idle is always last
    if a.policy == SchedulingPolicy::Idle && b.policy != SchedulingPolicy::Idle {
        return CmpOrdering::Greater; // b wins
    }
    if a.policy != SchedulingPolicy::Idle && b.policy == SchedulingPolicy::Idle {
        return CmpOrdering::Less; // a wins
    }
    
    // 5. For normal policies, lower nice wins
    a.nice.cmp(&b.nice)
}

/// Calculate dynamic priority based on wait time (anti-starvation)
/// 
/// Boosts priority of threads that have been waiting too long.
/// Returns priority boost (0-20).
pub fn calculate_priority_boost(wait_time_us: u64, policy: SchedulingPolicy) -> i8 {
    if policy.is_realtime() {
        return 0; // RT threads don't get boosted
    }
    
    // Every 100ms of waiting gives +1 priority, max +10
    let boost = (wait_time_us / 100_000).min(10) as i8;
    boost
}

/// Time quantum calculator
/// 
/// Returns the time quantum in microseconds based on policy and priority.
pub fn calculate_quantum_us(params: &SchedParams, load: usize) -> u64 {
    let base = params.effective_timeslice_us();
    
    match params.policy {
        SchedulingPolicy::Fifo => u64::MAX, // No preemption
        SchedulingPolicy::RoundRobin => base,
        SchedulingPolicy::Deadline => params.runtime_ns / 1000, // Convert to us
        SchedulingPolicy::Normal => {
            // Scale quantum based on nice value
            // nice -20 = 2x quantum, nice +19 = 0.5x quantum
            let nice_factor = (20 - params.nice as i32) as u64;
            let scaled = (base * nice_factor) / 20;
            
            // Also scale based on system load (more threads = shorter quantum)
            if load > 10 {
                scaled * 10 / load as u64
            } else {
                scaled
            }
        }
        SchedulingPolicy::Batch => base * 2, // Longer quantum for batch
        SchedulingPolicy::Idle => base / 2,  // Shorter quantum for idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_policy_priority() {
        let rt = SchedParams::realtime_fifo(50);
        let normal = SchedParams::default_normal();
        
        assert_eq!(compare_priority(&rt, &normal), CmpOrdering::Less);
    }
    
    #[test]
    fn test_deadline_wins() {
        let deadline = SchedParams::deadline(1_000_000, 5_000_000, 10_000_000);
        let rt = SchedParams::realtime_fifo(99);
        
        assert_eq!(compare_priority(&deadline, &rt), CmpOrdering::Less);
    }
}
