<<<<<<< Updated upstream
//! POSIX-compatible errno codes with Exo-OS extensions
//!
//! Complete implementation of standard POSIX error codes plus custom
//! Exo-OS specific errors. Optimized for performance with macro-generated
//! conversion functions and const operations.

use core::fmt;

/// Define errno codes with automatic implementations
macro_rules! define_errno {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $value:expr, $desc:expr
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
        #[repr(i32)]
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant = $value,
            )*
        }

        impl $name {
            /// Convert errno to raw i32 value
            #[inline(always)]
            pub const fn as_raw(self) -> i32 {
                self as i32
            }

            /// Convert raw i32 to errno (returns None if invalid)
            #[inline]
            pub const fn from_raw(errno: i32) -> Option<Self> {
                match errno {
                    $(
                        $value => Some(Self::$variant),
                    )*
                    _ => None,
                }
            }

            /// Get human-readable error description
            #[inline]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(
                        Self::$variant => $desc,
                    )*
                }
            }

            /// Check if this is a retriable error (EINTR, EAGAIN, EWOULDBLOCK)
            #[inline(always)]
            pub const fn is_retriable(self) -> bool {
                matches!(self, Self::EINTR | Self::EAGAIN)
            }

            /// Check if this is a fatal error (cannot recover)
            #[inline(always)]
            pub const fn is_fatal(self) -> bool {
                matches!(self, 
                    Self::EFAULT | Self::ENOMEM | Self::ENOSYS | 
                    Self::ECORRUPTED | Self::EMICROKERNEL
                )
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} (errno {})", self.as_str(), self.as_raw())
            }
        }

        impl From<$name> for i32 {
            #[inline(always)]
            fn from(errno: $name) -> i32 {
                errno.as_raw()
            }
        }
    };
}

