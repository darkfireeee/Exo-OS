//! POSIX signal types and signal sets
//!
//! Complete implementation of POSIX signals with type-safe signal sets.

use core::fmt;

/// POSIX Signal numbers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Signal {
    /// Hangup detected on controlling terminal or death of controlling process
    SIGHUP = 1,
    /// Interrupt from keyboard
    SIGINT = 2,
    /// Quit from keyboard
    SIGQUIT = 3,
    /// Illegal Instruction
    SIGILL = 4,
    /// Trace/breakpoint trap
    SIGTRAP = 5,
    /// Abort signal from abort()
    SIGABRT = 6,
    /// Bus error (bad memory access)
    SIGBUS = 7,
    /// Floating-point exception
    SIGFPE = 8,
    /// Kill signal
    SIGKILL = 9,
    /// User-defined signal 1
    SIGUSR1 = 10,
    /// Invalid memory reference
    SIGSEGV = 11,
    /// User-defined signal 2
    SIGUSR2 = 12,
    /// Broken pipe: write to pipe with no readers
    SIGPIPE = 13,
    /// Alarm clock
    SIGALRM = 14,
    /// Termination signal
    SIGTERM = 15,
    /// Stack fault on coprocessor (unused)
    SIGSTKFLT = 16,
    /// Child stopped or terminated
    SIGCHLD = 17,
    /// Continue if stopped
    SIGCONT = 18,
    /// Stop process
    SIGSTOP = 19,
    /// Stop typed at terminal
    SIGTSTP = 20,
    /// Terminal input for background process
    SIGTTIN = 21,
    /// Terminal output for background process
    SIGTTOU = 22,
    /// Urgent condition on socket
    SIGURG = 23,
    /// CPU time limit exceeded
    SIGXCPU = 24,
    /// File size limit exceeded
    SIGXFSZ = 25,
    /// Virtual alarm clock
    SIGVTALRM = 26,
    /// Profiling timer expired
    SIGPROF = 27,
    /// Window resize signal
    SIGWINCH = 28,
    /// I/O now possible
    SIGIO = 29,
    /// Power failure
    SIGPWR = 30,
    /// Bad system call
    SIGSYS = 31,
}

impl Signal {
    /// Minimum valid signal number
    pub const MIN: i32 = 1;

    /// Maximum valid signal number
    pub const MAX: i32 = 31;

    /// Convert signal to raw i32 value
    #[inline(always)]
    pub const fn as_raw(self) -> i32 {
        self as i32
    }

    /// Convert raw i32 to signal (returns None if invalid)
    #[inline]
    pub const fn from_raw(signum: i32) -> Option<Self> {
        match signum {
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
            16 => Some(Self::SIGSTKFLT),
            17 => Some(Self::SIGCHLD),
            18 => Some(Self::SIGCONT),
            19 => Some(Self::SIGSTOP),
            20 => Some(Self::SIGTSTP),
            21 => Some(Self::SIGTTIN),
            22 => Some(Self::SIGTTOU),
            23 => Some(Self::SIGURG),
            24 => Some(Self::SIGXCPU),
            25 => Some(Self::SIGXFSZ),
            26 => Some(Self::SIGVTALRM),
            27 => Some(Self::SIGPROF),
            28 => Some(Self::SIGWINCH),
            29 => Some(Self::SIGIO),
            30 => Some(Self::SIGPWR),
            31 => Some(Self::SIGSYS),
            _ => None,
        }
    }

