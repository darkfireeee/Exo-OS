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

            /// Check if this is a permission error
            #[inline(always)]
            pub const fn is_permission_error(self) -> bool {
                matches!(self, Self::EPERM | Self::EACCES | Self::ECAPABILITY | Self::ESECURITY)
            }

            /// Check if this is a not-found error
            #[inline(always)]
            pub const fn is_not_found(self) -> bool {
                matches!(self, Self::ENOENT | Self::ESRCH | Self::ENXIO | Self::ENODEV)
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
        /// Operation not permitted
        EPERM = 1, "Operation not permitted",
        /// No such file or directory
        ENOENT = 2, "No such file or directory",
        /// No such process
        ESRCH = 3, "No such process",
        /// Interrupted system call
        EINTR = 4, "Interrupted system call",
        /// I/O error
        EIO = 5, "I/O error",
        /// No such device or address
        ENXIO = 6, "No such device or address",
        /// Argument list too long
        E2BIG = 7, "Argument list too long",
        /// Exec format error
        ENOEXEC = 8, "Exec format error",
        /// Bad file descriptor
        EBADF = 9, "Bad file descriptor",
        /// No child processes
        ECHILD = 10, "No child processes",
        /// Resource temporarily unavailable
        EAGAIN = 11, "Resource temporarily unavailable",
        /// Out of memory
        ENOMEM = 12, "Out of memory",
        /// Permission denied
        EACCES = 13, "Permission denied",
        /// Bad address
        EFAULT = 14, "Bad address",
        /// Block device required
        ENOTBLK = 15, "Block device required",
        /// Device or resource busy
        EBUSY = 16, "Device or resource busy",
        /// File exists
        EEXIST = 17, "File exists",
        /// Invalid cross-device link
        EXDEV = 18, "Invalid cross-device link",
        /// No such device
        ENODEV = 19, "No such device",
        /// Not a directory
        ENOTDIR = 20, "Not a directory",
        /// Is a directory
        EISDIR = 21, "Is a directory",
        /// Invalid argument
        EINVAL = 22, "Invalid argument",
        /// Too many open files in system
        ENFILE = 23, "Too many open files in system",
        /// Too many open files
        EMFILE = 24, "Too many open files",
        /// Inappropriate ioctl for device
        ENOTTY = 25, "Inappropriate ioctl for device",
        /// Text file busy
        ETXTBSY = 26, "Text file busy",
        /// File too large
        EFBIG = 27, "File too large",
        /// No space left on device
        ENOSPC = 28, "No space left on device",
        /// Illegal seek
        ESPIPE = 29, "Illegal seek",
        /// Read-only file system
        EROFS = 30, "Read-only file system",
        /// Too many links
        EMLINK = 31, "Too many links",
        /// Broken pipe
        EPIPE = 32, "Broken pipe",
        /// Numerical argument out of domain
        EDOM = 33, "Numerical argument out of domain",
        /// Numerical result out of range
        ERANGE = 34, "Numerical result out of range",
        /// Resource deadlock avoided
        EDEADLK = 35, "Resource deadlock avoided",
        /// File name too long
        ENAMETOOLONG = 36, "File name too long",
        /// No locks available
        ENOLCK = 37, "No locks available",
        /// Function not implemented
        ENOSYS = 38, "Function not implemented",
        /// Directory not empty
        ENOTEMPTY = 39, "Directory not empty",
        /// Too many levels of symbolic links
        ELOOP = 40, "Too many levels of symbolic links",
        /// No message of desired type
        ENOMSG = 42, "No message of desired type",
        /// Identifier removed
        EIDRM = 43, "Identifier removed",
        /// Channel number out of range
        ECHRNG = 44, "Channel number out of range",
        /// Level 2 not synchronized
        EL2NSYNC = 45, "Level 2 not synchronized",
        /// Level 3 halted
        EL3HLT = 46, "Level 3 halted",
        /// Level 3 reset
        EL3RST = 47, "Level 3 reset",
        /// Link number out of range
        ELNRNG = 48, "Link number out of range",
        /// Protocol driver not attached
        EUNATCH = 49, "Protocol driver not attached",
        /// No CSI structure available
        ENOCSI = 50, "No CSI structure available",
        /// Level 2 halted
        EL2HLT = 51, "Level 2 halted",
        /// Invalid exchange
        EBADE = 52, "Invalid exchange",
        /// Invalid request descriptor
        EBADR = 53, "Invalid request descriptor",
        /// Exchange full
        EXFULL = 54, "Exchange full",
        /// No anode
        ENOANO = 55, "No anode",
        /// Invalid request code
        EBADRQC = 56, "Invalid request code",
        /// Invalid slot
        EBADSLT = 57, "Invalid slot",
        /// Bad font file format
        EBFONT = 59, "Bad font file format",
        /// Device not a stream
        ENOSTR = 60, "Device not a stream",
        /// No data available
        ENODATA = 61, "No data available",
        /// Timer expired
        ETIME = 62, "Timer expired",
        /// Out of streams resources
        ENOSR = 63, "Out of streams resources",
        /// Machine is not on the network
        ENONET = 64, "Machine is not on the network",
        /// Package not installed
        ENOPKG = 65, "Package not installed",
        /// Object is remote
        EREMOTE = 66, "Object is remote",
        /// Link has been severed
        ENOLINK = 67, "Link has been severed",
        /// Advertise error
        EADV = 68, "Advertise error",
        /// Srmount error
        ESRMNT = 69, "Srmount error",
        /// Communication error on send
        ECOMM = 70, "Communication error on send",
        /// Protocol error
        EPROTO = 71, "Protocol error",
        /// Multihop attempted
        EMULTIHOP = 72, "Multihop attempted",
        /// RFS specific error
        EDOTDOT = 73, "RFS specific error",
        /// Bad message
        EBADMSG = 74, "Bad message",
        /// Value too large for defined data type
        EOVERFLOW = 75, "Value too large for defined data type",
        /// Name not unique on network
        ENOTUNIQ = 76, "Name not unique on network",
        /// File descriptor in bad state
        EBADFD = 77, "File descriptor in bad state",
        /// Remote address changed
        EREMCHG = 78, "Remote address changed",
        /// Cannot access a needed shared library
        ELIBACC = 79, "Cannot access a needed shared library",
        /// Accessing a corrupted shared library
        ELIBBAD = 80, "Accessing a corrupted shared library",
        /// .lib section in a.out corrupted
        ELIBSCN = 81, ".lib section in a.out corrupted",
        /// Attempting to link in too many shared libraries
        ELIBMAX = 82, "Attempting to link in too many shared libraries",
        /// Cannot exec a shared library directly
        ELIBEXEC = 83, "Cannot exec a shared library directly",
        /// Invalid or incomplete multibyte or wide character
        EILSEQ = 84, "Invalid or incomplete multibyte or wide character",
        /// Interrupted system call should be restarted
        ERESTART = 85, "Interrupted system call should be restarted",
        /// Streams pipe error
        ESTRPIPE = 86, "Streams pipe error",
        /// Too many users
        EUSERS = 87, "Too many users",
        /// Socket operation on non-socket
        ENOTSOCK = 88, "Socket operation on non-socket",
        /// Destination address required
        EDESTADDRREQ = 89, "Destination address required",
        /// Message too long
        EMSGSIZE = 90, "Message too long",
        /// Protocol wrong type for socket
        EPROTOTYPE = 91, "Protocol wrong type for socket",
        /// Protocol not available
        ENOPROTOOPT = 92, "Protocol not available",
        /// Protocol not supported
        EPROTONOSUPPORT = 93, "Protocol not supported",
        /// Socket type not supported
        ESOCKTNOSUPPORT = 94, "Socket type not supported",
        /// Operation not supported
        EOPNOTSUPP = 95, "Operation not supported",
        /// Protocol family not supported
        EPFNOSUPPORT = 96, "Protocol family not supported",
        /// Address family not supported by protocol
        EAFNOSUPPORT = 97, "Address family not supported by protocol",
        /// Address already in use
        EADDRINUSE = 98, "Address already in use",
        /// Cannot assign requested address
        EADDRNOTAVAIL = 99, "Cannot assign requested address",
        /// Network is down
        ENETDOWN = 100, "Network is down",
        /// Network is unreachable
        ENETUNREACH = 101, "Network is unreachable",
        /// Network dropped connection on reset
        ENETRESET = 102, "Network dropped connection on reset",
        /// Software caused connection abort
        ECONNABORTED = 103, "Software caused connection abort",
        /// Connection reset by peer
        ECONNRESET = 104, "Connection reset by peer",
        /// No buffer space available
        ENOBUFS = 105, "No buffer space available",
        /// Transport endpoint is already connected
        EISCONN = 106, "Transport endpoint is already connected",
        /// Transport endpoint is not connected
        ENOTCONN = 107, "Transport endpoint is not connected",
        /// Cannot send after transport endpoint shutdown
        ESHUTDOWN = 108, "Cannot send after transport endpoint shutdown",
        /// Too many references: cannot splice
        ETOOMANYREFS = 109, "Too many references: cannot splice",
        /// Connection timed out
        ETIMEDOUT = 110, "Connection timed out",
        /// Connection refused
        ECONNREFUSED = 111, "Connection refused",
        /// Host is down
        EHOSTDOWN = 112, "Host is down",
        /// No route to host
        EHOSTUNREACH = 113, "No route to host",
        /// Operation already in progress
        EALREADY = 114, "Operation already in progress",
        /// Operation now in progress
        EINPROGRESS = 115, "Operation now in progress",
        /// Stale file handle
        ESTALE = 116, "Stale file handle",
        /// Structure needs cleaning
        EUCLEAN = 117, "Structure needs cleaning",
        /// Not a XENIX named type file
        ENOTNAM = 118, "Not a XENIX named type file",
        /// No XENIX semaphores available
        ENAVAIL = 119, "No XENIX semaphores available",
        /// Is a named type file
        EISNAM = 120, "Is a named type file",
        /// Remote I/O error
        EREMOTEIO = 121, "Remote I/O error",
        /// Disk quota exceeded
        EDQUOT = 122, "Disk quota exceeded",
        /// No medium found
        ENOMEDIUM = 123, "No medium found",
        /// Wrong medium type
        EMEDIUMTYPE = 124, "Wrong medium type",
        /// Operation canceled
        ECANCELED = 125, "Operation canceled",
        /// Required key not available
        ENOKEY = 126, "Required key not available",
        /// Key has expired
        EKEYEXPIRED = 127, "Key has expired",
        /// Key has been revoked
        EKEYREVOKED = 128, "Key has been revoked",
        /// Key was rejected by service
        EKEYREJECTED = 129, "Key was rejected by service",
        /// Owner died
        EOWNERDEAD = 130, "Owner died",
        /// State not recoverable
        ENOTRECOVERABLE = 131, "State not recoverable",
        /// Operation not possible due to RF-kill
        ERFKILL = 132, "Operation not possible due to RF-kill",
        /// Memory page has hardware error
        EHWPOISON = 133, "Memory page has hardware error",

        // ===== Exo-OS Custom Errors (1000+) =====
        /// Capability violation
        ECAPABILITY = 1000, "Capability violation",
        /// Security policy violation
        ESECURITY = 1001, "Security policy violation",
        /// Version mismatch
        EVERSION = 1002, "Version mismatch",
        /// Data corruption detected
        ECORRUPTED = 1003, "Data corruption detected",
        /// Resource quota exceeded
        EQUOTA = 1004, "Resource quota exceeded",
        /// Microkernel IPC error
        EMICROKERNEL = 1005, "Microkernel IPC error",
        /// Invalid capability token
        EINVALIDCAP = 1006, "Invalid capability token",
        /// Capability expired
        ECAPEXPIRED = 1007, "Capability expired",
        /// Process isolation violation
        EISOLATION = 1008, "Process isolation violation",
        /// Sandbox policy violation
        ESANDBOX = 1009, "Sandbox policy violation",
    }
}

