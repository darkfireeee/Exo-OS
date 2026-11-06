//! process/auxv.rs — Auxiliary vector (auxv) for do_exec()
//!
//! ## Spec reference: ExoFS_Syscall_Analyse.md §8 – BUG-04 / TLS execution
//!
//! The auxv vector is a list of pairs (type: u64, value: u64) placed
//! on the userspace stack during `execve()`, after argv and envp.
//!
//! ## AT_* constants
//! All standard Linux AT_* constants plus Exo-OS extensions.
//!
//! ## push_auxv function
//! Builds the auxv vector on the userspace stack and returns the new
//! stack pointer after writing.
//!
//! ## Rule BUG-04
//! AT_SIGNAL_TCB (51) must be included so exo-rt can locate the
//! SignalTcb without a fixed address (SIG-18: never a fixed address, ASLR).

use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Standard Linux AT_* constants
// ─────────────────────────────────────────────────────────────────────────────

/// End of auxv vector.
pub const AT_NULL: u64 = 0;
/// Ignored.
pub const AT_IGNORE: u64 = 1;
/// File descriptor of executable file.
pub const AT_EXECFD: u64 = 2;
/// Address of ELF program headers.
pub const AT_PHDR: u64 = 3;
/// Size of a program header entry.
pub const AT_PHENT: u64 = 4;
/// Number of program headers.
pub const AT_PHNUM: u64 = 5;
/// System page size.
pub const AT_PAGESZ: u64 = 6;
/// Base address of ELF interpreter.
pub const AT_BASE: u64 = 7;
/// Program flags.
pub const AT_FLAGS: u64 = 8;
/// Program entry point.
pub const AT_ENTRY: u64 = 9;
/// Non-zero if the program is NOT the ELF interpreter.
pub const AT_NOTELF: u64 = 10;
/// Real UID.
pub const AT_UID: u64 = 11;
/// Effective UID.
pub const AT_EUID: u64 = 12;
/// Real GID.
pub const AT_GID: u64 = 13;
/// Effective GID.
pub const AT_EGID: u64 = 14;
/// Platform identifying string.
pub const AT_PLATFORM: u64 = 15;
/// HWCAP bitmask of hardware capabilities.
pub const AT_HWCAP: u64 = 16;
/// Clock tick frequency.
pub const AT_CLKTCK: u64 = 17;
/// Virtual address of vDSO file.
pub const AT_SYSINFO: u64 = 32;
/// Address of vDSO ELF page (for fast-path clock_gettime).
pub const AT_SYSINFO_EHDR: u64 = 33;
/// Extended HWCAP2 bitmask.
pub const AT_HWCAP2: u64 = 26;
/// Pointer to 16 bytes of random data (security cookie).
pub const AT_RANDOM: u64 = 25;

// ─────────────────────────────────────────────────────────────────────────────
// Exo-OS extensions
// ─────────────────────────────────────────────────────────────────────────────

/// Virtual address of SignalTcb (SIG-18 — never fixed, ASLR).
/// Used by exo-rt to locate signal handlers.
pub const AT_SIGNAL_TCB: u64 = 51;

/// Initial process capability token (Exo-OS security layer).
/// Passed by kernel during exec() from init_server.
pub const AT_CAP_TOKEN: u64 = 52;

// ─────────────────────────────────────────────────────────────────────────────
// auxv structure
// ─────────────────────────────────────────────────────────────────────────────

/// An auxiliary vector pair (type, value).
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct AuxEntry {
    pub a_type: u64,
    pub a_val: u64,
}

