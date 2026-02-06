//! Process ID type
//!
//! Type-safe process identifier with validation and special PID constants.

use core::fmt;

/// Kernel process PID (always 0)
pub const KERNEL_PID: Pid = Pid(0);

/// Init process PID (always 1)
pub const INIT_PID: Pid = Pid(1);

/// Maximum valid PID on most systems
pub const MAX_PID: i32 = 32767;

/// Process ID
///
/// Wraps an i32 with type safety and validation.
/// On Unix-like systems, PIDs are positive integers (except special cases like 0).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Pid(i32);

impl Pid {
    /// Create a new PID with validation
    ///
    /// # Panics
    /// Panics in debug mode if PID is negative (except allowed special cases)
    #[inline(always)]
    pub const fn new(pid: i32) -> Self {
        debug_assert!(pid >= 0, "PID must be non-negative");
        Pid(pid)
    }

    /// Create PID without validation (unsafe)
    ///
    /// # Safety
    /// Caller must ensure PID is valid for the target system
    #[inline(always)]
    pub const unsafe fn new_unchecked(pid: i32) -> Self {
        Pid(pid)
    }

    /// Try to create PID, returning None if invalid
    #[inline(always)]
    pub const fn try_new(pid: i32) -> Option<Self> {
        if pid >= 0 {
            Some(Pid(pid))
        } else {
            None
        }
    }

    /// Get raw PID value
    #[inline(always)]
    pub const fn as_i32(self) -> i32 {
        self.0
    }

    /// Get as usize (for array indexing)
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Check if this is the kernel PID (0)
    #[inline(always)]
    pub const fn is_kernel(self) -> bool {
        self.0 == 0
    }

    /// Check if this is the init PID (1)
    #[inline(always)]
    pub const fn is_init(self) -> bool {
        self.0 == 1
    }

    /// Check if this is a special system PID (0 or 1)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        self.0 <= 1
    }

    /// Check if PID is valid (non-negative)
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 >= 0
    }

    /// Increment PID (for PID allocation)
    #[inline(always)]
    pub const fn increment(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(new_pid) if new_pid > 0 && new_pid <= MAX_PID => Some(Pid(new_pid)),
            _ => None,
        }
    }
}

impl fmt::Debug for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_kernel() {
            f.write_str("Pid(KERNEL)")
        } else if self.is_init() {
            f.write_str("Pid(INIT)")
        } else {
            f.debug_tuple("Pid").field(&self.0).finish()
        }
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<i32> for Pid {
    type Error = ();

    #[inline(always)]
    fn try_from(pid: i32) -> Result<Self, Self::Error> {
        Self::try_new(pid).ok_or(())
    }
}

impl From<Pid> for i32 {
    #[inline(always)]
    fn from(pid: Pid) -> Self {
        pid.0
    }
}

impl From<Pid> for usize {
    #[inline(always)]
    fn from(pid: Pid) -> Self {
        pid.0 as usize
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;

    #[test]
    fn test_pid_creation() {
        let pid = Pid::new(123);
        assert_eq!(pid.as_i32(), 123);

        let kernel = Pid::new(0);
        assert_eq!(kernel, KERNEL_PID);
        assert!(kernel.is_kernel());

        let init = Pid::new(1);
        assert_eq!(init, INIT_PID);
        assert!(init.is_init());
    }

    #[test]
    fn test_pid_try_new() {
        assert!(Pid::try_new(0).is_some());
        assert!(Pid::try_new(1).is_some());
        assert!(Pid::try_new(12345).is_some());
        assert!(Pid::try_new(-1).is_none());
        assert!(Pid::try_new(-100).is_none());
    }

    #[test]
    fn test_pid_special_checks() {
        assert!(KERNEL_PID.is_kernel());
        assert!(!KERNEL_PID.is_init());
        assert!(KERNEL_PID.is_system());

        assert!(!INIT_PID.is_kernel());
        assert!(INIT_PID.is_init());
        assert!(INIT_PID.is_system());

        let user_pid = Pid::new(100);
        assert!(!user_pid.is_kernel());
        assert!(!user_pid.is_init());
        assert!(!user_pid.is_system());
    }

    #[test]
    fn test_pid_valid() {
        assert!(Pid::new(0).is_valid());
        assert!(Pid::new(1).is_valid());
        assert!(Pid::new(MAX_PID).is_valid());
    }

    #[test]
    fn test_pid_increment() {
        let pid = Pid::new(100);
        assert_eq!(pid.increment(), Some(Pid::new(101)));

        let max = Pid::new(MAX_PID);
        assert_eq!(max.increment(), None);

        let kernel = KERNEL_PID;
        assert_eq!(kernel.increment(), Some(Pid::new(1)));
    }

    #[test]
    fn test_pid_conversions() {
        let pid = Pid::new(123);

        assert_eq!(pid.as_i32(), 123);
        assert_eq!(pid.as_usize(), 123);

        let i32_val: i32 = pid.into();
        assert_eq!(i32_val, 123);

        let usize_val: usize = pid.into();
        assert_eq!(usize_val, 123);

        assert_eq!(Pid::try_from(123).unwrap(), pid);
        assert!(Pid::try_from(-1).is_err());
    }

    #[test]
    fn test_pid_display() {
        let pid = Pid::new(123);
        assert_eq!(std::format!("{}", pid), "123");

        let debug = std::format!("{:?}", KERNEL_PID);
        assert!(debug.contains("KERNEL"));

        let debug = std::format!("{:?}", INIT_PID);
        assert!(debug.contains("INIT"));
    }

    #[test]
    fn test_pid_ordering() {
        let pid1 = Pid::new(10);
        let pid2 = Pid::new(20);
        let pid3 = Pid::new(10);

        assert!(pid1 < pid2);
        assert!(pid2 > pid1);
        assert_eq!(pid1, pid3);
        assert_ne!(pid1, pid2);
    }

    #[test]
    fn test_pid_size() {
        assert_eq!(size_of::<Pid>(), size_of::<i32>());
    }
}
