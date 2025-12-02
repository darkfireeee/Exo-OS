//! Signal to Message Translation
//!
//! Converts POSIX signals to Exo-OS IPC messages

// Message types defined locally

/// Placeholder Message type
#[derive(Debug, Clone)]
pub struct Message {
    pub msg_type: MessageType,
    pub sender: u64,
    pub data: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum MessageType {
    Terminate,
    ProcessExit,
    Suspend,
    Resume,
    Exception,
    Signal,
    Other,
}

/// POSIX signal numbers
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Signal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGSTKFLT = 16,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
    SIGURG = 23,
    SIGXCPU = 24,
    SIGXFSZ = 25,
    SIGVTALRM = 26,
    SIGPROF = 27,
    SIGWINCH = 28,
    SIGIO = 29,
    SIGPWR = 30,
    SIGSYS = 31,
}

impl Signal {
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::SIGHUP),
            2 => Some(Self::SIGINT),
            3 => Some(Self::SIGQUIT),
            6 => Some(Self::SIGABRT),
            9 => Some(Self::SIGKILL),
            11 => Some(Self::SIGSEGV),
            13 => Some(Self::SIGPIPE),
            15 => Some(Self::SIGTERM),
            17 => Some(Self::SIGCHLD),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SIGHUP => "SIGHUP",
            Self::SIGINT => "SIGINT",
            Self::SIGQUIT => "SIGQUIT",
            Self::SIGKILL => "SIGKILL",
            Self::SIGTERM => "SIGTERM",
            Self::SIGSEGV => "SIGSEGV",
            Self::SIGCHLD => "SIGCHLD",
            _ => "UNKNOWN",
        }
    }

    pub fn is_catchable(self) -> bool {
        !matches!(self, Self::SIGKILL | Self::SIGSTOP)
    }
}

/// Convert POSIX signal to Exo-OS message
pub fn signal_to_message(signal: Signal, sender_pid: u64) -> Message {
    let msg_type = match signal {
        Signal::SIGTERM | Signal::SIGKILL => MessageType::Terminate,
        Signal::SIGCHLD => MessageType::ProcessExit,
        Signal::SIGSTOP | Signal::SIGTSTP => MessageType::Suspend,
        Signal::SIGCONT => MessageType::Resume,
        Signal::SIGSEGV | Signal::SIGBUS => MessageType::Exception,
        _ => MessageType::Signal,
    };

    Message {
        msg_type,
        sender: sender_pid,
        data: signal as u64,
    }
}

/// Convert Exo-OS message to POSIX signal
pub fn message_to_signal(msg: &Message) -> Option<Signal> {
    match msg.msg_type {
        MessageType::Terminate => Some(Signal::SIGTERM),
        MessageType::ProcessExit => Some(Signal::SIGCHLD),
        MessageType::Suspend => Some(Signal::SIGSTOP),
        MessageType::Resume => Some(Signal::SIGCONT),
        MessageType::Exception => Some(Signal::SIGSEGV),
        MessageType::Signal => Signal::from_i32(msg.data as i32),
        _ => None,
    }
}

/// Get default signal action
#[derive(Debug, Clone, Copy)]
pub enum SignalAction {
    Terminate,
    Ignore,
    Stop,
    Continue,
    CoreDump,
}

pub fn default_action(signal: Signal) -> SignalAction {
    match signal {
        Signal::SIGKILL | Signal::SIGTERM => SignalAction::Terminate,
        Signal::SIGCHLD | Signal::SIGURG | Signal::SIGWINCH => SignalAction::Ignore,
        Signal::SIGSTOP | Signal::SIGTSTP => SignalAction::Stop,
        Signal::SIGCONT => SignalAction::Continue,
        Signal::SIGSEGV | Signal::SIGABRT => SignalAction::CoreDump,
        _ => SignalAction::Terminate,
    }
}