impl Errno {
    /// EWOULDBLOCK is an alias for EAGAIN on Linux
    pub const EWOULDBLOCK: Self = Self::EAGAIN;
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_errno_raw_conversion() {
        assert_eq!(Errno::EPERM.as_raw(), 1);
        assert_eq!(Errno::ENOENT.as_raw(), 2);
        assert_eq!(Errno::ECAPABILITY.as_raw(), 1000);
    }

    #[test]
    fn test_errno_from_raw() {
        assert_eq!(Errno::from_raw(1), Some(Errno::EPERM));
        assert_eq!(Errno::from_raw(2), Some(Errno::ENOENT));
        assert_eq!(Errno::from_raw(1000), Some(Errno::ECAPABILITY));
        assert_eq!(Errno::from_raw(9999), None);
    }

    #[test]
    fn test_errno_as_str() {
        assert_eq!(Errno::EPERM.as_str(), "Operation not permitted");
        assert_eq!(Errno::ENOENT.as_str(), "No such file or directory");
        assert_eq!(Errno::ECAPABILITY.as_str(), "Capability violation");
    }

    #[test]
    fn test_errno_is_retriable() {
        assert!(Errno::EINTR.is_retriable());
        assert!(Errno::EAGAIN.is_retriable());
        assert!(Errno::EWOULDBLOCK.is_retriable());
        assert!(!Errno::EPERM.is_retriable());
        assert!(!Errno::ENOMEM.is_retriable());
    }

