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
pub const AT_EXECFN: usize = 31;
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
    pub tcb_ptr: *mut Self,
    pub dtv: *mut usize,
    pub prev: *mut u8,
    pub next: *mut u8,
    pub sysinfo: usize,
    pub canary: usize,
    pub tid: i32,
    pub errno: i32,
    pub tls_end: *mut u8,
    pub tls_len: usize,
    pub tcb_len: usize,
    pub os_specific: Os,
}

const _: () = {
    assert!(core::mem::offset_of!(GenericTcb<usize>, tcb_ptr) == 0);
    assert!(core::mem::offset_of!(GenericTcb<usize>, dtv) == 8);
    assert!(core::mem::offset_of!(GenericTcb<usize>, prev) == 16);
    assert!(core::mem::offset_of!(GenericTcb<usize>, next) == 24);
    assert!(core::mem::offset_of!(GenericTcb<usize>, sysinfo) == 32);
    assert!(core::mem::offset_of!(GenericTcb<usize>, canary) == 40);
    assert!(core::mem::offset_of!(GenericTcb<usize>, tid) == 48);
    assert!(core::mem::offset_of!(GenericTcb<usize>, errno) == 52);
    assert!(core::mem::offset_of!(GenericTcb<usize>, tls_end) == 56);
    assert!(core::mem::offset_of!(GenericTcb<usize>, tls_len) == 64);
    assert!(core::mem::offset_of!(GenericTcb<usize>, tcb_len) == 72);
    assert!(core::mem::offset_of!(GenericTcb<usize>, os_specific) == 80);
    assert!(core::mem::size_of::<GenericTcb<usize>>() == 88);
};

pub type ExoTcb = GenericTcb<*mut SignalTcb>;

