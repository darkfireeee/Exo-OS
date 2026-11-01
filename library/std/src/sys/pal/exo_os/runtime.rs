//! Exo-OS low-level runtime inside `std::sys::pal::exo_os::runtime`:
//! - `GenericTcb` & `ExoTcb` at `%fs:0` (with self-pointer at `%fs:16` and `SignalTcb*` at `%fs:32`)
//! - `SignalTcb` 0-syscall `sigaction` (`LOCK CMPXCHG16B`) and `sigprocmask` (`AtomicU64`)
//! - `exo_sigreturn_trampoline` (`SYS_RT_SIGRETURN = 15`)
//! - `exo_rt_init_from_stack` parsing `argc`, `argv`, `envp`, `auxv` (`AT_SIGNAL_TCB = 51`, `AT_CAP_TOKEN = 52`, `AT_RANDOM = 25`, `AT_PAGESZ = 6`)
//! - `fork()` and `execve()` complying with `PROC-03..PROC-10`

#![allow(dead_code)]

use crate::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};
use super::syscall::{self, CapTokenUser};

pub const AT_NULL: usize = 0;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_ENTRY: usize = 9;
pub const AT_RANDOM: usize = 25;
pub const AT_SYSINFO_EHDR: usize = 33;
pub const AT_SIGNAL_TCB: usize = 51;
pub const AT_CAP_TOKEN: usize = 52;

pub const SIGKILL: u8 = 9;
pub const SIGSTOP: u8 = 19;
pub const NON_BLOCKABLE_MASK: u64 = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
pub const SIGRETURN_MAGIC: u32 = 0x5349_474E;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

#[repr(C)]
pub struct GenericTcb<Os> {
    pub tls_end: *mut u8,
    pub tls_len: usize,
    pub tcb_ptr: *mut Self,
    pub tcb_len: usize,
    pub os_specific: Os,
}

pub type ExoTcb = GenericTcb<*mut SignalTcb>;

impl ExoTcb {
    #[inline(always)]
    pub unsafe fn current_ptr() -> *mut Self {
        #[cfg(target_arch = "x86_64")]
        {
            let ptr: *mut Self;
            unsafe {
                core::arch::asm!(
                    "mov {}, fs:[16]",
                    out(reg) ptr,
                    options(nostack, preserves_flags, readonly)
                );
            }
            ptr
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            core::ptr::null_mut()
        }
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SigActionSlot {
    pub handler_vaddr: u64,
    pub flags: u32,
    pub _pad: u32,
    pub mask: u64,
    pub restorer: u64,
}

const _: () = assert!(core::mem::size_of::<SigActionSlot>() == 32);

#[repr(C, align(64))]
pub struct SignalTcb {
    pub blocked: AtomicU64,
    pub pending: AtomicU64,
    pub handlers: [SigActionSlot; 64],
    pub in_handler: AtomicU32,
    pub _pad1: u32,
    pub altstack_sp: AtomicU64,
    pub altstack_size: AtomicU64,
    pub altstack_flags: AtomicU32,
    pub _pad2: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<SignalTcb>() <= 4096);

static GLOBAL_SIGNAL_TCB_ADDR: AtomicUsize = AtomicUsize::new(0);
static INITIAL_CAP_TOKEN_ADDR: AtomicUsize = AtomicUsize::new(0);
static AT_RANDOM_ADDR: AtomicUsize = AtomicUsize::new(0);
static SAVED_PAGESZ: AtomicUsize = AtomicUsize::new(4096);
static SAVED_ARGC: AtomicUsize = AtomicUsize::new(0);
static SAVED_ARGV: AtomicUsize = AtomicUsize::new(0);
static SAVED_ENVP: AtomicUsize = AtomicUsize::new(0);

#[inline]
pub fn signal_tcb() -> Option<&'static SignalTcb> {
    let addr = GLOBAL_SIGNAL_TCB_ADDR.load(Ordering::Acquire);
    if addr == 0 {
        None
    } else {
        Some(unsafe { &*(addr as *const SignalTcb) })
    }
}

#[inline]
pub fn initial_cap_token_ptr() -> *const CapTokenUser {
    let addr = INITIAL_CAP_TOKEN_ADDR.load(Ordering::Acquire);
    if addr == 0 {
        core::ptr::null()
    } else {
        addr as *const CapTokenUser
    }
}

#[inline]
pub fn at_random_ptr() -> *const [u8; 16] {
    let addr = AT_RANDOM_ADDR.load(Ordering::Acquire);
    if addr == 0 {
        core::ptr::null()
    } else {
        addr as *const [u8; 16]
    }
}

#[inline]
pub fn page_size() -> usize {
    let sz = SAVED_PAGESZ.load(Ordering::Relaxed);
    if sz == 0 { 4096 } else { sz }
}

#[inline]
pub fn saved_args() -> (usize, *const *const u8, *const *const u8) {
    (
        SAVED_ARGC.load(Ordering::Acquire),
        SAVED_ARGV.load(Ordering::Acquire) as *const *const u8,
        SAVED_ENVP.load(Ordering::Acquire) as *const *const u8,
    )
}

#[inline]
unsafe fn atomic_write_slot(dst: *mut SigActionSlot, val: SigActionSlot) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let p0 = dst as *mut u64;
        let lo0 = val.handler_vaddr;
        let hi0 = (val.flags as u64) | ((val._pad as u64) << 32);
        write_u128_cmpxchg16b(p0, lo0, hi0);

