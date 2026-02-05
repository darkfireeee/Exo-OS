//! POSIX signals for process communication and control
//!
//! Provides type-safe signal handling with support for signal sets and masks.

use core::fmt;

/// POSIX signal types
///
/// Standard Unix signals for process control, termination, and communication.
/// Signal numbers follow standard POSIX conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Signal {
    /// Hangup detected on controlling terminal
    SIGHUP = 1,
    /// Interrupt from keyboard (Ctrl+C)
    SIGINT = 2,
    /// Quit from keyboard (Ctrl+\)
    SIGQUIT = 3,
    /// Illegal instruction
    SIGILL = 4,
    /// Trace/breakpoint trap
    SIGTRAP = 5,
    /// Abort signal from abort()
    SIGABRT = 6,
    /// Bus error (bad memory access)
    SIGBUS = 7,
    /// Floating point exception
    SIGFPE = 8,
    /// Kill signal (uncatchable)
    SIGKILL = 9,
    /// User-defined signal 1
    SIGUSR1 = 10,
    /// Invalid memory reference (segmentation fault)
    SIGSEGV = 11,
    /// User-defined signal 2
    SIGUSR2 = 12,
    /// Broken pipe (write to pipe with no readers)
    SIGPIPE = 13,
    /// Timer signal from alarm()
    SIGALRM = 14,
    /// Termination signal
    SIGTERM = 15,
    /// Child stopped or terminated
    SIGCHLD = 17,
    /// Continue if stopped
    SIGCONT = 18,
    /// Stop process (uncatchable)
    SIGSTOP = 19,
    /// Stop typed at terminal (Ctrl+Z)
    SIGTSTP = 20,
    /// Terminal input for background process
    SIGTTIN = 21,
    /// Terminal output for background process
    SIGTTOU = 22,
}

impl Signal {
    /// Check if signal is uncatchable (cannot be caught or ignored)
    ///
    /// SIGKILL and SIGSTOP cannot be caught, blocked, or ignored.
    #[inline(always)]
    pub const fn is_uncatchable(self) -> bool {
        matches!(self, Self::SIGKILL | Self::SIGSTOP)
    }
    
    /// Check if signal is a stop signal
    #[inline(always)]
    pub const fn is_stop_signal(self) -> bool {
        matches!(self, Self::SIGSTOP | Self::SIGTSTP | Self::SIGTTIN | Self::SIGTTOU)
    }
    
    /// Check if signal continues execution
    #[inline(always)]
    pub const fn is_continue_signal(self) -> bool {
        matches!(self, Self::SIGCONT)
    }
    
    /// Check if signal terminates process by default
    #[inline(always)]
    pub const fn is_terminating(self) -> bool {
        matches!(
            self,
            Self::SIGHUP | Self::SIGINT | Self::SIGQUIT | Self::SIGILL |
            Self::SIGABRT | Self::SIGBUS | Self::SIGFPE | Self::SIGKILL |
            Self::SIGSEGV | Self::SIGPIPE | Self::SIGALRM | Self::SIGTERM
        )
    }
    
    /// Check if signal generates core dump by default
    #[inline(always)]
    pub const fn generates_core_dump(self) -> bool {
        matches!(
            self,
            Self::SIGQUIT | Self::SIGILL | Self::SIGABRT | Self::SIGBUS |
            Self::SIGFPE | Self::SIGSEGV | Self::SIGTRAP
        )
    }
    
    /// Check if signal is user-defined
    #[inline(always)]
    pub const fn is_user_defined(self) -> bool {
        matches!(self, Self::SIGUSR1 | Self::SIGUSR2)
    }
    
    /// Check if signal is child-related
    #[inline(always)]
    pub const fn is_child_signal(self) -> bool {
        matches!(self, Self::SIGCHLD)
    }

    /// Convert signal to its numeric value
    #[inline(always)]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
    
