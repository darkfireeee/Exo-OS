// libs/exo_types/src/pid.rs
//! Process ID type-safe wrapper

use core::fmt;
use core::num::NonZeroU32;

/// Process ID (type-safe wrapper around u32)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Pid(NonZeroU32);

impl Pid {
    /// Minimum valid PID (1)
    pub const MIN: u32 = 1;
    
    /// Maximum valid PID
    pub const MAX: u32 = u32::MAX - 1;
    
    /// Init process PID
    pub const INIT: Pid = unsafe { Pid(NonZeroU32::new_unchecked(1)) };
    
    /// Kernel PID (reserved)
    pub const KERNEL: Pid = unsafe { Pid(NonZeroU32::new_unchecked(0xFFFF_FFFF)) };
    
    /// Create new PID from raw value
    ///
    /// Returns None if value is 0 or MAX
    #[inline]
    pub const fn new(pid: u32) -> Option<Self> {
        if pid == 0 || pid > Self::MAX {
            None
        } else {
            // Safety: checked above
            Some(unsafe { Self::from_raw_unchecked(pid) })
        }
    }
    
    /// Create PID from raw value without validation
    ///
    /// # Safety
    /// Caller must ensure pid is in range [1, MAX]
    #[inline]
    pub const unsafe fn from_raw_unchecked(pid: u32) -> Self {
        Self(NonZeroU32::new_unchecked(pid))
    }
    
    /// Get raw PID value
    #[inline]
    pub const fn as_raw(self) -> u32 {
        self.0.get()
    }
    
    /// Get current process PID (syscall stub)
    #[inline]
    pub fn current() -> Self {
        // TODO: Real syscall
        Self::INIT
    }
    
    /// Check if this is the init process
    #[inline]
    pub const fn is_init(self) -> bool {
        self.0.get() == 1
    }
    
    /// Check if this is a kernel PID
    #[inline]
    pub const fn is_kernel(self) -> bool {
        self.0.get() == 0xFFFF_FFFF
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Pid> for u32 {
    #[inline]
    fn from(pid: Pid) -> u32 {
        pid.as_raw()
    }
}

impl TryFrom<u32> for Pid {
    type Error = ();
    
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_pid_creation() {
        assert!(Pid::new(0).is_none());
        assert!(Pid::new(1).is_some());
        assert!(Pid::new(Pid::MAX).is_some());
        assert!(Pid::new(Pid::MAX + 1).is_none());
    }
    
    #[test]
    fn test_pid_constants() {
        assert_eq!(Pid::INIT.as_raw(), 1);
        assert!(Pid::INIT.is_init());
        assert!(!Pid::KERNEL.is_init());
        assert!(Pid::KERNEL.is_kernel());
    }
}
