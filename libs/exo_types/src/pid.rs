<<<<<<< Updated upstream
//! Process ID type-safe wrapper
//!
//! Provides a type-safe wrapper around process IDs with compile-time
//! guarantees and zero runtime overhead.

use core::fmt;
use core::num::NonZeroU32;

/// Process ID (zero-cost type-safe wrapper)
///
/// Uses NonZeroU32 internally to guarantee valid PIDs and enable
/// niche optimization (Option<Pid> is same size as Pid).
///
/// # Valid PID range
/// - 1: Init process (reserved)
/// - 2-65535: Regular processes
/// - 65536+: System processes (kernel threads, etc.)
/// - 0: Invalid (cannot be created)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Pid(NonZeroU32);

impl Pid {
    /// Minimum valid PID
    pub const MIN: u32 = 1;
    
    /// Maximum regular user PID (16-bit range)
    pub const MAX_USER: u32 = 0xFFFF;
    
    /// Maximum valid PID
    pub const MAX: u32 = u32::MAX - 1;
    
    /// Init process PID (always 1)
    pub const INIT: Self = unsafe { Self(NonZeroU32::new_unchecked(1)) };
    
    /// First user PID (typically for non-system processes)
    pub const FIRST_USER: Self = unsafe { Self(NonZeroU32::new_unchecked(1000)) };
    
    /// Create new PID from raw value
    ///
    /// Returns None if value is 0 or exceeds MAX.
    ///
    /// # Examples
    /// ```
    /// use exo_types::Pid;
    /// 
    /// assert!(Pid::new(0).is_none());
    /// assert!(Pid::new(1).is_some());
    /// assert_eq!(Pid::new(42).unwrap().as_raw(), 42);
    /// ```
    #[inline(always)]
    pub const fn new(pid: u32) -> Option<Self> {
        if pid == 0 || pid > Self::MAX {
            None
        } else {
            Some(unsafe { Self::from_raw_unchecked(pid) })
        }
    }
    
    /// Create PID from raw value without validation
    ///
    /// # Safety
    /// Caller must ensure `pid` is in range [1, MAX].
    #[inline(always)]
    pub const unsafe fn from_raw_unchecked(pid: u32) -> Self {
        Self(NonZeroU32::new_unchecked(pid))
    }
    
    /// Get raw PID value
    #[inline(always)]
    pub const fn as_raw(self) -> u32 {
        self.0.get()
    }
    
    /// Get as usize
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0.get() as usize
    }
    
    /// Check if this is the init process (PID 1)
    #[inline(always)]
    pub const fn is_init(self) -> bool {
        self.0.get() == 1
    }
    
    /// Check if this is a user process (PID >= 1000)
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.0.get() >= 1000
    }
    
    /// Check if this is a system process (PID < 1000, but not init)
    #[inline(always)]
    pub const fn is_system(self) -> bool {
        let raw = self.0.get();
        raw > 1 && raw < 1000
    }
    
    /// Check if PID is in user range (< 65536)
    #[inline(always)]
    pub const fn is_in_user_range(self) -> bool {
        self.0.get() <= Self::MAX_USER
    }
    
    /// Checked increment (returns None on overflow)
    #[inline(always)]
    pub const fn checked_add(self, n: u32) -> Option<Self> {
        match self.0.get().checked_add(n) {
            Some(new_pid) if new_pid <= Self::MAX => {
                Some(unsafe { Self::from_raw_unchecked(new_pid) })
            }
            _ => None,
        }
    }
    
    /// Checked decrement (returns None on underflow to 0)
    #[inline(always)]
    pub const fn checked_sub(self, n: u32) -> Option<Self> {
        match self.0.get().checked_sub(n) {
            Some(new_pid) if new_pid > 0 => {
                Some(unsafe { Self::from_raw_unchecked(new_pid) })
            }
            _ => None,
        }
    }
    
    /// Saturating increment (stops at MAX)
    #[inline(always)]
    pub const fn saturating_add(self, n: u32) -> Self {
        let new_pid = self.0.get().saturating_add(n);
        if new_pid > Self::MAX {
            unsafe { Self::from_raw_unchecked(Self::MAX) }
        } else {
            unsafe { Self::from_raw_unchecked(new_pid) }
        }
    }
    
    /// Saturating decrement (stops at 1)
    #[inline(always)]
    pub const fn saturating_sub(self, n: u32) -> Self {
        let new_pid = self.0.get().saturating_sub(n);
        if new_pid == 0 {
            Self::INIT
        } else {
            unsafe { Self::from_raw_unchecked(new_pid) }
        }
    }
