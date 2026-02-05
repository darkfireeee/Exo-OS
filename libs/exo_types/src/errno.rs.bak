// libs/exo_types/src/errno.rs
//! POSIX-compatible errno codes

use core::fmt;

/// POSIX errno codes + Exo-OS extensions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum Errno {
    // POSIX standard errors (1-133)
    EPERM = 1,              // Operation not permitted
    ENOENT = 2,             // No such file or directory
    ESRCH = 3,              // No such process
    EINTR = 4,              // Interrupted system call
    EIO = 5,                // I/O error
    ENXIO = 6,              // No such device or address
    E2BIG = 7,              // Argument list too long
    ENOEXEC = 8,            // Exec format error
    EBADF = 9,              // Bad file number
    ECHILD = 10,            // No child processes
    EAGAIN = 11,            // Try again (also EWOULDBLOCK)
    ENOMEM = 12,            // Out of memory
    EACCES = 13,            // Permission denied
    EFAULT = 14,            // Bad address
    ENOTBLK = 15,           // Block device required
    EBUSY = 16,             // Device or resource busy
    EEXIST = 17,            // File exists
    EXDEV = 18,             // Cross-device link
    ENODEV = 19,            // No such device
    ENOTDIR = 20,           // Not a directory
    EISDIR = 21,            // Is a directory
    EINVAL = 22,            // Invalid argument
    ENFILE = 23,            // File table overflow
    EMFILE = 24,            // Too many open files
    ENOTTY = 25,            // Not a typewriter
    ETXTBSY = 26,           // Text file busy
    EFBIG = 27,             // File too large
    ENOSPC = 28,            // No space left on device
    ESPIPE = 29,            // Illegal seek
    EROFS = 30,             // Read-only file system
    EMLINK = 31,            // Too many links
    EPIPE = 32,             // Broken pipe
    EDOM = 33,              // Math argument out of domain
    ERANGE = 34,            // Math result not representable
    EDEADLK = 35,           // Resource deadlock would occur
    ENAMETOOLONG = 36,      // File name too long
    ENOLCK = 37,            // No record locks available
    ENOSYS = 38,            // Function not implemented
    ENOTEMPTY = 39,         // Directory not empty
    ELOOP = 40,             // Too many symbolic links
    ENOMSG = 42,            // No message of desired type
    EIDRM = 43,             // Identifier removed
    ECHRNG = 44,            // Channel number out of range
    EL2NSYNC = 45,          // Level 2 not synchronized
    EL3HLT = 46,            // Level 3 halted
    EL3RST = 47,            // Level 3 reset
    ELNRNG = 48,            // Link number out of range
    EUNATCH = 49,           // Protocol driver not attached
    ENOCSI = 50,            // No CSI structure available
    EL2HLT = 51,            // Level 2 halted
    EBADE = 52,             // Invalid exchange
    EBADR = 53,             // Invalid request descriptor
    EXFULL = 54,            // Exchange full
    ENOANO = 55,            // No anode
    EBADRQC = 56,           // Invalid request code
    EBADSLT = 57,           // Invalid slot
    EBFONT = 59,            // Bad font file format
    ENOSTR = 60,            // Device not a stream
    ENODATA = 61,           // No data available
    ETIME = 62,             // Timer expired
    ENOSR = 63,             // Out of streams resources
    ENONET = 64,            // Machine is not on the network
    ENOPKG = 65,            // Package not installed
    EREMOTE = 66,           // Object is remote
    ENOLINK = 67,           // Link has been severed
    EADV = 68,              // Advertise error
    ESRMNT = 69,            // Srmount error
    ECOMM = 70,             // Communication error on send
    EPROTO = 71,            // Protocol error
    EMULTIHOP = 72,         // Multihop attempted
    EDOTDOT = 73,           // RFS specific error
    EBADMSG = 74,           // Not a data message
    EOVERFLOW = 75,         // Value too large for defined data type
    ENOTUNIQ = 76,          // Name not unique on network
    EBADFD = 77,            // File descriptor in bad state
    EREMCHG = 78,           // Remote address changed
    ELIBACC = 79,           // Can not access a needed shared library
    ELIBBAD = 80,           // Accessing a corrupted shared library
    ELIBSCN = 81,           // .lib section in a.out corrupted
    ELIBMAX = 82,           // Attempting to link in too many shared libraries
    ELIBEXEC = 83,          // Cannot exec a shared library directly
    EILSEQ = 84,            // Illegal byte sequence
    ERESTART = 85,          // Interrupted system call should be restarted
    ESTRPIPE = 86,          // Streams pipe error
    EUSERS = 87,            // Too many users
    ENOTSOCK = 88,          // Socket operation on non-socket
    EDESTADDRREQ = 89,      // Destination address required
    EMSGSIZE = 90,          // Message too long
    EPROTOTYPE = 91,        // Protocol wrong type for socket
    ENOPROTOOPT = 92,       // Protocol not available
    EPROTONOSUPPORT = 93,   // Protocol not supported
    ESOCKTNOSUPPORT = 94,   // Socket type not supported
    EOPNOTSUPP = 95,        // Operation not supported on transport endpoint
    EPFNOSUPPORT = 96,      // Protocol family not supported
    EAFNOSUPPORT = 97,      // Address family not supported by protocol
    EADDRINUSE = 98,        // Address already in use
    EADDRNOTAVAIL = 99,     // Cannot assign requested address
    ENETDOWN = 100,         // Network is down
    ENETUNREACH = 101,      // Network is unreachable
    ENETRESET = 102,        // Network dropped connection because of reset
    ECONNABORTED = 103,     // Software caused connection abort
    ECONNRESET = 104,       // Connection reset by peer
    ENOBUFS = 105,          // No buffer space available
    EISCONN = 106,          // Transport endpoint is already connected
    ENOTCONN = 107,         // Transport endpoint is not connected
    ESHUTDOWN = 108,        // Cannot send after transport endpoint shutdown
    ETOOMANYREFS = 109,     // Too many references: cannot splice
    ETIMEDOUT = 110,        // Connection timed out
    ECONNREFUSED = 111,     // Connection refused
    EHOSTDOWN = 112,        // Host is down
    EHOSTUNREACH = 113,     // No route to host
    EALREADY = 114,         // Operation already in progress
    EINPROGRESS = 115,      // Operation now in progress
    ESTALE = 116,           // Stale file handle
    EUCLEAN = 117,          // Structure needs cleaning
    ENOTNAM = 118,          // Not a XENIX named type file
    ENAVAIL = 119,          // No XENIX semaphores available
    EISNAM = 120,           // Is a named type file
    EREMOTEIO = 121,        // Remote I/O error
    EDQUOT = 122,           // Quota exceeded
    ENOMEDIUM = 123,        // No medium found
    EMEDIUMTYPE = 124,      // Wrong medium type
    ECANCELED = 125,        // Operation Canceled
    ENOKEY = 126,           // Required key not available
    EKEYEXPIRED = 127,      // Key has expired
    EKEYREVOKED = 128,      // Key has been revoked
    EKEYREJECTED = 129,     // Key was rejected by service
    EOWNERDEAD = 130,       // Owner died
    ENOTRECOVERABLE = 131,  // State not recoverable
    ERFKILL = 132,          // Operation not possible due to RF-kill
    EHWPOISON = 133,        // Memory page has hardware error
    
    // Exo-OS custom errors (1000+)
    ECAPABILITY = 1000,     // Capability error
    ESECURITY = 1001,       // Security violation
    EVERSION = 1002,        // Version mismatch
    ECORRUPTED = 1003,      // Data corruption detected
    EQUOTA = 1004,          // Quota limit exceeded
    EMICROKERNEL = 1005,    // Microkernel IPC error
}