impl AuxEntry {
    #[inline]
    pub const fn new(a_type: u64, a_val: u64) -> Self {
        Self { a_type, a_val }
    }
    /// Terminating entry.
    #[inline]
    pub const fn null() -> Self {
        Self {
            a_type: AT_NULL,
            a_val: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// auxv vector build parameters
// ─────────────────────────────────────────────────────────────────────────────

/// ELF information required to build the auxv vector.
pub struct AuxvParams {
    /// Address of program headers (PT_PHDR or computed from EHDR).
    pub phdr_vaddr: u64,
    /// Number of program entries (e_phnum).
    pub phnum: u64,
    /// Program entry point (e_entry, after relocation for PIE).
    pub entry_vaddr: u64,
    /// Interpreter base address (ld-linux, ld-exo).
    pub interp_base: u64,
    /// vDSO ELF page address (for fast-path syscalls).
    pub vdso_ehdr_vaddr: u64,
    /// Address of SignalTcb mapped in userspace (SIG-18).
    pub signal_tcb_vaddr: u64,
    /// Initial capability token.
    pub cap_token: u64,
    /// Process UID/GID.
    pub uid: u32,
    pub gid: u32,
    /// Pointer to 16 random bytes ALREADY on the stack.
    pub random_ptr: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// auxv vector construction
// ─────────────────────────────────────────────────────────────────────────────

/// Builds the list of auxv entries from execution parameters.
///
/// Returns a Vec<AuxEntry> to serialize onto the userspace stack.
/// OOM-02: explicit try_reserve.
#[allow(clippy::result_unit_err)]
pub fn build_auxv(params: &AuxvParams) -> Result<Vec<AuxEntry>, ()> {
    let mut v: Vec<AuxEntry> = Vec::new();
    // Maximum known count: ~16 entries + AT_NULL.
    v.try_reserve(20).map_err(|_| ())?;

    v.push(AuxEntry::new(AT_PHDR, params.phdr_vaddr));
    v.push(AuxEntry::new(AT_PHENT, 56)); // Elf64_Phdr = standard 56 bytes
    v.push(AuxEntry::new(AT_PHNUM, params.phnum));
    v.push(AuxEntry::new(AT_PAGESZ, 4096));
    v.push(AuxEntry::new(AT_BASE, params.interp_base));
    v.push(AuxEntry::new(AT_FLAGS, 0));
    v.push(AuxEntry::new(AT_ENTRY, params.entry_vaddr));
    v.push(AuxEntry::new(AT_UID, params.uid as u64));
    v.push(AuxEntry::new(AT_EUID, params.uid as u64));
    v.push(AuxEntry::new(AT_GID, params.gid as u64));
    v.push(AuxEntry::new(AT_EGID, params.gid as u64));
    v.push(AuxEntry::new(AT_RANDOM, params.random_ptr));
    v.push(AuxEntry::new(AT_CLKTCK, 100));
    v.push(AuxEntry::new(AT_HWCAP, 0));

    // vDSO: present only if the page address is non-zero.
    if params.vdso_ehdr_vaddr != 0 {
        v.push(AuxEntry::new(AT_SYSINFO_EHDR, params.vdso_ehdr_vaddr));
    }

    // Exo-OS extensions
    if params.signal_tcb_vaddr != 0 {
        v.push(AuxEntry::new(AT_SIGNAL_TCB, params.signal_tcb_vaddr));
    }
    if params.cap_token != 0 {
        v.push(AuxEntry::new(AT_CAP_TOKEN, params.cap_token));
    }

    // Mandatory termination
    v.push(AuxEntry::null());
    Ok(v)
}

/// Serializes the auxv vector into bytes (for copy_to_user onto the stack).
///
/// RECUR-01: while, no for.
#[allow(clippy::result_unit_err)]
pub fn serialize_auxv(entries: &[AuxEntry]) -> Result<Vec<u8>, ()> {
    let byte_count = entries
        .len()
        .checked_mul(core::mem::size_of::<AuxEntry>())
        .ok_or(())?;
    let mut buf = Vec::new();
    buf.try_reserve(byte_count).map_err(|_| ())?;
    let mut i = 0usize;
    while i < entries.len() {
        let e = &entries[i];
        // SAFETY: AuxEntry is #[repr(C)] — stable binary layout.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                e as *const AuxEntry as *const u8,
                core::mem::size_of::<AuxEntry>(),
            )
        };
        let mut j = 0usize;
        while j < bytes.len() {
            buf.push(bytes[j]);
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// push_auxv: entry point for do_exec()
// ─────────────────────────────────────────────────────────────────────────────

/// Writes the auxv vector onto the userspace stack and returns the resulting SP.
///
/// `stack_top`: pointer to top of stack BEFORE writing auxv.
/// Returns the new SP (stack_top - auxv_size), aligned to 8 bytes.
///
/// ## Usage in do_exec()
/// ```ignore
/// let new_sp = push_auxv(user_stack_top, &params)?;
/// // Then write environ/argv above new_sp.
/// ```
#[allow(clippy::result_unit_err)]
pub fn push_auxv(stack_top: u64, params: &AuxvParams) -> Result<u64, ()> {
    let entries = build_auxv(params)?;
    let bytes = serialize_auxv(&entries)?;
    let size = bytes.len() as u64;
    // Align downwards to 8 bytes.
    let sp = (stack_top.saturating_sub(size)) & !7u64;
    // Write via copy_to_user (validates pointer before writing).
    match crate::syscall::validation::copy_to_user(sp as *mut u8, bytes.as_ptr(), bytes.len()) {
        Ok(_) => Ok(sp),
        Err(_) => Err(()),
    }
}