=======
//! Process ID types

use core::fmt;

/// Process identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Pid(pub u32);

impl Pid {
    /// Create a new PID
    pub const fn new(pid: u32) -> Self {
        Self(pid)
    }

    /// Get the raw PID value
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Special PID for init process
    pub const INIT: Self = Self(1);

    /// Invalid PID
    pub const INVALID: Self = Self(0);
>>>>>>> Stashed changes
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

<<<<<<< Updated upstream
impl fmt::LowerHex for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0.get(), f)
    }
}

impl fmt::UpperHex for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::UpperHex::fmt(&self.0.get(), f)
=======
impl From<u32> for Pid {
    fn from(pid: u32) -> Self {
        Self(pid)
>>>>>>> Stashed changes
    }
}

impl From<Pid> for u32 {
<<<<<<< Updated upstream
    #[inline(always)]
    fn from(pid: Pid) -> u32 {
        pid.as_raw()
    }
}

impl From<Pid> for usize {
    #[inline(always)]
    fn from(pid: Pid) -> usize {
        pid.as_usize()
    }
}

impl From<Pid> for i32 {
    #[inline(always)]
    fn from(pid: Pid) -> i32 {
        pid.as_raw() as i32
    }
}

impl TryFrom<u32> for Pid {
    type Error = ();
    
    #[inline(always)]
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(())
    }
}

impl TryFrom<usize> for Pid {
    type Error = ();
    
    #[inline(always)]
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value > u32::MAX as usize {
            Err(())
        } else {
            Self::new(value as u32).ok_or(())
        }
    }
}

impl TryFrom<i32> for Pid {
    type Error = ();
    