    /// Get signal name as string
    #[inline]
    pub const fn as_str(self) -> &'static str {
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
            Self::SIGSTKFLT => "SIGSTKFLT",
            Self::SIGCHLD => "SIGCHLD",
            Self::SIGCONT => "SIGCONT",
            Self::SIGSTOP => "SIGSTOP",
            Self::SIGTSTP => "SIGTSTP",
            Self::SIGTTIN => "SIGTTIN",
            Self::SIGTTOU => "SIGTTOU",
            Self::SIGURG => "SIGURG",
            Self::SIGXCPU => "SIGXCPU",
            Self::SIGXFSZ => "SIGXFSZ",
            Self::SIGVTALRM => "SIGVTALRM",
            Self::SIGPROF => "SIGPROF",
            Self::SIGWINCH => "SIGWINCH",
            Self::SIGIO => "SIGIO",
            Self::SIGPWR => "SIGPWR",
            Self::SIGSYS => "SIGSYS",
        }
    }

    /// Check if signal can be caught/blocked (SIGKILL and SIGSTOP cannot)
    #[inline(always)]
    pub const fn is_catchable(self) -> bool {
        !matches!(self, Self::SIGKILL | Self::SIGSTOP)
    }

    /// Check if signal is for fatal errors (can't continue)
    #[inline(always)]
    pub const fn is_fatal(self) -> bool {
        matches!(self, Self::SIGILL | Self::SIGSEGV | Self::SIGBUS | Self::SIGFPE | Self::SIGSYS)
    }

    /// Check if signal is for termination
    #[inline(always)]
    pub const fn is_termination(self) -> bool {
        matches!(self, Self::SIGTERM | Self::SIGKILL | Self::SIGQUIT | Self::SIGABRT)
    }

    /// Check if signal is for job control
    #[inline(always)]
    pub const fn is_job_control(self) -> bool {
        matches!(self, Self::SIGSTOP | Self::SIGTSTP | Self::SIGTTIN | Self::SIGTTOU | Self::SIGCONT)
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.as_str(), self.as_raw())
    }
}

impl From<Signal> for i32 {
    #[inline(always)]
    fn from(sig: Signal) -> i32 {
        sig.as_raw()
    }
}

/// Signal set for tracking multiple signals
///
/// Uses a bitset for efficient signal storage and operations.
/// Each bit represents one signal (1-31).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct SignalSet(u32);

impl SignalSet {
    /// Empty signal set
    pub const EMPTY: Self = Self(0);

    /// Full signal set (all signals 1-31)
    pub const FULL: Self = Self(0xFFFF_FFFE); // bits 1-31

    /// Create new empty signal set
    #[inline(always)]
    pub const fn new() -> Self {
        Self::EMPTY
    }

    /// Create signal set from raw bitmask
    #[inline(always)]
    pub const fn from_raw(mask: u32) -> Self {
        Self(mask)
    }

    /// Get raw bitmask
    #[inline(always)]
    pub const fn as_raw(self) -> u32 {
        self.0
    }

    /// Add signal to set
    #[inline(always)]
    pub const fn add(self, signal: Signal) -> Self {
        Self(self.0 | (1 << signal.as_raw()))
    }

    /// Remove signal from set
    #[inline(always)]
    pub const fn remove(self, signal: Signal) -> Self {
        Self(self.0 & !(1 << signal.as_raw()))
    }

    /// Check if signal is in set
    #[inline(always)]
    pub const fn contains(self, signal: Signal) -> bool {
        (self.0 & (1 << signal.as_raw())) != 0
    }

    /// Check if set is empty
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Check if set is full
    #[inline(always)]
    pub const fn is_full(self) -> bool {
        self.0 == Self::FULL.0
    }

    /// Get number of signals in set
    #[inline(always)]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// Union of two sets
    #[inline(always)]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Intersection of two sets
    #[inline(always)]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Difference of two sets (signals in self but not in other)
    #[inline(always)]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Complement of set
    #[inline(always)]
    pub const fn complement(self) -> Self {
        Self((!self.0) & Self::FULL.0)
    }

    /// Clear all signals
    #[inline(always)]
    pub const fn clear(self) -> Self {
        Self::EMPTY
    }

    /// Fill with all signals
    #[inline(always)]
    pub const fn fill(self) -> Self {
        Self::FULL
    }

    /// Iterator over signals in the set
    pub fn iter(self) -> SignalSetIter {
        SignalSetIter {
            set: self,
            current: Signal::MIN,
        }
    }
}

impl Default for SignalSet {
    #[inline]
    fn default() -> Self {
        Self::EMPTY
    }
}

