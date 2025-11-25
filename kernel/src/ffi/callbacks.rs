//! Callback function types for C interop
//! 
//! Defines common callback signatures used in kernel APIs

use super::types::*;

/// Generic callback function
pub type Callback = extern "C" fn();

/// Callback with single pointer argument
pub type CallbackPtr = extern "C" fn(*mut c_void);

/// Callback with two arguments
pub type Callback2 = extern "C" fn(*mut c_void, *mut c_void);

/// Interrupt handler callback
pub type InterruptHandler = extern "C" fn(interrupt_number: c_uint);

/// Timer callback
pub type TimerCallback = extern "C" fn(timer_id: c_uint, data: *mut c_void);

/// I/O callback
pub type IoCallback = extern "C" fn(
    fd: c_int,
    buffer: *mut c_void,
    size: c_size_t,
    result: c_ssize_t,
);

/// Error callback
pub type ErrorCallback = extern "C" fn(errno: c_int, message: *const c_char);

/// Cleanup callback (called before object destruction)
pub type CleanupCallback = extern "C" fn(object: *mut c_void);

/// Comparison callback (for sorting, searching)
pub type CompareFn = extern "C" fn(a: *const c_void, b: *const c_void) -> c_int;

/// Hash callback
pub type HashFn = extern "C" fn(key: *const c_void) -> c_size_t;

/// Free callback (for memory management)
pub type FreeFn = extern "C" fn(ptr: *mut c_void);

/// Thread entry point
pub type ThreadEntry = extern "C" fn(arg: *mut c_void) -> *mut c_void;

/// Signal handler
pub type SignalHandler = extern "C" fn(signal: c_int);

/// System call handler
pub type SyscallHandler = extern "C" fn(
    syscall_number: c_uint,
    arg1: c_ulong,
    arg2: c_ulong,
    arg3: c_ulong,
    arg4: c_ulong,
    arg5: c_ulong,
    arg6: c_ulong,
) -> c_long;