        let p1 = p0.add(2);
        let lo1 = val.mask;
        let hi1 = val.restorer;
        write_u128_cmpxchg16b(p1, lo1, hi1);
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        dst.write_volatile(val);
    }
}

#[inline]
unsafe fn atomic_read_slot(src: *const SigActionSlot) -> SigActionSlot {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let p0 = src as *mut u64;
        let (lo0, hi0) = read_u128_cmpxchg16b(p0);
        let p1 = p0.add(2);
        let (lo1, hi1) = read_u128_cmpxchg16b(p1);
        SigActionSlot {
            handler_vaddr: lo0,
            flags: hi0 as u32,
            _pad: (hi0 >> 32) as u32,
            mask: lo1,
            restorer: hi1,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        src.read_volatile()
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn write_u128_cmpxchg16b(ptr: *mut u64, new_lo: u64, new_hi: u64) {
    unsafe {
        let mut prev_lo = ptr.read_volatile();
        let mut prev_hi = ptr.add(1).read_volatile();
        loop {
            let ok: u8;
            core::arch::asm!(
                "xchg rbx, {rbx_in}",
                "lock cmpxchg16b [{ptr}]",
                "sete {ok}",
                "xchg rbx, {rbx_in}",
                ptr = in(reg) ptr,
                rbx_in = inout(reg) new_lo => _,
                in("rcx") new_hi,
                inout("rax") prev_lo,
                inout("rdx") prev_hi,
                ok = out(reg_byte) ok,
                options(nostack)
            );
            if ok != 0 {
                break;
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn read_u128_cmpxchg16b(ptr: *mut u64) -> (u64, u64) {
    let mut lo = 0u64;
    let mut hi = 0u64;
    let zero = 0u64;
    unsafe {
        core::arch::asm!(
            "xchg rbx, {zero}",
            "xor rcx, rcx",
            "lock cmpxchg16b [{ptr}]",
            "xchg rbx, {zero}",
            ptr = in(reg) ptr,
            zero = inout(reg) zero => _,
            inout("rax") lo,
            inout("rdx") hi,
            out("rcx") _,
            options(nostack)
        );
    }
    (lo, hi)
}

pub fn sigaction_0syscall(
    sig: u8,
    new_act: Option<SigActionSlot>,
    old_act: Option<&mut SigActionSlot>,
) -> Result<(), i32> {
    if sig == 0 || sig > 64 || sig == SIGKILL || sig == SIGSTOP {
        return Err(22); // EINVAL
    }
    let idx = (sig - 1) as usize;
    let Some(tcb) = signal_tcb() else {
        return Err(38); // ENOSYS
    };
    let slot_ptr = &tcb.handlers[idx] as *const SigActionSlot as *mut SigActionSlot;
    if let Some(old_out) = old_act {
        *old_out = unsafe { atomic_read_slot(slot_ptr) };
    }
    if let Some(mut act) = new_act {
        act.mask &= !NON_BLOCKABLE_MASK;
        if act.restorer == 0 {
            act.restorer = exo_sigreturn_trampoline as *const () as usize as u64;
        }
        unsafe { atomic_write_slot(slot_ptr, act) };
    }
    Ok(())
}

pub fn sigprocmask_0syscall(
    how: i32,
    set: Option<u64>,
    oldset: Option<&mut u64>,
) -> Result<(), i32> {
    let Some(tcb) = signal_tcb() else {
        return Err(38); // ENOSYS
    };
    let prev = tcb.blocked.load(Ordering::Acquire);
    if let Some(old) = oldset {
        *old = prev;
    }
    if let Some(mask) = set {
        let clean = mask & !NON_BLOCKABLE_MASK;
        match how {
            SIG_BLOCK => {
                tcb.blocked.fetch_or(clean, Ordering::AcqRel);
            }
            SIG_UNBLOCK => {
                tcb.blocked.fetch_and(!clean, Ordering::AcqRel);
            }
            SIG_SETMASK => {
                tcb.blocked.store(clean, Ordering::Release);
            }
            _ => return Err(22), // EINVAL
        }
        fence(Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub unsafe extern "C" fn exo_sigreturn_trampoline() -> ! {
    core::arch::naked_asm!(
        "mov eax, 15",
        "syscall",
        "ud2",
    );
}

#[cfg(not(target_arch = "x86_64"))]
pub unsafe extern "C" fn exo_sigreturn_trampoline() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[inline]
pub fn fork() -> Result<u32, i32> {
    fence(Ordering::SeqCst);
    let res = unsafe {
        syscall::clone_raw(
            17, /* SIGCHLD */
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            0,
        )
    }
    .map_err(|e| e.errno())?;
    if res == 0 {
        if let Some(tcb) = signal_tcb() {
            tcb.pending.store(0, Ordering::Release);
            tcb.in_handler.store(0, Ordering::Release);
        }
    }
    Ok(res)
}

#[inline]
pub unsafe fn execve(
    path_cstr: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> Result<(), i32> {
    unsafe { syscall::execve_raw(path_cstr, argv, envp).map_err(|e| e.errno()) }
}

pub unsafe fn exo_rt_init_from_stack(
    sp: *const usize,
) -> (usize, *const *const u8, *const *const u8) {
    if sp.is_null() {
        return (0, core::ptr::null(), core::ptr::null());
    }
    unsafe {
        let argc = *sp;
        let argv = sp.add(1) as *const *const u8;
        let envp = sp.add(1 + argc + 1) as *const *const u8;

        let mut env_cursor = envp;
        while !(*env_cursor).is_null() {
            env_cursor = env_cursor.add(1);
        }
        let mut auxv_ptr = env_cursor.add(1) as *const usize;

        loop {
            let a_type = *auxv_ptr;
            let a_val = *auxv_ptr.add(1);
            if a_type == AT_NULL {
                break;
            }
            match a_type {
                AT_PAGESZ => {
                    if a_val != 0 {
                        SAVED_PAGESZ.store(a_val, Ordering::Release);
                    }
                }
                AT_RANDOM => {
                    AT_RANDOM_ADDR.store(a_val, Ordering::Release);
                }
                AT_SIGNAL_TCB => {
                    GLOBAL_SIGNAL_TCB_ADDR.store(a_val, Ordering::Release);
                }
                AT_CAP_TOKEN => {
                    INITIAL_CAP_TOKEN_ADDR.store(a_val, Ordering::Release);
                }
                _ => {}
            }
            auxv_ptr = auxv_ptr.add(2);
        }

        SAVED_ARGC.store(argc, Ordering::Release);
        SAVED_ARGV.store(argv as usize, Ordering::Release);
        SAVED_ENVP.store(envp as usize, Ordering::Release);

        let tcb_ptr = ExoTcb::current_ptr();
        if !tcb_ptr.is_null() {
            let sig_addr = GLOBAL_SIGNAL_TCB_ADDR.load(Ordering::Acquire);
            if sig_addr != 0 {
                (*tcb_ptr).os_specific = sig_addr as *mut SignalTcb;
            }
        }

        (argc, argv, envp)
    }
}