    #[inline(always)]
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        if value < 0 {
            Err(())
        } else {
            Self::new(value as u32).ok_or(())
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;
    
    #[test]
    fn test_pid_creation() {
        assert!(Pid::new(0).is_none());
        assert!(Pid::new(1).is_some());
        assert!(Pid::new(42).is_some());
        assert!(Pid::new(Pid::MAX).is_some());
        assert!(Pid::new(Pid::MAX + 1).is_none());
        assert!(Pid::new(u32::MAX).is_none());
    }
    
    #[test]
    fn test_pid_constants() {
        assert_eq!(Pid::INIT.as_raw(), 1);
        assert_eq!(Pid::FIRST_USER.as_raw(), 1000);
        assert_eq!(Pid::MIN, 1);
        assert_eq!(Pid::MAX, u32::MAX - 1);
        assert_eq!(Pid::MAX_USER, 0xFFFF);
    }
    
    #[test]
    fn test_pid_is_init() {
        assert!(Pid::INIT.is_init());
        assert!(!Pid::new(2).unwrap().is_init());
        assert!(!Pid::new(1000).unwrap().is_init());
    }
    
    #[test]
    fn test_pid_is_user() {
        assert!(!Pid::INIT.is_user());
        assert!(!Pid::new(999).unwrap().is_user());
        assert!(Pid::new(1000).unwrap().is_user());
        assert!(Pid::new(2000).unwrap().is_user());
    }
    
    #[test]
    fn test_pid_is_system() {
        assert!(!Pid::INIT.is_system());
        assert!(Pid::new(2).unwrap().is_system());
        assert!(Pid::new(500).unwrap().is_system());
        assert!(Pid::new(999).unwrap().is_system());
        assert!(!Pid::new(1000).unwrap().is_system());
    }
    
    #[test]
    fn test_pid_is_in_user_range() {
        assert!(Pid::INIT.is_in_user_range());
        assert!(Pid::new(1000).unwrap().is_in_user_range());
        assert!(Pid::new(65535).unwrap().is_in_user_range());
        assert!(!Pid::new(65536).unwrap().is_in_user_range());
    }
    
    #[test]
    fn test_pid_checked_add() {
        let pid = Pid::new(100).unwrap();
        assert_eq!(pid.checked_add(50).unwrap().as_raw(), 150);
        assert_eq!(pid.checked_add(0).unwrap().as_raw(), 100);
        
        let max_pid = Pid::new(Pid::MAX).unwrap();
        assert!(max_pid.checked_add(1).is_none());
        assert!(max_pid.checked_add(100).is_none());
    }
    
    #[test]
    fn test_pid_checked_sub() {
        let pid = Pid::new(100).unwrap();
        assert_eq!(pid.checked_sub(50).unwrap().as_raw(), 50);
        assert_eq!(pid.checked_sub(99).unwrap().as_raw(), 1);
        assert!(pid.checked_sub(100).is_none());
        assert!(pid.checked_sub(101).is_none());
        
        assert!(Pid::INIT.checked_sub(1).is_none());
    }
    
    #[test]
    fn test_pid_saturating_add() {
        let pid = Pid::new(100).unwrap();
        assert_eq!(pid.saturating_add(50).as_raw(), 150);
        
        let max_pid = Pid::new(Pid::MAX).unwrap();
        assert_eq!(max_pid.saturating_add(1).as_raw(), Pid::MAX);
        assert_eq!(max_pid.saturating_add(1000).as_raw(), Pid::MAX);
    }
    
    #[test]
    fn test_pid_saturating_sub() {
        let pid = Pid::new(100).unwrap();
        assert_eq!(pid.saturating_sub(50).as_raw(), 50);
        assert_eq!(pid.saturating_sub(100).as_raw(), 1);
        assert_eq!(pid.saturating_sub(1000).as_raw(), 1);
        
        assert_eq!(Pid::INIT.saturating_sub(1).as_raw(), 1);
    }
    
    #[test]
    fn test_pid_conversions() {
        let pid = Pid::new(42).unwrap();
        
        assert_eq!(pid.as_raw(), 42);
        assert_eq!(pid.as_usize(), 42);
        
        let u32_val: u32 = pid.into();
        assert_eq!(u32_val, 42);
        
        let usize_val: usize = pid.into();
        assert_eq!(usize_val, 42);
        
        let i32_val: i32 = pid.into();
        assert_eq!(i32_val, 42);
    }
    
    #[test]
    fn test_pid_try_from() {
        assert_eq!(Pid::try_from(42u32).unwrap().as_raw(), 42);
        assert!(Pid::try_from(0u32).is_err());
        
        assert_eq!(Pid::try_from(42usize).unwrap().as_raw(), 42);
        assert!(Pid::try_from(0usize).is_err());
        
        assert_eq!(Pid::try_from(42i32).unwrap().as_raw(), 42);
        assert!(Pid::try_from(-1i32).is_err());
        assert!(Pid::try_from(0i32).is_err());
    }
    
    #[test]
    fn test_pid_display() {
        let pid = Pid::new(42).unwrap();
        let s = std::format!("{}", pid);
        assert_eq!(s, "42");
        
        let hex_lower = std::format!("{:x}", pid);
        assert_eq!(hex_lower, "2a");
        
        let hex_upper = std::format!("{:X}", pid);
        assert_eq!(hex_upper, "2A");
    }
    
    #[test]
    fn test_pid_ordering() {
        let pid1 = Pid::new(100).unwrap();
        let pid2 = Pid::new(200).unwrap();
        let pid3 = Pid::new(100).unwrap();
        
        assert!(pid1 < pid2);
        assert!(pid2 > pid1);
        assert_eq!(pid1, pid3);
        assert_ne!(pid1, pid2);
    }
    
    #[test]
    fn test_pid_size() {
        assert_eq!(size_of::<Pid>(), size_of::<u32>());
        assert_eq!(size_of::<Option<Pid>>(), size_of::<u32>());
    }
    
    #[test]
    fn test_pid_copy() {
        let pid1 = Pid::new(42).unwrap();
        let pid2 = pid1;
        assert_eq!(pid1.as_raw(), pid2.as_raw());
    }
    
    #[test]
    fn test_pid_hash() {
        use std::collections::HashSet;
        
        let mut set = HashSet::new();
        let pid1 = Pid::new(42).unwrap();
        let pid2 = Pid::new(42).unwrap();
        let pid3 = Pid::new(43).unwrap();
        
        set.insert(pid1);
        assert!(set.contains(&pid2));
        assert!(!set.contains(&pid3));
=======
    fn from(pid: Pid) -> Self {
        pid.0
>>>>>>> Stashed changes
    }
}