define_errno! {
    /// POSIX errno codes + Exo-OS custom errors
    pub enum Errno {
        // ===== Standard POSIX Errors (1-133) =====
        EPERM = 1, "Operation not permitted",
        ENOENT = 2, "No such file or directory",
        ESRCH = 3, "No such process",
        EINTR = 4, "Interrupted system call",
        EIO = 5, "I/O error",
        ENXIO = 6, "No such device or address",
        E2BIG = 7, "Argument list too long",
        ENOEXEC = 8, "Exec format error",
        EBADF = 9, "Bad file descriptor",
        ECHILD = 10, "No child processes",
        EAGAIN = 11, "Resource temporarily unavailable",
        ENOMEM = 12, "Out of memory",
        EACCES = 13, "Permission denied",
        EFAULT = 14, "Bad address",
        ENOTBLK = 15, "Block device required",
        EBUSY = 16, "Device or resource busy",
        EEXIST = 17, "File exists",
        EXDEV = 18, "Invalid cross-device link",
        ENODEV = 19, "No such device",
        ENOTDIR = 20, "Not a directory",
        EISDIR = 21, "Is a directory",
        EINVAL = 22, "Invalid argument",
        ENFILE = 23, "Too many open files in system",
        EMFILE = 24, "Too many open files",
        ENOTTY = 25, "Inappropriate ioctl for device",
        ETXTBSY = 26, "Text file busy",
        EFBIG = 27, "File too large",
        ENOSPC = 28, "No space left on device",
        ESPIPE = 29, "Illegal seek",
        EROFS = 30, "Read-only file system",
        EMLINK = 31, "Too many links",
        EPIPE = 32, "Broken pipe",
        EDOM = 33, "Numerical argument out of domain",
        ERANGE = 34, "Numerical result out of range",
        EDEADLK = 35, "Resource deadlock avoided",
        ENAMETOOLONG = 36, "File name too long",
        ENOLCK = 37, "No locks available",
        ENOSYS = 38, "Function not implemented",
        ENOTEMPTY = 39, "Directory not empty",
        ELOOP = 40, "Too many levels of symbolic links",
        ENOMSG = 42, "No message of desired type",
        EIDRM = 43, "Identifier removed",
        ECHRNG = 44, "Channel number out of range",
        EL2NSYNC = 45, "Level 2 not synchronized",
        EL3HLT = 46, "Level 3 halted",
        EL3RST = 47, "Level 3 reset",
        ELNRNG = 48, "Link number out of range",
        EUNATCH = 49, "Protocol driver not attached",
        ENOCSI = 50, "No CSI structure available",
        EL2HLT = 51, "Level 2 halted",
        EBADE = 52, "Invalid exchange",
        EBADR = 53, "Invalid request descriptor",
        EXFULL = 54, "Exchange full",
        ENOANO = 55, "No anode",
        EBADRQC = 56, "Invalid request code",
        EBADSLT = 57, "Invalid slot",
        EBFONT = 59, "Bad font file format",
        ENOSTR = 60, "Device not a stream",
        ENODATA = 61, "No data available",
        ETIME = 62, "Timer expired",
        ENOSR = 63, "Out of streams resources",
        ENONET = 64, "Machine is not on the network",
        ENOPKG = 65, "Package not installed",
        EREMOTE = 66, "Object is remote",
        ENOLINK = 67, "Link has been severed",
        EADV = 68, "Advertise error",
        ESRMNT = 69, "Srmount error",
        ECOMM = 70, "Communication error on send",
        EPROTO = 71, "Protocol error",
        EMULTIHOP = 72, "Multihop attempted",
        EDOTDOT = 73, "RFS specific error",
        EBADMSG = 74, "Bad message",
        EOVERFLOW = 75, "Value too large for defined data type",
        ENOTUNIQ = 76, "Name not unique on network",
        EBADFD = 77, "File descriptor in bad state",
        EREMCHG = 78, "Remote address changed",
        ELIBACC = 79, "Cannot access a needed shared library",
        ELIBBAD = 80, "Accessing a corrupted shared library",
        ELIBSCN = 81, ".lib section in a.out corrupted",
        ELIBMAX = 82, "Attempting to link in too many shared libraries",
        ELIBEXEC = 83, "Cannot exec a shared library directly",
        EILSEQ = 84, "Invalid or incomplete multibyte or wide character",
        ERESTART = 85, "Interrupted system call should be restarted",
        ESTRPIPE = 86, "Streams pipe error",
        EUSERS = 87, "Too many users",
        ENOTSOCK = 88, "Socket operation on non-socket",
        EDESTADDRREQ = 89, "Destination address required",
        EMSGSIZE = 90, "Message too long",
        EPROTOTYPE = 91, "Protocol wrong type for socket",
        ENOPROTOOPT = 92, "Protocol not available",
        EPROTONOSUPPORT = 93, "Protocol not supported",
        ESOCKTNOSUPPORT = 94, "Socket type not supported",
        EOPNOTSUPP = 95, "Operation not supported",
        EPFNOSUPPORT = 96, "Protocol family not supported",
        EAFNOSUPPORT = 97, "Address family not supported by protocol",
        EADDRINUSE = 98, "Address already in use",
        EADDRNOTAVAIL = 99, "Cannot assign requested address",
        ENETDOWN = 100, "Network is down",
        ENETUNREACH = 101, "Network is unreachable",
        ENETRESET = 102, "Network dropped connection on reset",
        ECONNABORTED = 103, "Software caused connection abort",
        ECONNRESET = 104, "Connection reset by peer",
        ENOBUFS = 105, "No buffer space available",
        EISCONN = 106, "Transport endpoint is already connected",
        ENOTCONN = 107, "Transport endpoint is not connected",
        ESHUTDOWN = 108, "Cannot send after transport endpoint shutdown",
        ETOOMANYREFS = 109, "Too many references: cannot splice",
        ETIMEDOUT = 110, "Connection timed out",
        ECONNREFUSED = 111, "Connection refused",
        EHOSTDOWN = 112, "Host is down",
        EHOSTUNREACH = 113, "No route to host",
        EALREADY = 114, "Operation already in progress",
        EINPROGRESS = 115, "Operation now in progress",
        ESTALE = 116, "Stale file handle",
        EUCLEAN = 117, "Structure needs cleaning",
        ENOTNAM = 118, "Not a XENIX named type file",
        ENAVAIL = 119, "No XENIX semaphores available",
        EISNAM = 120, "Is a named type file",
        EREMOTEIO = 121, "Remote I/O error",
        EDQUOT = 122, "Disk quota exceeded",
        ENOMEDIUM = 123, "No medium found",
        EMEDIUMTYPE = 124, "Wrong medium type",
        ECANCELED = 125, "Operation canceled",
        ENOKEY = 126, "Required key not available",
        EKEYEXPIRED = 127, "Key has expired",
        EKEYREVOKED = 128, "Key has been revoked",
        EKEYREJECTED = 129, "Key was rejected by service",
        EOWNERDEAD = 130, "Owner died",
        ENOTRECOVERABLE = 131, "State not recoverable",
        ERFKILL = 132, "Operation not possible due to RF-kill",
        EHWPOISON = 133, "Memory page has hardware error",

        // ===== Exo-OS Custom Errors (1000+) =====
        ECAPABILITY = 1000, "Capability violation",
        ESECURITY = 1001, "Security policy violation",
        EVERSION = 1002, "Version mismatch",
        ECORRUPTED = 1003, "Data corruption detected",
        EQUOTA = 1004, "Resource quota exceeded",
        EMICROKERNEL = 1005, "Microkernel IPC error",
=======
//! Error numbers (errno values)

use core::fmt;

/// Error number (POSIX-like errno)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Errno(pub i32);

impl Errno {
    pub const SUCCESS: Self = Self(0);
    pub const EPERM: Self = Self(1);         // Operation not permitted
    pub const ENOENT: Self = Self(2);        // No such file or directory
    pub const ESRCH: Self = Self(3);         // No such process
    pub const EINTR: Self = Self(4);         // Interrupted system call
    pub const EIO: Self = Self(5);           // I/O error
    pub const ENXIO: Self = Self(6);         // No such device or address
    pub const E2BIG: Self = Self(7);         // Argument list too long
    pub const ENOEXEC: Self = Self(8);       // Exec format error
    pub const EBADF: Self = Self(9);         // Bad file number
    pub const ECHILD: Self = Self(10);       // No child processes
    pub const EAGAIN: Self = Self(11);       // Try again
    pub const ENOMEM: Self = Self(12);       // Out of memory
    pub const EACCES: Self = Self(13);       // Permission denied
    pub const EFAULT: Self = Self(14);       // Bad address
    pub const ENOTBLK: Self = Self(15);      // Block device required
    pub const EBUSY: Self = Self(16);        // Device or resource busy
    pub const EEXIST: Self = Self(17);       // File exists
    pub const EXDEV: Self = Self(18);        // Cross-device link
    pub const ENODEV: Self = Self(19);       // No such device
    pub const ENOTDIR: Self = Self(20);      // Not a directory
    pub const EISDIR: Self = Self(21);       // Is a directory
    pub const EINVAL: Self = Self(22);       // Invalid argument
    pub const ENFILE: Self = Self(23);       // File table overflow
    pub const EMFILE: Self = Self(24);       // Too many open files
    pub const ENOTTY: Self = Self(25);       // Not a typewriter
    pub const ETXTBSY: Self = Self(26);      // Text file busy
    pub const EFBIG: Self = Self(27);        // File too large
    pub const ENOSPC: Self = Self(28);       // No space left on device
    pub const ESPIPE: Self = Self(29);       // Illegal seek
    pub const EROFS: Self = Self(30);        // Read-only file system
    pub const EMLINK: Self = Self(31);       // Too many links
    pub const EPIPE: Self = Self(32);        // Broken pipe
    pub const EDOM: Self = Self(33);         // Math argument out of domain
    pub const ERANGE: Self = Self(34);       // Math result not representable
    pub const EDEADLK: Self = Self(35);      // Resource deadlock would occur
    pub const ENAMETOOLONG: Self = Self(36); // File name too long
    pub const ENOLCK: Self = Self(37);       // No record locks available
    pub const ENOSYS: Self = Self(38);       // Function not implemented
    pub const ENOTEMPTY: Self = Self(39);    // Directory not empty

    /// Create from raw errno value
    pub const fn new(errno: i32) -> Self {
        Self(errno)
    }

    /// Get raw errno value
    pub const fn as_i32(self) -> i32 {
        self.0
    }

    /// Check if this is a success (0)
    pub const fn is_success(self) -> bool {
        self.0 == 0
    }

    /// Get error description
    pub fn description(&self) -> &'static str {
        match *self {
            Self::SUCCESS => "Success",
            Self::EPERM => "Operation not permitted",
            Self::ENOENT => "No such file or directory",
            Self::ESRCH => "No such process",
            Self::EINTR => "Interrupted system call",
            Self::EIO => "I/O error",
            Self::ENXIO => "No such device or address",
            Self::E2BIG => "Argument list too long",
            Self::ENOEXEC => "Exec format error",
            Self::EBADF => "Bad file number",
            Self::ECHILD => "No child processes",
            Self::EAGAIN => "Try again",
            Self::ENOMEM => "Out of memory",
            Self::EACCES => "Permission denied",
            Self::EFAULT => "Bad address",
            Self::ENOTBLK => "Block device required",
            Self::EBUSY => "Device or resource busy",
            Self::EEXIST => "File exists",
            Self::EXDEV => "Cross-device link",
            Self::ENODEV => "No such device",
            Self::ENOTDIR => "Not a directory",
            Self::EISDIR => "Is a directory",
            Self::EINVAL => "Invalid argument",
            Self::ENFILE => "File table overflow",
            Self::EMFILE => "Too many open files",
            Self::ENOTTY => "Not a typewriter",
            Self::ETXTBSY => "Text file busy",
            Self::EFBIG => "File too large",
            Self::ENOSPC => "No space left on device",
            Self::ESPIPE => "Illegal seek",
            Self::EROFS => "Read-only file system",
            Self::EMLINK => "Too many links",
            Self::EPIPE => "Broken pipe",
            Self::EDOM => "Math argument out of domain",
            Self::ERANGE => "Math result not representable",
            Self::EDEADLK => "Resource deadlock would occur",
            Self::ENAMETOOLONG => "File name too long",
            Self::ENOLCK => "No record locks available",
            Self::ENOSYS => "Function not implemented",
            Self::ENOTEMPTY => "Directory not empty",
            _ => "Unknown error",
        }
    }
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (errno {})", self.description(), self.0)
    }
}

impl From<i32> for Errno {
    fn from(errno: i32) -> Self {
        Self(errno)
    }
}

impl From<Errno> for i32 {
    fn from(errno: Errno) -> Self {
        errno.0
>>>>>>> Stashed changes
    }
}