impl fmt::Debug for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalSet")
            .field("count", &self.count())
            .field("mask", &format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Display for SignalSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignalSet(")?;
        let mut first = true;
        for sig in self.iter() {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{}", sig.as_str())?;
            first = false;
        }
        write!(f, ")")
    }
}

/// Iterator over signals in a signal set
pub struct SignalSetIter {
    set: SignalSet,
    current: i32,
}

impl Iterator for SignalSetIter {
    type Item = Signal;

    fn next(&mut self) -> Option<Self::Item> {
        while self.current <= Signal::MAX {
            if let Some(sig) = Signal::from_raw(self.current) {
                self.current += 1;
                if self.set.contains(sig) {
                    return Some(sig);
                }
            } else {
                self.current += 1;
            }
        }
        None
    }
}

/// Signal handler type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SignalHandler {
    /// Default signal handler
    Default = 0,
    /// Ignore signal
    Ignore = 1,
    /// Custom handler (function pointer would go here in full implementation)
    Custom = 2,
}

/// Signal action (handler + flags)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalAction {
    /// Handler type
    handler: SignalHandler,
    /// Signals to block during handler execution
    mask: SignalSet,
    /// Action flags (SA_RESTART, etc.)
    flags: u32,
}

impl SignalAction {
    /// Create new signal action
    #[inline(always)]
    pub const fn new(handler: SignalHandler, mask: SignalSet, flags: u32) -> Self {
        Self { handler, mask, flags }
    }

    /// Default action (system default handler)
    pub const DEFAULT: Self = Self {
        handler: SignalHandler::Default,
        mask: SignalSet::EMPTY,
        flags: 0,
    };

    /// Ignore action
    pub const IGNORE: Self = Self {
        handler: SignalHandler::Ignore,
        mask: SignalSet::EMPTY,
        flags: 0,
    };

    /// Get handler
    #[inline(always)]
    pub const fn handler(self) -> SignalHandler {
        self.handler
    }

    /// Get signal mask
    #[inline(always)]
    pub const fn mask(self) -> SignalSet {
        self.mask
    }

    /// Get flags
    #[inline(always)]
    pub const fn flags(self) -> u32 {
        self.flags
    }

    /// Set handler
    #[inline(always)]
    pub const fn set_handler(mut self, handler: SignalHandler) -> Self {
        self.handler = handler;
        self
    }

    /// Set mask
    #[inline(always)]
    pub const fn set_mask(mut self, mask: SignalSet) -> Self {
        self.mask = mask;
        self
    }

    /// Set flags
    #[inline(always)]
    pub const fn set_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_signal_conversions() {
        assert_eq!(Signal::SIGINT.as_raw(), 2);
        assert_eq!(Signal::from_raw(2), Some(Signal::SIGINT));
        assert_eq!(Signal::from_raw(99), None);
    }

    #[test]
    fn test_signal_as_str() {
        assert_eq!(Signal::SIGINT.as_str(), "SIGINT");
        assert_eq!(Signal::SIGTERM.as_str(), "SIGTERM");
        assert_eq!(Signal::SIGKILL.as_str(), "SIGKILL");
    }

    #[test]
    fn test_signal_is_catchable() {
        assert!(Signal::SIGINT.is_catchable());
        assert!(Signal::SIGTERM.is_catchable());
        assert!(!Signal::SIGKILL.is_catchable());
        assert!(!Signal::SIGSTOP.is_catchable());
    }

    #[test]
    fn test_signal_is_fatal() {
        assert!(Signal::SIGSEGV.is_fatal());
        assert!(Signal::SIGBUS.is_fatal());
        assert!(Signal::SIGILL.is_fatal());
        assert!(!Signal::SIGTERM.is_fatal());
        assert!(!Signal::SIGINT.is_fatal());
    }

    #[test]
    fn test_signal_is_termination() {
        assert!(Signal::SIGTERM.is_termination());
        assert!(Signal::SIGKILL.is_termination());
        assert!(Signal::SIGQUIT.is_termination());
        assert!(!Signal::SIGINT.is_termination());
    }

