//! # POSIX Compatibility Types and Structures
//!
//! Types for POSIX API compatibility

use alloc::string::String;

/// POSIX file descriptor
pub type Fd = i32;

/// POSIX process ID
pub type Pid = i32;

/// POSIX user ID
pub type Uid = u32;

/// POSIX group ID
pub type Gid = u32;

/// POSIX mode
pub type Mode = u32;

/// POSIX offset
pub type Off = i64;

/// File open flags (POSIX)
pub mod open_flags {
    pub const O_RDONLY: i32 = 0;
    pub const O_WRONLY: i32 = 1;
    pub const O_RDWR: i32 = 2;
    pub const O_CREAT: i32 = 0o100;
    pub const O_EXCL: i32 = 0o200;
    pub const O_NOCTTY: i32 = 0o400;
    pub const O_TRUNC: i32 = 0o1000;
    pub const O_APPEND: i32 = 0o2000;
    pub const O_NONBLOCK: i32 = 0o4000;
    pub const O_SYNC: i32 = 0o10000;
    pub const O_DIRECTORY: i32 = 0o200000;
    pub const O_CLOEXEC: i32 = 0o2000000;
}

/// File seek whence (POSIX)
pub mod seek_whence {
    pub const SEEK_SET: i32 = 0;
    pub const SEEK_CUR: i32 = 1;
    pub const SEEK_END: i32 = 2;
}

/// POSIX stat structure
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: i32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
}

/// POSIX timespec structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// POSIX timeval structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// POSIX compatibility layer main struct
pub struct PosixCompat {
    /// Current working directory
    cwd: String,
    /// Process umask
    umask: Mode,
}

impl PosixCompat {
    /// Create new POSIX compat layer
    pub fn new() -> Self {
        Self {
            cwd: String::from("/"),
            umask: 0o022,
        }
    }

    /// Get current working directory
    pub fn getcwd(&self) -> &str {
        &self.cwd
    }

    /// Set current working directory
    pub fn chdir(&mut self, path: &str) {
        self.cwd = String::from(path);
    }

    /// Get umask
    pub fn getumask(&self) -> Mode {
        self.umask
    }

    /// Set umask
    pub fn setumask(&mut self, mask: Mode) -> Mode {
        let old = self.umask;
        self.umask = mask & 0o777;
        old
    }
}

impl Default for PosixCompat {
    fn default() -> Self {
        Self::new()
    }
}
