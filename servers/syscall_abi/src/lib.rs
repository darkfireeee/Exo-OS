#![no_std]

#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    unsafe { syscall6(nr, a1, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let ret: i64;
    // SAFETY: the caller is responsible for passing kernel-valid arguments.
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    unsafe { syscall6(nr, 0, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall5(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, a5, 0) }
}

pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_KILL: u64 = 62;
pub const SYS_GETPID: u64 = 39;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141;
pub const SYS_SCHED_SETPARAM: u64 = 142;
pub const SYS_SCHED_GETPARAM: u64 = 143;
pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
pub const SYS_SCHED_GETSCHEDULER: u64 = 145;
pub const SYS_SCHED_GET_PRIORITY_MAX: u64 = 146;
pub const SYS_SCHED_GET_PRIORITY_MIN: u64 = 147;
pub const SYS_SCHED_RR_GET_INTERVAL: u64 = 148;
pub const SYS_SCHED_SETAFFINITY: u64 = 203;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;

pub const SYS_EXO_IPC_SEND: u64 = 300;
pub const SYS_EXO_IPC_RECV: u64 = 301;
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
pub const SYS_EXO_IPC_CALL: u64 = 303;
pub const SYS_EXO_IPC_CREATE: u64 = 304;
pub const SYS_EXO_IPC_DESTROY: u64 = 305;

pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;

pub const SYS_IPC_REGISTER: u64 = SYS_EXO_IPC_CREATE;
pub const SYS_IPC_RECV: u64 = SYS_EXO_IPC_RECV;
pub const SYS_IPC_SEND: u64 = SYS_EXO_IPC_SEND;

pub const SYS_MMIO_MAP: u64 = 532;
pub const SYS_MMIO_UNMAP: u64 = 533;
pub const SYS_DMA_ALLOC: u64 = 534;
pub const SYS_DMA_FREE: u64 = 535;
pub const SYS_DMA_SYNC: u64 = 536;
pub const SYS_PCI_CFG_READ: u64 = 537;
pub const SYS_PCI_CFG_WRITE: u64 = 538;
pub const SYS_PCI_BUS_MASTER: u64 = 539;
pub const SYS_PCI_CLAIM: u64 = 540;
pub const SYS_DMA_MAP: u64 = 541;
pub const SYS_DMA_UNMAP: u64 = 542;
pub const SYS_MSI_ALLOC: u64 = 543;
pub const SYS_MSI_CONFIG: u64 = 544;
pub const SYS_MSI_FREE: u64 = 545;
pub const SYS_PCI_SET_TOPOLOGY: u64 = 546;

pub const O_RDONLY: u64 = 0;

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const IPC_FLAG_TIMEOUT: u64 = 0x0001;
pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const EINTR: i64 = -4;
pub const EIO: i64 = -5;
pub const E2BIG: i64 = -7;
pub const EAGAIN: i64 = -11;
pub const ENOMEM: i64 = -12;
pub const EACCES: i64 = -13;
pub const EFAULT: i64 = -14;
pub const EBUSY: i64 = -16;
pub const EEXIST: i64 = -17;
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const ENODEV: i64 = -19;
pub const EINVAL: i64 = -22;
pub const EMFILE: i64 = -24;
pub const EPIPE: i64 = -32;
pub const ENOSPC: i64 = -28;
pub const EAFNOSUPPORT: i64 = -97;
pub const EADDRINUSE: i64 = -98;
pub const ENOBUFS: i64 = -105;
pub const ENOTCONN: i64 = -107;
pub const ENETDOWN: i64 = -100;
pub const ENETUNREACH: i64 = -101;
pub const ENOSYS: i64 = -38;
pub const ETIMEDOUT: i64 = -110;