    /// Convert signal to i32 (common syscall interface)
    #[inline(always)]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Convert numeric value to Signal
    #[inline(always)]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::SIGHUP),
            2 => Some(Self::SIGINT),
            3 => Some(Self::SIGQUIT),
            4 => Some(Self::SIGILL),
            5 => Some(Self::SIGTRAP),
            6 => Some(Self::SIGABRT),
            7 => Some(Self::SIGBUS),
            8 => Some(Self::SIGFPE),
            9 => Some(Self::SIGKILL),
            10 => Some(Self::SIGUSR1),
            11 => Some(Self::SIGSEGV),
            12 => Some(Self::SIGUSR2),
            13 => Some(Self::SIGPIPE),
            14 => Some(Self::SIGALRM),
            15 => Some(Self::SIGTERM),
            17 => Some(Self::SIGCHLD),
            18 => Some(Self::SIGCONT),
            19 => Some(Self::SIGSTOP),
            20 => Some(Self::SIGTSTP),
            21 => Some(Self::SIGTTIN),
            22 => Some(Self::SIGTTOU),
            _ => None,
        }
    }
    
    /// Convert i32 to Signal
    #[inline(always)]
    pub const fn from_i32(value: i32) -> Option<Self> {
        if value < 0 || value > 255 {
            None
        } else {
            Self::from_u8(value as u8)
        }
    }
    
    /// Get signal name as string
    #[inline(always)]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SIGHUP => "SIGHUP",
            Self::SIGINT => "SIGINT",
            Self::SIGQUIT => "SIGQUIT",
            Self::SIGILL => "SIGILL",
            Self::SIGTRAP => "SIGTRAP",
            Self::SIGABRT => "SIGABRT",
            Self::SIGBUS => "SIGBUS",
            Self::SIGFPE => "SIGFPE",
            Self::SIGKILL => "SIGKILL",
            Self::SIGUSR1 => "SIGUSR1",
            Self::SIGSEGV => "SIGSEGV",
            Self::SIGUSR2 => "SIGUSR2",
            Self::SIGPIPE => "SIGPIPE",
            Self::SIGALRM => "SIGALRM",
            Self::SIGTERM => "SIGTERM",
            Self::SIGCHLD => "SIGCHLD",
            Self::SIGCONT => "SIGCONT",
            Self::SIGSTOP => "SIGSTOP",
            Self::SIGTSTP => "SIGTSTP",
            Self::SIGTTIN => "SIGTTIN",
            Self::SIGTTOU => "SIGTTOU",
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Signal set (bitmask of signals)
///
/// Efficient representation of multiple signals using a 32-bit bitmask.
/// Supports up to 32 signals (sufficient for standard POSIX signals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SignalSet(u32);

impl SignalSet {
    /// Empty signal set
    pub const EMPTY: Self = Self(0);
    
    /// Full signal set (all signals)
    pub const FULL: Self = Self(0x007F_FFFF); // Bits 0-22 (signals 1-22)
    
    /// Create empty signal set
    #[inline(always)]
    pub const fn empty() -> Self {
        Self::EMPTY
    }
    
    /// Create full signal set
    #[inline(always)]
    pub const fn full() -> Self {
        Self::FULL
    }
    
    /// Create signal set from single signal
    #[inline(always)]
    pub const fn from_signal(signal: Signal) -> Self {
        let bit = signal.as_u8() as u32;
        Self(1u32 << (bit - 1))
    }
    
    /// Create signal set from raw bitmask
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
    
    /// Get raw bitmask
    #[inline(always)]
    pub const fn as_bits(self) -> u32 {
        self.0
    }
    
    /// Add signal to set
    #[inline(always)]
    pub const fn add(self, signal: Signal) -> Self {
        let bit = signal.as_u8() as u32;
        Self(self.0 | (1u32 << (bit - 1)))
    }
    
    /// Remove signal from set
    #[inline(always)]
    pub const fn remove(self, signal: Signal) -> Self {
        let bit = signal.as_u8() as u32;
        Self(self.0 & !(1u32 << (bit - 1)))
    }
    