impl ExoTcb {
    #[inline(always)]
    pub unsafe fn current_ptr() -> *mut Self {
        #[cfg(target_arch = "x86_64")]
        {
            let ptr: *mut Self;
            unsafe {
                core::arch::asm!(
                    "mov {}, fs:[0]",
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

    #[inline(always)]
    pub unsafe fn errno_ptr() -> *mut i32 {
        let ptr = unsafe { Self::current_ptr() };
        if ptr.is_null() {
            core::ptr::null_mut()
        } else {
            unsafe { &raw mut (*ptr).errno }
        }
    }
}

#[inline]
pub fn sync_thread_tcb_tid() {
    let tid = syscall::gettid() as i32;
    let tcb_ptr = unsafe { ExoTcb::current_ptr() };
    if !tcb_ptr.is_null() {
        unsafe {
            (*tcb_ptr).tid = tid;
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
static SAVED_PHDR: AtomicUsize = AtomicUsize::new(0);
static SAVED_PHENT: AtomicUsize = AtomicUsize::new(0);
static SAVED_PHNUM: AtomicUsize = AtomicUsize::new(0);
static SAVED_AT_BASE: AtomicUsize = AtomicUsize::new(0);
static SAVED_EXECFN: AtomicUsize = AtomicUsize::new(0);
static DYN_TLS_ALLOC_HOOK: AtomicUsize = AtomicUsize::new(0);
static DYN_TLS_FREE_HOOK: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,
    r_info: u64,
    r_addend: i64,
}

const DT_NULL: i64 = 0;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_RELAENT: i64 = 9;
const R_X86_64_RELATIVE: u64 = 8;

#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Sym {
    st_name: u32,
    st_info: u8,
    st_other: u8,
    st_shndx: u16,
    st_value: u64,
    st_size: u64,
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;

pub fn register_dynamic_tls_hooks(alloc_fn: usize, free_fn: usize) {
    DYN_TLS_ALLOC_HOOK.store(alloc_fn, Ordering::Release);
    DYN_TLS_FREE_HOOK.store(free_fn, Ordering::Release);
}

#[cfg(target_arch = "x86_64")]
unsafe fn lookup_interp_sym(interp_base: usize, target_name: &[u8]) -> usize {
    if interp_base == 0 {
        return 0;
    }
    let ehdr = interp_base as *const u8;
    let magic = unsafe { core::slice::from_raw_parts(ehdr, 4) };
    if magic != b"\x7FELF" {
        return 0;
    }
    let e_phoff = unsafe { (ehdr.add(32) as *const u64).read_unaligned() as usize };
    let e_phent = unsafe { (ehdr.add(54) as *const u16).read_unaligned() as usize };
    let e_phnum = unsafe { (ehdr.add(56) as *const u16).read_unaligned() as usize };
    if e_phent < core::mem::size_of::<Elf64Phdr>() || e_phnum == 0 {
        return 0;
    }

    let mut dyn_vaddr = 0usize;
    for i in 0..e_phnum {
        let ph = unsafe {
            ((interp_base + e_phoff + i * e_phent) as *const Elf64Phdr).read_unaligned()
        };
        if ph.p_type == PT_DYNAMIC {
            dyn_vaddr = interp_base.wrapping_add(ph.p_vaddr as usize);
            break;
        }
    }
    if dyn_vaddr == 0 {
        return 0;
    }

    let mut symtab = 0usize;
    let mut strtab = 0usize;
    let mut hash = 0usize;
    let dynp = dyn_vaddr as *const Elf64Dyn;
    for i in 0..512 {
        let d = unsafe { dynp.add(i).read_unaligned() };
        match d.d_tag {
            0 => break,
            4 => hash = interp_base.wrapping_add(d.d_val as usize),
            5 => strtab = interp_base.wrapping_add(d.d_val as usize),
            6 => symtab = interp_base.wrapping_add(d.d_val as usize),
            _ => {}
        }
    }
    if symtab == 0 || strtab == 0 || hash == 0 {
        return 0;
    }

    let mut h = 0u32;
    for &b in target_name {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }

    let hash_u32 = hash as *const u32;
    let nbuckets = unsafe { *hash_u32 } as usize;
    let nchain = unsafe { *hash_u32.add(1) } as usize;
    if nbuckets == 0 || nchain == 0 {
        return 0;
    }
    let buckets = unsafe { hash_u32.add(2) };
    let chains = unsafe { buckets.add(nbuckets) };
    let mut idx = unsafe { *buckets.add((h as usize) % nbuckets) } as usize;
    let mut steps = 0usize;
    while idx != 0 && idx < nchain && steps < nchain {
        let sym = unsafe { *(symtab as *const Elf64Sym).add(idx) };
        if sym.st_shndx != 0 {
            let s_ptr = unsafe { (strtab as *const u8).add(sym.st_name as usize) };
            let mut matched = true;
            let mut k = 0usize;
            while k < target_name.len() {
                if unsafe { *s_ptr.add(k) } != target_name[k] {
                    matched = false;
                    break;
                }
                k += 1;
            }
            if matched && unsafe { *s_ptr.add(target_name.len()) } == 0 {
                return interp_base.wrapping_add(sym.st_value as usize);
            }
        }
        idx = unsafe { *chains.add(idx) } as usize;
        steps += 1;
    }
    0
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn lookup_interp_sym(_interp_base: usize, _target_name: &[u8]) -> usize {
    0
}

/// Releases dynamic `DTV` module allocations owned by the currently exiting thread.
pub unsafe fn free_current_thread_dyn_tls() {
    let hook = DYN_TLS_FREE_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        let tcb = unsafe { ExoTcb::current_ptr() };
        if !tcb.is_null() {
            type FreeFn = unsafe extern "C" fn(*mut u8);
            let f: FreeFn = unsafe { core::mem::transmute(hook) };
            unsafe { f(tcb as *mut u8) };
        }
    }
}

/// Allocates and initializes a per-thread TLS block (`.tdata` + `.tbss` + `ExoTcb` at `%fs:0`).
/// Returns `(mmap_base_ptr, mmap_alloc_len, fs_base_vaddr)`.
pub unsafe fn allocate_thread_tls() -> Result<(*mut u8, usize, u64), i32> {
    let hook = DYN_TLS_ALLOC_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        type AllocFn = unsafe extern "C" fn(*const u8, *mut usize, *mut usize, *mut u64) -> i32;
        let f: AllocFn = unsafe { core::mem::transmute(hook) };
        let parent_tcb = unsafe { ExoTcb::current_ptr() };
        let mut raw_base = 0usize;
        let mut alloc_len = 0usize;
        let mut fs_base = 0u64;
        let rc = unsafe { f(parent_tcb as *const u8, &mut raw_base, &mut alloc_len, &mut fs_base) };
        if rc == 0 && raw_base != 0 {
            return Ok((raw_base as *mut u8, alloc_len, fs_base));
        }
        return Err(if rc < 0 { -rc } else { 12 });
    }

    use super::alloc_dlmalloc::{MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE, sys_mmap};

    let phdr_addr = SAVED_PHDR.load(Ordering::Acquire);
    let phent = SAVED_PHENT.load(Ordering::Acquire);
    let phnum = SAVED_PHNUM.load(Ordering::Acquire);

    let mut pt_phdr_vaddr = 0u64;
    let mut tls_vaddr = 0u64;
    let mut tls_filesz = 0usize;
    let mut tls_memsz = 0usize;
    let mut tls_align = 1usize;

    if phdr_addr != 0 && phent >= core::mem::size_of::<Elf64Phdr>() && phnum > 0 {
        for i in 0..phnum {
            let ph_ptr = (phdr_addr + i * phent) as *const Elf64Phdr;
            let ph = unsafe { ph_ptr.read_unaligned() };
            if ph.p_type == PT_PHDR {
                pt_phdr_vaddr = ph.p_vaddr;
            } else if ph.p_type == PT_TLS {
                tls_vaddr = ph.p_vaddr;
                tls_filesz = ph.p_filesz as usize;
                tls_memsz = ph.p_memsz as usize;
                let align = ph.p_align as usize;
                tls_align = if align <= 1 { 1 } else { align.next_power_of_two() };
            }
        }
    }

    let load_bias = if pt_phdr_vaddr != 0 {
        phdr_addr.wrapping_sub(pt_phdr_vaddr as usize)
    } else {
        0
    };

    let filesz = tls_filesz.min(tls_memsz);
    // SysV x86_64 Variant II TLS: tdata sits at tls_base - tls_block_bytes,
    // preserving p_vaddr mod p_align for negative %fs offsets (no forced .max(16)).
    let tls_block_bytes = if tls_memsz == 0 {
        0
    } else {
        let misalign = (0usize
            .wrapping_sub(tls_memsz)
            .wrapping_sub(tls_vaddr as usize))
            & (tls_align - 1);
        tls_memsz + misalign
    };
    let page_sz = page_size();
    let base_align = page_sz.max(tls_align);
    let aligned_block_span = if tls_block_bytes == 0 {
        0
    } else {
        (tls_block_bytes + base_align - 1) & !(base_align - 1)
    };
    let extra_align_pad = base_align.saturating_sub(page_sz);
    let tls_alloc_len = aligned_block_span + extra_align_pad + page_sz;

    let raw_base = unsafe {
        sys_mmap(
            0,
            tls_alloc_len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            !0,
            0,
        )?
    };

    let raw_end_for_block = raw_base + aligned_block_span;
    let tls_base = if tls_align <= page_sz {
        raw_end_for_block
    } else {
        (raw_end_for_block + tls_align - 1) & !(tls_align - 1)
    };
    if filesz > 0 && tls_vaddr != 0 {
        let src_ptr = load_bias.wrapping_add(tls_vaddr as usize) as *const u8;
        let dst_ptr = (tls_base - tls_block_bytes) as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, filesz);
        }
    }

    let parent_tcb = unsafe { ExoTcb::current_ptr() };
    let parent_canary = if !parent_tcb.is_null() {
        unsafe { (*parent_tcb).canary }
    } else {
        0
    };

    let tcb_ptr = tls_base as *mut ExoTcb;
    let dtv_ptr = (tls_base + 96) as *mut usize;
    let tdata_ptr = tls_base - tls_block_bytes;
    let sig_tcb = GLOBAL_SIGNAL_TCB_ADDR.load(Ordering::Acquire) as *mut SignalTcb;
    unsafe {
        (*dtv_ptr) = if tls_memsz > 0 { 1 } else { 0 };
        (*dtv_ptr.add(1)) = if tls_memsz > 0 { tdata_ptr } else { 0 };
        (*tcb_ptr).tcb_ptr = tcb_ptr;
        (*tcb_ptr).dtv = dtv_ptr;
        (*tcb_ptr).prev = core::ptr::null_mut();
        (*tcb_ptr).next = core::ptr::null_mut();
        (*tcb_ptr).sysinfo = 0;
        (*tcb_ptr).canary = parent_canary;
        (*tcb_ptr).tid = 0;
        (*tcb_ptr).errno = 0;
        (*tcb_ptr).tls_end = tls_base as *mut u8;
        (*tcb_ptr).tls_len = tls_alloc_len;
        (*tcb_ptr).tcb_len = page_sz;
        (*tcb_ptr).os_specific = sig_tcb;
    }

    Ok((raw_base as *mut u8, tls_alloc_len, tls_base as u64))
}

/// Frees a per-thread TLS block previously allocated with [`allocate_thread_tls`].
pub unsafe fn free_thread_tls(alloc_base: *mut u8, alloc_len: usize) {
    if !alloc_base.is_null() && alloc_len > 0 {
        let _ = unsafe { super::alloc_dlmalloc::sys_munmap(alloc_base as usize, alloc_len) };
    }
}

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
pub fn at_execfn_ptr() -> *const u8 {
    let addr = SAVED_EXECFN.load(Ordering::Acquire);
    if addr == 0 {
        core::ptr::null()
    } else {
        addr as *const u8
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
            let ok: u64;
            core::arch::asm!(
                "mov r8, rbx",
                "mov rbx, rsi",
                "lock cmpxchg16b [rdi]",
                "sete r9b",
                "mov rbx, r8",
                "movzx r9d, r9b",
                in("rdi") ptr,
                in("rsi") new_lo,
                in("rcx") new_hi,
                inout("rax") prev_lo,
                inout("rdx") prev_hi,
                out("r8") _,
                lateout("r9") ok,
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
    unsafe {
        core::arch::asm!(
            "mov r8, rbx",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "lock cmpxchg16b [rdi]",
            "mov rbx, r8",
            in("rdi") ptr,
            inout("rax") lo,
            inout("rdx") hi,
            out("rcx") _,
            out("r8") _,
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
    let clean_in = set.map(|mask| mask & !NON_BLOCKABLE_MASK);
    let mut prev = 0u64;
    syscall::rt_sigprocmask(
        how,
        clean_in.as_ref(),
        if oldset.is_some() { Some(&mut prev) } else { None },
    )
    .map_err(|e| e.errno())?;
    if let Some(old) = oldset {
        *old = prev;
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

/// Applies `R_X86_64_RELATIVE` relocations in-place for static-PIE (`ET_DYN` without `PT_INTERP`).
///
/// Must execute before any Rust code reads relocated pointers in `.data.rel.ro`, `.data`, or `.got`.
/// Uses only stack locals, raw pointers, and wrapping integer arithmetic (zero statics, zero panics).
#[inline(never)]
pub unsafe fn self_relocate_static_pie(sp: *const usize) {
    if sp.is_null() {
        return;
    }
    unsafe {
        let argc = sp.read_volatile();
        let mut cursor = sp.add(argc.wrapping_add(2));
        while cursor.read_volatile() != 0 {
            cursor = cursor.add(1);
        }
        let mut auxv_ptr = cursor.add(1);

        let mut at_phdr = 0usize;
        let mut at_phent = 0usize;
        let mut at_phnum = 0usize;
        let mut at_base = 0usize;
        loop {
            let a_type = auxv_ptr.read_volatile();
            let a_val = auxv_ptr.add(1).read_volatile();
            if a_type == AT_NULL {
                break;
            }
            if a_type == AT_PHDR {
                at_phdr = a_val;
            } else if a_type == AT_PHENT {
                at_phent = a_val;
            } else if a_type == AT_PHNUM {
                at_phnum = a_val;
            } else if a_type == AT_BASE {
                at_base = a_val;
            }
            auxv_ptr = auxv_ptr.add(2);
        }

        // If an ELF interpreter (`PT_INTERP` / `/lib/ld-exo.so`) ran, `AT_BASE != 0`
        // and the dynamic loader has already applied all relocations.
        if at_base != 0 || at_phdr == 0 || at_phent < core::mem::size_of::<Elf64Phdr>() || at_phnum == 0 {
            return;
        }

        let mut has_pt_phdr = false;
        let mut pt_phdr_vaddr = 0usize;
        let mut has_pt_dyn = false;
        let mut pt_dyn_vaddr = 0usize;
        let mut i = 0usize;
        while i < at_phnum {
            let ph = (at_phdr.wrapping_add(i.wrapping_mul(at_phent)) as *const Elf64Phdr).read_unaligned();
            if ph.p_type == PT_PHDR {
                has_pt_phdr = true;
                pt_phdr_vaddr = ph.p_vaddr as usize;
            } else if ph.p_type == PT_DYNAMIC {
                has_pt_dyn = true;
                pt_dyn_vaddr = ph.p_vaddr as usize;
            }
            i = i.wrapping_add(1);
        }

        if !has_pt_phdr || !has_pt_dyn {
            return;
        }
        let load_bias = at_phdr.wrapping_sub(pt_phdr_vaddr);
        if load_bias == 0 {
            return;
        }

        let dyn_ptr = load_bias.wrapping_add(pt_dyn_vaddr) as *const Elf64Dyn;
        let mut rela_vaddr = 0usize;
        let mut rela_sz = 0usize;
        let mut rela_ent = core::mem::size_of::<Elf64Rela>();
        let mut d_idx = 0usize;
        while d_idx < 512 {
            let d = dyn_ptr.add(d_idx).read_unaligned();
            if d.d_tag == DT_NULL {
                break;
            } else if d.d_tag == DT_RELA {
                rela_vaddr = d.d_val as usize;
            } else if d.d_tag == DT_RELASZ {
                rela_sz = d.d_val as usize;
            } else if d.d_tag == DT_RELAENT && (d.d_val as usize) >= core::mem::size_of::<Elf64Rela>() {
                rela_ent = d.d_val as usize;
            }
            d_idx = d_idx.wrapping_add(1);
        }

        if rela_vaddr == 0 || rela_sz == 0 {
            return;
        }
        let rela_base = load_bias.wrapping_add(rela_vaddr);
        let mut off = 0usize;
        while off.wrapping_add(rela_ent) <= rela_sz {
            let rela = (rela_base.wrapping_add(off) as *const Elf64Rela).read_unaligned();
            if (rela.r_info & 0xFFFF_FFFF) == R_X86_64_RELATIVE {
                let target = load_bias.wrapping_add(rela.r_offset as usize) as *mut usize;
                let value = load_bias.wrapping_add(rela.r_addend as isize as usize);
                target.write_volatile(value);
            }
            off = off.wrapping_add(rela_ent);
        }
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
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
                AT_PHDR => {
                    SAVED_PHDR.store(a_val, Ordering::Release);
                }
                AT_PHENT => {
                    SAVED_PHENT.store(a_val, Ordering::Release);
                }
                AT_PHNUM => {
                    SAVED_PHNUM.store(a_val, Ordering::Release);
                }
                AT_PAGESZ => {
                    if a_val != 0 {
                        SAVED_PAGESZ.store(a_val, Ordering::Release);
                    }
                }
                AT_BASE => {
                    if a_val != 0 {
                        SAVED_AT_BASE.store(a_val, Ordering::Release);
                    }
                }
                AT_RANDOM => {
                    AT_RANDOM_ADDR.store(a_val, Ordering::Release);
                }
                AT_EXECFN => {
                    if a_val != 0 {
                        SAVED_EXECFN.store(a_val, Ordering::Release);
                    }
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

        let interp_base = SAVED_AT_BASE.load(Ordering::Acquire);
        if interp_base != 0 {
            let alloc_fn = unsafe { lookup_interp_sym(interp_base, b"__exo_dl_alloc_thread_tls") };
            let free_fn = unsafe { lookup_interp_sym(interp_base, b"__exo_dl_free_thread_tls") };
            if alloc_fn != 0 {
                register_dynamic_tls_hooks(alloc_fn, free_fn);
            }
        }

        let tcb_ptr = ExoTcb::current_ptr();
        if !tcb_ptr.is_null() {
            let sig_addr = GLOBAL_SIGNAL_TCB_ADDR.load(Ordering::Acquire);
            if sig_addr != 0 {
                (*tcb_ptr).os_specific = sig_addr as *mut SignalTcb;
            }
        }
        sync_thread_tcb_tid();

        (argc, argv, envp)
    }
}
