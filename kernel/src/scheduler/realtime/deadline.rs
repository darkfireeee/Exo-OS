//! Deadline scheduling

use core::cmp::Ordering;

/// Deadline for real-time task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    /// Absolute deadline (nanoseconds since boot)
    pub deadline_ns: u64,
    /// Period (for periodic tasks)
    pub period_ns: u64,
}

impl Deadline {
    pub fn new(deadline_ns: u64, period_ns: u64) -> Self {
        Self {
            deadline_ns,
            period_ns,
        }
    }
    
    /// Check if deadline has passed
    pub fn is_overdue(&self, now_ns: u64) -> bool {
        now_ns > self.deadline_ns
    }
    
    /// Get remaining time to deadline
    pub fn remaining(&self, now_ns: u64) -> u64 {
        if now_ns >= self.deadline_ns {
            0
        } else {
            self.deadline_ns - now_ns
        }
    }
    
    /// Advance to next period
    pub fn next_period(&mut self) {
        self.deadline_ns += self.period_ns;
    }
}

impl PartialOrd for Deadline {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Deadline {
    fn cmp(&self, other: &Self) -> Ordering {
        self.deadline_ns.cmp(&other.deadline_ns)
    }
}

/// Deadline scheduler (Earliest Deadline First)
pub struct DeadlineScheduler;

impl DeadlineScheduler {
    /// Compare two deadlines for EDF scheduling
    pub fn compare(a: &Deadline, b: &Deadline) -> Ordering {
        a.cmp(b)
    }
}