    /// Check if signal is in set
    #[inline(always)]
    pub const fn contains(self, signal: Signal) -> bool {
        let bit = signal.as_u8() as u32;
        (self.0 & (1u32 << (bit - 1))) != 0
    }
    
    /// Check if set is empty
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    
    /// Check if set contains all signals
    #[inline(always)]
    pub const fn is_full(self) -> bool {
        self.0 == Self::FULL.0
    }
    
    /// Union of two signal sets
    #[inline(always)]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    
    /// Intersection of two signal sets
    #[inline(always)]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    
    /// Difference of two signal sets
    #[inline(always)]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
    
    /// Complement of signal set
    #[inline(always)]
    pub const fn complement(self) -> Self {
        Self(!self.0 & Self::FULL.0)
    }
    
    /// Count signals in set
    #[inline(always)]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }
}

impl Default for SignalSet {
    #[inline(always)]
    fn default() -> Self {
        Self::EMPTY
    }
}

impl fmt::Display for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignalSet({:#010x})", self.0)
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;
    
    #[test]
    fn test_signal_conversions() {
        assert_eq!(Signal::SIGINT.as_u8(), 2);
        assert_eq!(Signal::SIGKILL.as_u8(), 9);
        
        assert_eq!(Signal::from_u8(2), Some(Signal::SIGINT));
        assert_eq!(Signal::from_u8(9), Some(Signal::SIGKILL));
        assert_eq!(Signal::from_u8(99), None);
        
        assert_eq!(Signal::from_i32(2), Some(Signal::SIGINT));
        assert_eq!(Signal::from_i32(-1), None);
    }
    
    #[test]
    fn test_signal_is_uncatchable() {
        assert!(Signal::SIGKILL.is_uncatchable());
        assert!(Signal::SIGSTOP.is_uncatchable());
        assert!(!Signal::SIGTERM.is_uncatchable());
        assert!(!Signal::SIGINT.is_uncatchable());
    }
    
    #[test]
    fn test_signal_is_stop() {
        assert!(Signal::SIGSTOP.is_stop_signal());
        assert!(Signal::SIGTSTP.is_stop_signal());
        assert!(Signal::SIGTTIN.is_stop_signal());
        assert!(Signal::SIGTTOU.is_stop_signal());
        assert!(!Signal::SIGTERM.is_stop_signal());
    }
    
    #[test]
    fn test_signal_is_continue() {
        assert!(Signal::SIGCONT.is_continue_signal());
        assert!(!Signal::SIGSTOP.is_continue_signal());
    }
    
    #[test]
    fn test_signal_is_terminating() {
        assert!(Signal::SIGINT.is_terminating());
        assert!(Signal::SIGTERM.is_terminating());
        assert!(Signal::SIGKILL.is_terminating());
        assert!(!Signal::SIGCHLD.is_terminating());
    }
    
    #[test]
    fn test_signal_generates_core_dump() {
        assert!(Signal::SIGQUIT.generates_core_dump());
        assert!(Signal::SIGSEGV.generates_core_dump());
        assert!(Signal::SIGABRT.generates_core_dump());
        assert!(!Signal::SIGTERM.generates_core_dump());
        assert!(!Signal::SIGINT.generates_core_dump());
    }
    
    #[test]
    fn test_signal_is_user_defined() {
        assert!(Signal::SIGUSR1.is_user_defined());
        assert!(Signal::SIGUSR2.is_user_defined());
        assert!(!Signal::SIGTERM.is_user_defined());
    }
    
    #[test]
    fn test_signal_is_child() {
        assert!(Signal::SIGCHLD.is_child_signal());
        assert!(!Signal::SIGTERM.is_child_signal());
    }
    
    #[test]
    fn test_signal_name() {
        assert_eq!(Signal::SIGINT.name(), "SIGINT");
        assert_eq!(Signal::SIGKILL.name(), "SIGKILL");
    }
    
    #[test]
    fn test_signal_display() {
        let s = std::format!("{}", Signal::SIGINT);
        assert_eq!(s, "SIGINT");
    }
    
    #[test]
    fn test_signalset_empty() {
        let set = SignalSet::empty();
        assert!(set.is_empty());
        assert!(!set.contains(Signal::SIGINT));
    }
    
    #[test]
    fn test_signalset_full() {
        let set = SignalSet::full();
        assert!(set.is_full());
        assert!(set.contains(Signal::SIGINT));
        assert!(set.contains(Signal::SIGTERM));
    }
    
    #[test]
    fn test_signalset_from_signal() {
        let set = SignalSet::from_signal(Signal::SIGINT);
        assert!(set.contains(Signal::SIGINT));
        assert!(!set.contains(Signal::SIGTERM));
    }
    
    #[test]
    fn test_signalset_add() {
        let set = SignalSet::empty()
            .add(Signal::SIGINT)
            .add(Signal::SIGTERM);
        
        assert!(set.contains(Signal::SIGINT));
        assert!(set.contains(Signal::SIGTERM));
        assert!(!set.contains(Signal::SIGKILL));
    }
    
    #[test]
    fn test_signalset_remove() {
        let set = SignalSet::full()
            .remove(Signal::SIGINT)
            .remove(Signal::SIGTERM);
        
        assert!(!set.contains(Signal::SIGINT));
        assert!(!set.contains(Signal::SIGTERM));
        assert!(set.contains(Signal::SIGKILL));
    }
    
    #[test]
    fn test_signalset_union() {
        let set1 = SignalSet::from_signal(Signal::SIGINT);
        let set2 = SignalSet::from_signal(Signal::SIGTERM);
        let union = set1.union(set2);
        
        assert!(union.contains(Signal::SIGINT));
        assert!(union.contains(Signal::SIGTERM));
    }
    
    #[test]
    fn test_signalset_intersection() {
        let set1 = SignalSet::empty()
            .add(Signal::SIGINT)
            .add(Signal::SIGTERM);
        let set2 = SignalSet::empty()
            .add(Signal::SIGTERM)
            .add(Signal::SIGKILL);
        let inter = set1.intersection(set2);
        
        assert!(!inter.contains(Signal::SIGINT));
        assert!(inter.contains(Signal::SIGTERM));
        assert!(!inter.contains(Signal::SIGKILL));
    }
    
    #[test]
    fn test_signalset_difference() {
        let set1 = SignalSet::empty()
            .add(Signal::SIGINT)
            .add(Signal::SIGTERM);
        let set2 = SignalSet::from_signal(Signal::SIGTERM);
        let diff = set1.difference(set2);
        
        assert!(diff.contains(Signal::SIGINT));
        assert!(!diff.contains(Signal::SIGTERM));
    }
    
    #[test]
    fn test_signalset_complement() {
        let set = SignalSet::from_signal(Signal::SIGINT);
        let comp = set.complement();
        
        assert!(!comp.contains(Signal::SIGINT));
        assert!(comp.contains(Signal::SIGTERM));
    }
    
    #[test]
    fn test_signalset_count() {
        let set = SignalSet::empty()
            .add(Signal::SIGINT)
            .add(Signal::SIGTERM)
            .add(Signal::SIGKILL);
        
        assert_eq!(set.count(), 3);
        assert_eq!(SignalSet::empty().count(), 0);
    }
    
    #[test]
    fn test_signalset_display() {
        let set = SignalSet::from_signal(Signal::SIGINT);
        let s = std::format!("{}", set);
        assert!(s.starts_with("SignalSet("));
    }
    
    #[test]
    fn test_signal_size() {
        assert_eq!(size_of::<Signal>(), 1);
    }
    
    #[test]
    fn test_signalset_size() {
        assert_eq!(size_of::<SignalSet>(), 4);
    }
    
    #[test]
    fn test_signal_copy() {
        let s1 = Signal::SIGINT;
        let s2 = s1;
        assert_eq!(s1, s2);
    }
    
    #[test]
    fn test_signalset_copy() {
        let set1 = SignalSet::from_signal(Signal::SIGINT);
        let set2 = set1;
        assert_eq!(set1.as_bits(), set2.as_bits());
    }
}