    #[test]
    fn test_errno_is_fatal() {
        assert!(Errno::EFAULT.is_fatal());
        assert!(Errno::ENOMEM.is_fatal());
        assert!(Errno::ENOSYS.is_fatal());
        assert!(Errno::ECORRUPTED.is_fatal());
        assert!(Errno::EMICROKERNEL.is_fatal());
        assert!(!Errno::EAGAIN.is_fatal());
    }

    #[test]
    fn test_errno_is_permission_error() {
        assert!(Errno::EPERM.is_permission_error());
        assert!(Errno::EACCES.is_permission_error());
        assert!(Errno::ECAPABILITY.is_permission_error());
        assert!(Errno::ESECURITY.is_permission_error());
        assert!(!Errno::ENOENT.is_permission_error());
    }

    #[test]
    fn test_errno_is_not_found() {
        assert!(Errno::ENOENT.is_not_found());
        assert!(Errno::ESRCH.is_not_found());
        assert!(Errno::ENXIO.is_not_found());
        assert!(Errno::ENODEV.is_not_found());
        assert!(!Errno::EPERM.is_not_found());
    }

    #[test]
    fn test_errno_display() {
        let s = std::format!("{}", Errno::EPERM);
        assert!(s.contains("Operation not permitted"));
        assert!(s.contains("errno 1"));
    }

    #[test]
    fn test_errno_i32_conversion() {
        let errno = Errno::EPERM;
        let val: i32 = errno.into();
        assert_eq!(val, 1);
    }

    #[test]
    fn test_errno_ewouldblock_alias() {
        assert_eq!(Errno::EWOULDBLOCK.as_raw(), Errno::EAGAIN.as_raw());
    }
}