    #[test]
    fn test_signal_is_job_control() {
        assert!(Signal::SIGSTOP.is_job_control());
        assert!(Signal::SIGCONT.is_job_control());
        assert!(Signal::SIGTSTP.is_job_control());
        assert!(!Signal::SIGTERM.is_job_control());
    }

    #[test]
    fn test_signalset_empty() {
        let set = SignalSet::new();
        assert!(set.is_empty());
        assert!(!set.is_full());
        assert_eq!(set.count(), 0);
    }

    #[test]
    fn test_signalset_add_remove() {
        let set = SignalSet::new();
        let set = set.add(Signal::SIGINT);
        assert!(set.contains(Signal::SIGINT));
        assert!(!set.contains(Signal::SIGTERM));

        let set = set.add(Signal::SIGTERM);
        assert!(set.contains(Signal::SIGINT));
        assert!(set.contains(Signal::SIGTERM));
        assert_eq!(set.count(), 2);

        let set = set.remove(Signal::SIGINT);
        assert!(!set.contains(Signal::SIGINT));
        assert!(set.contains(Signal::SIGTERM));
        assert_eq!(set.count(), 1);
    }

    #[test]
    fn test_signalset_operations() {
        let set1 = SignalSet::new().add(Signal::SIGINT).add(Signal::SIGTERM);
        let set2 = SignalSet::new().add(Signal::SIGTERM).add(Signal::SIGKILL);

        let union = set1.union(set2);
        assert!(union.contains(Signal::SIGINT));
        assert!(union.contains(Signal::SIGTERM));
        assert!(union.contains(Signal::SIGKILL));
        assert_eq!(union.count(), 3);

        let intersection = set1.intersection(set2);
        assert!(!intersection.contains(Signal::SIGINT));
        assert!(intersection.contains(Signal::SIGTERM));
        assert!(!intersection.contains(Signal::SIGKILL));
        assert_eq!(intersection.count(), 1);

        let diff = set1.difference(set2);
        assert!(diff.contains(Signal::SIGINT));
        assert!(!diff.contains(Signal::SIGTERM));
        assert!(!diff.contains(Signal::SIGKILL));
        assert_eq!(diff.count(), 1);
    }

    #[test]
    fn test_signalset_full() {
        let set = SignalSet::FULL;
        assert!(!set.is_empty());
        assert!(set.is_full());
        assert_eq!(set.count(), 31);

        assert!(set.contains(Signal::SIGINT));
        assert!(set.contains(Signal::SIGTERM));
        assert!(set.contains(Signal::SIGKILL));
    }

    #[test]
    fn test_signalset_complement() {
        let set = SignalSet::new().add(Signal::SIGINT);
        let comp = set.complement();

        assert!(!comp.contains(Signal::SIGINT));
        assert!(comp.contains(Signal::SIGTERM));
        assert_eq!(comp.count(), 30);
    }

    #[test]
    fn test_signalset_iter() {
        let set = SignalSet::new()
            .add(Signal::SIGINT)
            .add(Signal::SIGTERM)
            .add(Signal::SIGKILL);

        let signals: std::vec::Vec<_> = set.iter().collect();
        assert_eq!(signals.len(), 3);
        assert!(signals.contains(&Signal::SIGINT));
        assert!(signals.contains(&Signal::SIGTERM));
        assert!(signals.contains(&Signal::SIGKILL));
    }

    #[test]
    fn test_signal_action() {
        let action = SignalAction::new(
            SignalHandler::Custom,
            SignalSet::new().add(Signal::SIGINT),
            0x01,
        );

        assert_eq!(action.handler(), SignalHandler::Custom);
        assert!(action.mask().contains(Signal::SIGINT));
        assert_eq!(action.flags(), 0x01);

        assert_eq!(SignalAction::DEFAULT.handler(), SignalHandler::Default);
        assert_eq!(SignalAction::IGNORE.handler(), SignalHandler::Ignore);
    }
}
