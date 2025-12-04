//! Syscall Entry Point for x86_64
//! 
//! SYSCALL/SYSRET fast path implementation with full syscall dispatch.
//!
//! ## Syscall ABI (Linux compatible)
//! - RAX = syscall number
//! - RDI = arg1, RSI = arg2, RDX = arg3
//! - R10 = arg4, R8 = arg5, R9 = arg6
//! - Return: RAX (negative = -errno)

use core::arch::asm;
use crate::memory::VirtualAddress;

/// MSR addresses
const IA32_STAR: u32 = 0xC0000081;
const IA32_LSTAR: u32 = 0xC0000082;
const IA32_FMASK: u32 = 0xC0000084;
const IA32_EFER: u32 = 0xC0000080;
const IA32_KERNEL_GS_BASE: u32 = 0xC0000102;

// ═══════════════════════════════════════════════════════════════════════════════
// Syscall Numbers (Linux x86_64 compatible)
// ═══════════════════════════════════════════════════════════════════════════════

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_READLINK: u64 = 89;
pub const SYS_GETUID: u64 = 102;
pub const SYS_GETGID: u64 = 104;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_GETTID: u64 = 186;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_GETRANDOM: u64 = 318;

// Error codes
pub const ENOSYS: i64 = -38;
pub const EBADF: i64 = -9;
pub const EINVAL: i64 = -22;
pub const EFAULT: i64 = -14;
pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;

// ═══════════════════════════════════════════════════════════════════════════════
// External ASM symbols
// ═══════════════════════════════════════════════════════════════════════════════

extern "C" {
    fn syscall_entry();
    fn syscall_entry_simple();
    fn set_kernel_stack(stack: u64);
    fn get_user_rsp() -> u64;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Per-CPU Data for syscalls
// ═══════════════════════════════════════════════════════════════════════════════

/// Per-CPU syscall data
#[repr(C)]
pub struct SyscallCpuData {
    /// User RSP saved on syscall entry
    pub user_rsp: u64,
    /// Kernel stack to use during syscall
    pub kernel_rsp: u64,
    /// Current task pointer
    pub current_task: u64,
}

/// Default kernel stack for syscalls (64KB)
static mut SYSCALL_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

/// Per-CPU data (single CPU for now)
static mut CPU_DATA: SyscallCpuData = SyscallCpuData {
    user_rsp: 0,
    kernel_rsp: 0,
    current_task: 0,
};

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

/// Initialize SYSCALL/SYSRET
pub fn init() {
    unsafe {
        // Set up per-CPU data
        let stack_top = SYSCALL_STACK.as_ptr().add(SYSCALL_STACK.len()) as u64;
        CPU_DATA.kernel_rsp = stack_top;
        
        // Set kernel GS base to point to CPU data
        let cpu_data_addr = &CPU_DATA as *const _ as u64;
        wrmsr(IA32_KERNEL_GS_BASE, cpu_data_addr);
        
        // Set STAR: segment selectors for syscall/sysret
        // Bits 32-47: Kernel CS (0x08) / SS (0x10) base
        // Bits 48-63: User CS (0x1B) / SS (0x23) base (with RPL=3 added by SYSRET)
        // SYSRET loads CS from STAR[48:63]+16, SS from STAR[48:63]+8
        let star: u64 = (0x08u64 << 32) | (0x18u64 << 48);
        wrmsr(IA32_STAR, star);

        // Set LSTAR: syscall entry point
        // Use simple entry for now (no GS swap) until per-CPU is fully set up
        let lstar = syscall_entry_simple as u64;
        wrmsr(IA32_LSTAR, lstar);

        // Set FMASK: flags to clear on syscall (clear IF to disable interrupts)
        let fmask: u64 = 0x200; 
        wrmsr(IA32_FMASK, fmask);

        // Enable SYSCALL in EFER
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | 1);

        log::info!("SYSCALL/SYSRET initialized: LSTAR={:#x}, stack={:#x}", lstar, stack_top);
    }
}

/// Read MSR
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (high, low): (u32, u32);
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write MSR
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack)
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Syscall Handler
// ═══════════════════════════════════════════════════════════════════════════════

