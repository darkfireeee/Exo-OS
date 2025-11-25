//! C-compatible types for FFI
//! 
//! Defines standard C types for safe interoperability

#![allow(non_camel_case_types)]

// Standard C integer types
pub type c_char = i8;
pub type c_schar = i8;
pub type c_uchar = u8;
pub type c_short = i16;
pub type c_ushort = u16;
pub type c_int = i32;
pub type c_uint = u32;
pub type c_long = i64;
pub type c_ulong = u64;
pub type c_longlong = i64;
pub type c_ulonglong = u64;

// Floating point types
pub type c_float = f32;
pub type c_double = f64;

// Size types (x86_64)
pub type c_size_t = u64;
pub type c_ssize_t = i64;
pub type c_ptrdiff_t = i64;

// Void pointer
pub type c_void = core::ffi::c_void;

// Boolean type
pub type c_bool = bool;

// Standard sizes
pub const CHAR_BIT: usize = 8;
pub const SCHAR_MIN: i8 = i8::MIN;
pub const SCHAR_MAX: i8 = i8::MAX;
pub const UCHAR_MAX: u8 = u8::MAX;
pub const SHRT_MIN: i16 = i16::MIN;
pub const SHRT_MAX: i16 = i16::MAX;
pub const USHRT_MAX: u16 = u16::MAX;
pub const INT_MIN: i32 = i32::MIN;
pub const INT_MAX: i32 = i32::MAX;
pub const UINT_MAX: u32 = u32::MAX;
pub const LONG_MIN: i64 = i64::MIN;
pub const LONG_MAX: i64 = i64::MAX;
pub const ULONG_MAX: u64 = u64::MAX;

// NULL pointer
pub const NULL: *const c_void = core::ptr::null();
pub const NULL_MUT: *mut c_void = core::ptr::null_mut();

/// Errno values (POSIX compatible)
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    SUCCESS = 0,
    EPERM = 1,      // Operation not permitted
    ENOENT = 2,     // No such file or directory
    ESRCH = 3,      // No such process
    EINTR = 4,      // Interrupted system call
    EIO = 5,        // I/O error
    ENXIO = 6,      // No such device or address
    E2BIG = 7,      // Argument list too long
    ENOEXEC = 8,    // Exec format error
    EBADF = 9,      // Bad file number
    ECHILD = 10,    // No child processes
    EAGAIN = 11,    // Try again
    ENOMEM = 12,    // Out of memory
    EACCES = 13,    // Permission denied
    EFAULT = 14,    // Bad address
    ENOTBLK = 15,   // Block device required
    EBUSY = 16,     // Device or resource busy
    EEXIST = 17,    // File exists
    EXDEV = 18,     // Cross-device link
    ENODEV = 19,    // No such device
    ENOTDIR = 20,   // Not a directory
    EISDIR = 21,    // Is a directory
    EINVAL = 22,    // Invalid argument
    ENFILE = 23,    // File table overflow
    EMFILE = 24,    // Too many open files
    ENOTTY = 25,    // Not a typewriter
    ETXTBSY = 26,   // Text file busy
    EFBIG = 27,     // File too large
    ENOSPC = 28,    // No space left on device
    ESPIPE = 29,    // Illegal seek
    EROFS = 30,     // Read-only file system
    EMLINK = 31,    // Too many links
    EPIPE = 32,     // Broken pipe
}

impl Errno {
    pub fn as_c_int(self) -> c_int {
        self as c_int
    }
    
    pub fn from_c_int(errno: c_int) -> Self {
        match errno {
            0 => Errno::SUCCESS,
            1 => Errno::EPERM,
            2 => Errno::ENOENT,
            3 => Errno::ESRCH,
            4 => Errno::EINTR,
            5 => Errno::EIO,
            6 => Errno::ENXIO,
            7 => Errno::E2BIG,
            8 => Errno::ENOEXEC,
            9 => Errno::EBADF,
            10 => Errno::ECHILD,
            11 => Errno::EAGAIN,
            12 => Errno::ENOMEM,
            13 => Errno::EACCES,
            14 => Errno::EFAULT,
            _ => Errno::EINVAL,
        }
    }
}
