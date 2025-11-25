//! Real-time priority management

/// Maximum real-time priority
pub const RT_PRIORITY_MAX: u8 = 99;

/// Minimum real-time priority
pub const RT_PRIORITY_MIN: u8 = 1;

/// Real-time priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RealtimePriority(pub u8);

impl RealtimePriority {
    pub fn new(priority: u8) -> Option<Self> {
        if priority >= RT_PRIORITY_MIN && priority <= RT_PRIORITY_MAX {
            Some(Self(priority))
        } else {
            None
        }
    }
    
    pub fn max() -> Self {
        Self(RT_PRIORITY_MAX)
    }
    
    pub fn min() -> Self {
        Self(RT_PRIORITY_MIN)
    }
    
    pub fn value(&self) -> u8 {
        self.0
    }
}

impl Default for RealtimePriority {
    fn default() -> Self {
        Self(RT_PRIORITY_MIN)
    }
}