impl Errno {
    /// Get error description
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EPERM => "Operation not permitted",
            Self::ENOENT => "No such file or directory",
            Self::ESRCH => "No such process",
            Self::EINTR => "Interrupted system call",
            Self::EIO => "I/O error",
            Self::ENXIO => "No such device or address",
            Self::E2BIG => "Argument list too long",
            Self::ENOEXEC => "Exec format error",
            Self::EBADF => "Bad file descriptor",
            Self::ECHILD => "No child processes",
            Self::EAGAIN => "Resource temporarily unavailable",
            Self::ENOMEM => "Out of memory",
            Self::EACCES => "Permission denied",
            Self::EFAULT => "Bad address",
            Self::EBUSY => "Device or resource busy",
            Self::EEXIST => "File exists",
            Self::ENOTDIR => "Not a directory",
            Self::EISDIR => "Is a directory",
            Self::EINVAL => "Invalid argument",
            Self::EMFILE => "Too many open files",
            Self::ENOSPC => "No space left on device",
            Self::EPIPE => "Broken pipe",
            Self::ENOSYS => "Function not implemented",
            Self::ETIMEDOUT => "Connection timed out",
            Self::ECONNREFUSED => "Connection refused",
            Self::ECAPABILITY => "Capability error",
            Self::ESECURITY => "Security violation",
            _ => "Unknown error",
        }
    }
    
    /// Convert from raw errno value
    pub const fn from_raw(errno: i32) -> Option<Self> {
        match errno {
            1 => Some(Self::EPERM),
            2 => Some(Self::ENOENT),
            3 => Some(Self::ESRCH),
            4 => Some(Self::EINTR),
            5 => Some(Self::EIO),
            9 => Some(Self::EBADF),
            11 => Some(Self::EAGAIN),
            12 => Some(Self::ENOMEM),
            13 => Some(Self::EACCES),
            22 => Some(Self::EINVAL),
            32 => Some(Self::EPIPE),
            38 => Some(Self::ENOSYS),
            110 => Some(Self::ETIMEDOUT),
            111 => Some(Self::ECONNREFUSED),
            1000 => Some(Self::ECAPABILITY),
            1001 => Some(Self::ESECURITY),
            _ => None,
        }
    }
    
    /// Get raw errno value
    pub const fn as_raw(self) -> i32 {
        self as i32
    }
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (errno {})", self.as_str(), self.as_raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_errno_conversion() {
        assert_eq!(Errno::from_raw(1), Some(Errno::EPERM));
        assert_eq!(Errno::from_raw(12), Some(Errno::ENOMEM));
        assert_eq!(Errno::EPERM.as_raw(), 1);
    }
}
