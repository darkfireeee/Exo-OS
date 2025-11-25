//! Syscall Dispatcher
//! 
//! Fast syscall implementation using SYSCALL/SYSRET instructions
//! Target: <60 cycles for fast path

use core::arch::asm;

/// Maximum number of syscalls
pub const MAX_SYSCALLS: usize = 512;

/// Syscall handler function type
pub type SyscallHandler = fn(args: &[u64; 6]) -> Result<u64, SyscallError>;

/// Syscall error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SyscallError {
    InvalidSyscall = -1,
    InvalidArgument = -2,
    PermissionDenied = -3,
    NotFound = -4,
    AlreadyExists = -5,
    OutOfMemory = -6,
    IoError = -7,
    Interrupted = -8,
    WouldBlock = -9,
    NotSupported = -10,
}

impl SyscallError {
    pub fn to_errno(self) -> i64 {
        self as i64
    }
}

/// Syscall dispatch table
static mut SYSCALL_TABLE: [Option<SyscallHandler>; MAX_SYSCALLS] = [None; MAX_SYSCALLS];

/// Register a syscall handler
pub fn register_syscall(num: usize, handler: SyscallHandler) -> Result<(), SyscallError> {
    if num >= MAX_SYSCALLS {
        return Err(SyscallError::InvalidArgument);
    }

    unsafe {
        SYSCALL_TABLE[num] = Some(handler);
    }

    Ok(())
}

/// Unregister a syscall handler
pub fn unregister_syscall(num: usize) -> Result<(), SyscallError> {
    if num >= MAX_SYSCALLS {
        return Err(SyscallError::InvalidArgument);
    }

    unsafe {
        SYSCALL_TABLE[num] = None;
    }

    Ok(())
}

/// Dispatch a syscall
#[inline(never)]
pub fn dispatch_syscall(num: u64, args: &[u64; 6]) -> i64 {
    let num = num as usize;

    if num >= MAX_SYSCALLS {
        return SyscallError::InvalidSyscall.to_errno();
    }

    unsafe {
        if let Some(handler) = SYSCALL_TABLE[num] {
            match handler(args) {
                Ok(result) => result as i64,
                Err(err) => err.to_errno(),
            }
        } else {
            SyscallError::InvalidSyscall.to_errno()
        }
    }
}

/// Initialize syscall MSRs (Model Specific Registers)
pub unsafe fn init_syscall() {
    // IA32_STAR: Set kernel/user segments
    // Bits 63:48 = kernel CS/SS (0x08, 0x10)
    // Bits 47:32 = user CS/SS (0x18, 0x20)
    let star: u64 = ((0x08u64 << 32) | (0x18u64 << 48));
    write_msr(0xC0000081, star);

    // IA32_LSTAR: Set syscall entry point
    let syscall_entry = syscall_entry_asm as u64;
    write_msr(0xC0000082, syscall_entry);

    // IA32_FMASK: Clear these flags on syscall
    // Clear IF (interrupts), DF (direction), TF (trap)
    let fmask: u64 = 0x700;
    write_msr(0xC0000084, fmask);

    // Enable SYSCALL/SYSRET in IA32_EFER
    let efer = read_msr(0xC0000080);
    write_msr(0xC0000080, efer | 1); // Set SCE (System Call Extensions)
}

/// Write to MSR
#[inline]
unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags)
    );
}