/// Main Rust syscall handler
/// 
/// Called from assembly with:
/// - rdi = syscall number
/// - rsi = arg1, rdx = arg2, rcx = arg3, r8 = arg4, r9 = arg5
/// 
/// Returns result in RAX
#[no_mangle]
pub extern "C" fn syscall_handler_rust(
    syscall_num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
) -> i64 {
    match syscall_num {
        // ─────────────────────────────────────────────────────────────
        // Process Control
        // ─────────────────────────────────────────────────────────────
        SYS_EXIT => {
            sys_exit(arg1 as i32)
        }
        SYS_EXIT_GROUP => {
            sys_exit(arg1 as i32)
        }
        SYS_GETPID => {
            sys_getpid()
        }
        SYS_GETTID => {
            sys_gettid()
        }
        SYS_GETPPID => {
            sys_getppid()
        }
        SYS_GETUID | SYS_GETEUID => {
            0 // root
        }
        SYS_GETGID | SYS_GETEGID => {
            0 // root
        }
        
        // ─────────────────────────────────────────────────────────────
        // File I/O
        // ─────────────────────────────────────────────────────────────
        SYS_READ => {
            sys_read(arg1 as i32, arg2 as *mut u8, arg3 as usize)
        }
        SYS_WRITE => {
            sys_write(arg1 as i32, arg2 as *const u8, arg3 as usize)
        }
        SYS_OPEN => {
            sys_open(arg1 as *const u8, arg2 as i32, arg3 as u32)
        }
        SYS_OPENAT => {
            sys_openat(arg1 as i32, arg2 as *const u8, arg3 as i32, arg4 as u32)
        }
        SYS_CLOSE => {
            sys_close(arg1 as i32)
        }
        SYS_LSEEK => {
            sys_lseek(arg1 as i32, arg2 as i64, arg3 as i32)
        }
        SYS_FSTAT | SYS_STAT | SYS_LSTAT | SYS_NEWFSTATAT => {
            // Return dummy stat for now
            0
        }
        SYS_IOCTL => {
            // Ignore most ioctls
            0
        }
        
        // ─────────────────────────────────────────────────────────────
        // Memory Management
        // ─────────────────────────────────────────────────────────────
        SYS_BRK => {
            sys_brk(arg1 as usize)
        }
        SYS_MMAP => {
            sys_mmap(arg1, arg2 as usize, arg3 as i32, arg4 as i32, arg5 as i32, 0)
        }
        SYS_MUNMAP => {
            sys_munmap(arg1, arg2 as usize)
        }
        SYS_MPROTECT => {
            0 // Success (ignored for now)
        }
        
        // ─────────────────────────────────────────────────────────────
        // Misc
        // ─────────────────────────────────────────────────────────────
        SYS_ARCH_PRCTL => {
            sys_arch_prctl(arg1 as i32, arg2)
        }
        SYS_SET_TID_ADDRESS => {
            sys_gettid() // Return TID
        }
        SYS_UNAME => {
            sys_uname(arg1 as *mut u8)
        }
        SYS_GETRANDOM => {
            sys_getrandom(arg1 as *mut u8, arg2 as usize, arg3 as u32)
        }
        SYS_CLOCK_GETTIME => {
            // Return zeros for now
            0
        }
        SYS_NANOSLEEP => {
            // Just return success
            0
        }
        
        // ─────────────────────────────────────────────────────────────
        // Not implemented
        // ─────────────────────────────────────────────────────────────
        _ => {
            log::warn!("Unimplemented syscall: {} (args: {:#x}, {:#x}, {:#x})", 
                syscall_num, arg1, arg2, arg3);
            ENOSYS
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Syscall Implementations
// ═══════════════════════════════════════════════════════════════════════════════

/// sys_exit - Terminate the calling process
fn sys_exit(status: i32) -> i64 {
    log::info!("Process exiting with status {}", status);
    
    // For now, just halt. Later: remove from scheduler, free resources
    // TODO: Proper process termination
    loop {
        unsafe { asm!("hlt"); }
    }
}

/// sys_getpid - Get process ID
fn sys_getpid() -> i64 {
    // TODO: Get from current process
    1
}

/// sys_gettid - Get thread ID
fn sys_gettid() -> i64 {
    // TODO: Get from current thread
    1
}

/// sys_getppid - Get parent process ID  
fn sys_getppid() -> i64 {
    0 // init has no parent
}

/// sys_write - Write to a file descriptor
fn sys_write(fd: i32, buf: *const u8, count: usize) -> i64 {
    // Validate pointer (basic check)
    if buf.is_null() {
        return EFAULT;
    }
    
    match fd {
        // stdout (1) or stderr (2) - write to serial console
        1 | 2 => {
            unsafe {
                for i in 0..count {
                    let byte = *buf.add(i);
                    // Write to serial port 0x3F8
                    crate::arch::x86_64::serial::write_byte(byte);
                }
            }
            count as i64
        }
        _ => {
            // TODO: Look up fd in process file table and write to VFS
            EBADF
        }
    }
}

/// sys_read - Read from a file descriptor
fn sys_read(fd: i32, buf: *mut u8, count: usize) -> i64 {
    if buf.is_null() {
        return EFAULT;
    }
    
    match fd {
        // stdin (0) - read from serial/keyboard
        0 => {
            // TODO: Implement proper input handling
            0 // EOF for now
        }
        _ => {
            // TODO: Look up fd in process file table
            EBADF
        }
    }
}

/// sys_open - Open a file
fn sys_open(pathname: *const u8, flags: i32, mode: u32) -> i64 {
    if pathname.is_null() {
        return EFAULT;
    }
    
    // TODO: Parse path string, call VFS open
    // For now, return error
    ENOENT
}

/// sys_openat - Open file relative to directory fd
fn sys_openat(dirfd: i32, pathname: *const u8, flags: i32, mode: u32) -> i64 {
    // AT_FDCWD (-100) means current directory
    sys_open(pathname, flags, mode)
}

/// sys_close - Close a file descriptor
fn sys_close(fd: i32) -> i64 {
    // TODO: Implement FD table
    0
}

/// sys_lseek - Reposition file offset
fn sys_lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    // TODO: Implement
    EBADF
}

/// sys_brk - Change data segment size
fn sys_brk(addr: usize) -> i64 {
    // Simple brk implementation
    // TODO: Track per-process heap
    static mut CURRENT_BRK: usize = 0x1000_0000;
    
    unsafe {
        if addr == 0 {
            return CURRENT_BRK as i64;
        }
        
        if addr > CURRENT_BRK {
            // Expand - would need to map pages
            CURRENT_BRK = addr;
        }
        
        CURRENT_BRK as i64
    }
}

/// sys_mmap - Map memory
fn sys_mmap(addr: u64, length: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> i64 {
    use crate::memory::{mmap, PageProtection, VirtualAddress};
    use crate::memory::mmap::MmapFlags;
    
    let vaddr = if addr == 0 { None } else { Some(VirtualAddress::new(addr as usize)) };
    let mm_flags = MmapFlags::new(flags as u32);
    
    let protection = PageProtection::from_prot(prot as u32);
    
    match mmap::mmap(vaddr, length, protection, mm_flags, if fd < 0 { None } else { Some(fd) }, offset as usize) {
        Ok(mapped_addr) => mapped_addr.value() as i64,
        Err(_) => -12, // ENOMEM
    }
}

/// sys_munmap - Unmap memory
fn sys_munmap(addr: u64, length: usize) -> i64 {
    use crate::memory::{mmap, VirtualAddress};
    
    match mmap::munmap(VirtualAddress::new(addr as usize), length) {
        Ok(()) => 0,
        Err(_) => EINVAL,
    }
}

/// sys_arch_prctl - Architecture-specific thread state
fn sys_arch_prctl(code: i32, addr: u64) -> i64 {
    const ARCH_SET_GS: i32 = 0x1001;
    const ARCH_SET_FS: i32 = 0x1002;
    const ARCH_GET_FS: i32 = 0x1003;
    const ARCH_GET_GS: i32 = 0x1004;
    
    const IA32_FS_BASE: u32 = 0xC0000100;
    const IA32_GS_BASE: u32 = 0xC0000101;
    
    unsafe {
        match code {
            ARCH_SET_FS => {
                wrmsr(IA32_FS_BASE, addr);
                0
            }
            ARCH_SET_GS => {
                wrmsr(IA32_GS_BASE, addr);
                0
            }
            ARCH_GET_FS => {
                let fs = rdmsr(IA32_FS_BASE);
                *(addr as *mut u64) = fs;
                0
            }
            ARCH_GET_GS => {
                let gs = rdmsr(IA32_GS_BASE);
                *(addr as *mut u64) = gs;
                0
            }
            _ => EINVAL,
        }
    }
}

/// sys_uname - Get system information
fn sys_uname(buf: *mut u8) -> i64 {
    if buf.is_null() {
        return EFAULT;
    }
    
    // struct utsname { char [65] for each field }
    // sysname, nodename, release, version, machine, domainname
    let utsname = b"Exo-OS\0";
    
    unsafe {
        // sysname
        core::ptr::copy_nonoverlapping(utsname.as_ptr(), buf, utsname.len());
        // Fill rest with zeros
        core::ptr::write_bytes(buf.add(utsname.len()), 0, 65 - utsname.len());
        // nodename
        core::ptr::copy_nonoverlapping(b"exo\0".as_ptr(), buf.add(65), 4);
        // release
        core::ptr::copy_nonoverlapping(b"0.5.0\0".as_ptr(), buf.add(130), 6);
        // version
        core::ptr::copy_nonoverlapping(b"#1\0".as_ptr(), buf.add(195), 3);
        // machine
        core::ptr::copy_nonoverlapping(b"x86_64\0".as_ptr(), buf.add(260), 7);
    }
    
    0
}

/// sys_getrandom - Get random bytes
fn sys_getrandom(buf: *mut u8, count: usize, flags: u32) -> i64 {
    if buf.is_null() {
        return EFAULT;
    }
    
    unsafe {
        // Use RDRAND if available, otherwise simple PRNG
        for i in 0..count {
            let mut val: u64 = 0;
            let success: u8;
            asm!(
                "rdrand {0}",
                "setc {1}",
                out(reg) val,
                out(reg_byte) success,
            );
            
            if success == 0 {
                // RDRAND failed, use timestamp as fallback
                asm!("rdtsc", out("eax") val, out("edx") _, options(nomem, nostack));
            }
            
            *buf.add(i) = (val ^ (val >> 8)) as u8;
        }
    }
    
    count as i64
}
