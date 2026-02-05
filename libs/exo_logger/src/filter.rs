//! Log level filtering

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
    Fatal = 5,
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
    
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "TRACE" | "trace" => Some(Self::Trace),
            "DEBUG" | "debug" => Some(Self::Debug),
            "INFO" | "info" => Some(Self::Info),
            "WARN" | "warn" => Some(Self::Warn),
            "ERROR" | "error" => Some(Self::Error),
            "FATAL" | "fatal" => Some(Self::Fatal),
            _ => None,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub struct LogFilter {
    min_level: LogLevel,
}

impl LogFilter {
    pub const fn new(min_level: LogLevel) -> Self {
        Self { min_level }
    }
    
    pub fn should_log(&self, level: LogLevel) -> bool {
        level >= self.min_level
    }
    
    pub fn set_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }
}

impl Default for LogFilter {
    fn default() -> Self {
        Self::new(LogLevel::Info)
    }
}