/// Read from MSR
#[inline]
unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Syscall entry point (stub - real implementation in assembly file)
/// This is a placeholder that will be replaced with proper assembly
unsafe extern "C" fn syscall_entry_asm() -> ! {
    // This is just a stub - actual syscall entry should be in .S file
    // For now, just halt
    loop {
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// Common syscall numbers (Linux compatibility)
pub mod syscall_numbers {
    pub const SYS_READ: usize = 0;
    pub const SYS_WRITE: usize = 1;
    pub const SYS_OPEN: usize = 2;
    pub const SYS_CLOSE: usize = 3;
    pub const SYS_STAT: usize = 4;
    pub const SYS_FSTAT: usize = 5;
    pub const SYS_LSTAT: usize = 6;
    pub const SYS_POLL: usize = 7;
    pub const SYS_LSEEK: usize = 8;
    pub const SYS_MMAP: usize = 9;
    pub const SYS_MPROTECT: usize = 10;
    pub const SYS_MUNMAP: usize = 11;
    pub const SYS_BRK: usize = 12;
    pub const SYS_RT_SIGACTION: usize = 13;
    pub const SYS_RT_SIGPROCMASK: usize = 14;
    pub const SYS_IOCTL: usize = 16;
    pub const SYS_PIPE: usize = 22;
    pub const SYS_SELECT: usize = 23;
    pub const SYS_SCHED_YIELD: usize = 24;
    pub const SYS_MREMAP: usize = 25;
    pub const SYS_DUP: usize = 32;
    pub const SYS_DUP2: usize = 33;
    pub const SYS_PAUSE: usize = 34;
    pub const SYS_NANOSLEEP: usize = 35;
    pub const SYS_GETPID: usize = 39;
    pub const SYS_SOCKET: usize = 41;
    pub const SYS_CONNECT: usize = 42;
    pub const SYS_ACCEPT: usize = 43;
    pub const SYS_SENDTO: usize = 44;
    pub const SYS_RECVFROM: usize = 45;
    pub const SYS_CLONE: usize = 56;
    pub const SYS_FORK: usize = 57;
    pub const SYS_VFORK: usize = 58;
    pub const SYS_EXECVE: usize = 59;
    pub const SYS_EXIT: usize = 60;
    pub const SYS_WAIT4: usize = 61;
    pub const SYS_KILL: usize = 62;
    pub const SYS_GETUID: usize = 102;
    pub const SYS_GETGID: usize = 104;
    pub const SYS_GETTID: usize = 186;
    pub const SYS_FUTEX: usize = 202;
    pub const SYS_GETDENTS64: usize = 217;
    pub const SYS_CLOCK_GETTIME: usize = 228;
    pub const SYS_EXIT_GROUP: usize = 231;
}

/// Default syscall handlers (stubs)
mod default_handlers {
    use super::*;

    pub fn sys_read(_args: &[u64; 6]) -> Result<u64, SyscallError> {
        Err(SyscallError::NotSupported)
    }

    pub fn sys_write(_args: &[u64; 6]) -> Result<u64, SyscallError> {
        Err(SyscallError::NotSupported)
    }

    pub fn sys_open(_args: &[u64; 6]) -> Result<u64, SyscallError> {
        Err(SyscallError::NotSupported)
    }

    pub fn sys_close(_args: &[u64; 6]) -> Result<u64, SyscallError> {
        Err(SyscallError::NotSupported)
    }

    pub fn sys_getpid(_args: &[u64; 6]) -> Result<u64, SyscallError> {
        Ok(1) // Return PID 1 for now
    }

    pub fn sys_exit(args: &[u64; 6]) -> Result<u64, SyscallError> {
        let _exit_code = args[0];
        // TODO: Terminate current process
        loop {
            unsafe {
                asm!("hlt", options(nomem, nostack));
            }
        }
    }
}

/// Initialize default syscall handlers
pub fn init_default_handlers() {
    use syscall_numbers::*;
    use default_handlers::*;

    let _ = register_syscall(SYS_READ, sys_read);
    let _ = register_syscall(SYS_WRITE, sys_write);
    let _ = register_syscall(SYS_OPEN, sys_open);
    let _ = register_syscall(SYS_CLOSE, sys_close);
    let _ = register_syscall(SYS_GETPID, sys_getpid);
    let _ = register_syscall(SYS_EXIT, sys_exit);
}

/// Initialize the syscall subsystem
pub unsafe fn init() {
    init_syscall();
    init_default_handlers();
}
