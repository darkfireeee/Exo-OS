//! # syscall/validation.rs — Userspace argument validation
//!
//! Provides validation primitives and secure memory copying
//! between user space and the kernel.
//!
//! ## Security Model
//!
//! Any data coming from Ring 3 is **untrusted** until validated.
//! The rules are:
//! 1. Non-NULL pointer
//! 2. Within canonical userspace range (< [`USER_ADDR_MAX`])
//! 3. Entire range [ptr, ptr+len) must be in user space
//! 4. No arithmetic overflow on `ptr + len`
//! 5. Proper alignment for non-packed types
//!
//! ## copy_from_user / copy_to_user
//! The functions never directly dereference the Ring 3 address in
//! kernel context. On the kernel target, they translate the user address
//! via the current address space, trigger if needed the same
//! demand-paging/CoW path as a userspace #PF, then copy via the kernel physmap.
//!
//! ## UNSAFE CONTRACT RULE (regle_bonus.md)
//! Every `unsafe {}` block is preceded by a `// SAFETY:` comment.

use core::fmt;
use core::mem;
use core::sync::atomic::{AtomicU64, Ordering};

use alloc::vec::Vec;

use super::numbers::{E2BIG, EFAULT, EINVAL, ENOMEM};

// ─────────────────────────────────────────────────────────────────────────────
// Address range constants
// ─────────────────────────────────────────────────────────────────────────────

/// Beginning of kernel in virtual address space (x86_64 higher-half canonical)
/// Any address ≥ USER_ADDR_MAX belongs to the kernel.
pub const USER_ADDR_MAX: u64 = 0x0000_8000_0000_0000;

/// Maximum length of a file path
pub const PATH_MAX: usize = 4096;

/// Maximum length of a generic string passed in a syscall
pub const STRING_MAX: usize = 65536;

/// Maximum size of an I/O buffer in a single syscall (256 MiB)
pub const IO_BUF_MAX: usize = 256 * 1024 * 1024;

/// Maximum number of argv/envp parameters in execve
pub const ARGV_MAX: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation counters
// ─────────────────────────────────────────────────────────────────────────────

static VALIDATION_COUNT: AtomicU64 = AtomicU64::new(0);
static VALIDATION_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
static COPY_FROM_USER_BYTES: AtomicU64 = AtomicU64::new(0);
static COPY_TO_USER_BYTES: AtomicU64 = AtomicU64::new(0);

/// Returns total number of validations performed
#[inline]
pub fn validation_count() -> u64 {
    VALIDATION_COUNT.load(Ordering::Relaxed)
}
/// Returns number of validation faults (EFAULT raised)
#[inline]
pub fn validation_fault_count() -> u64 {
    VALIDATION_FAULT_COUNT.load(Ordering::Relaxed)
}
/// Returns total bytes copied from userspace
#[inline]
pub fn copy_from_user_bytes_total() -> u64 {
    COPY_FROM_USER_BYTES.load(Ordering::Relaxed)
}
/// Returns total bytes copied to userspace
#[inline]
pub fn copy_to_user_bytes_total() -> u64 {
    COPY_TO_USER_BYTES.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// UserPtr<T> — typed pointer from user space
// ─────────────────────────────────────────────────────────────────────────────

/// Type wrapper around a raw pointer coming from Ring 3.
///
/// Cannot be dereferenced directly. Must be validated via
/// [`UserPtr::validate`] before any access, which returns a
/// [`ValidatedUserPtr<T>`].
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct UserPtr<T> {
    addr: u64,
    _phantom: core::marker::PhantomData<*mut T>,
}

// SAFETY: UserPtr is merely an integer encoding a Ring 3 address.
// It is not directly dereferenced, so Send+Sync are safe.
unsafe impl<T> Send for UserPtr<T> {}
unsafe impl<T> Sync for UserPtr<T> {}

impl<T> UserPtr<T> {
    /// Constructs a `UserPtr` from a raw address (from a syscall register).
    #[inline]
    pub fn from_raw(addr: u64) -> Self {
        Self {
            addr,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Returns the raw address without validation.
    #[inline]
    pub fn as_raw(&self) -> u64 {
        self.addr
    }

    /// Returns `true` if the pointer is null.
    #[inline]
    pub fn is_null(&self) -> bool {
        self.addr == 0
    }

    /// Validates the pointer and returns a [`ValidatedUserPtr<T>`].
    ///
    /// Checks:
    /// - Non-NULL
    /// - Within canonical userspace range
    /// - Aligned to `align_of::<T>()`
    /// - Range `[addr, addr + size_of::<T>())` does not exceed `USER_ADDR_MAX`
    pub fn validate(&self) -> Result<ValidatedUserPtr<T>, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        validate_user_range(self.addr, mem::size_of::<T>(), mem::align_of::<T>())?;
        Ok(ValidatedUserPtr {
            addr: self.addr,
            _phantom: core::marker::PhantomData,
        })
    }

    /// Validates a nullable pointer (`NULL` is accepted → returns `None`).
    pub fn validate_nullable(&self) -> Result<Option<ValidatedUserPtr<T>>, SyscallError> {
        if self.addr == 0 {
            return Ok(None);
        }
        self.validate().map(Some)
    }
}

impl<T> fmt::Debug for UserPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserPtr(0x{:016x})", self.addr)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ValidatedUserPtr<T> — validated pointer, dereferenceable under unsafe
// ─────────────────────────────────────────────────────────────────────────────

/// User pointer whose range and alignment have been verified.
/// Can be copied via `copy_from_user` / `copy_to_user`.
pub struct ValidatedUserPtr<T> {
    addr: u64,
    _phantom: core::marker::PhantomData<*mut T>,
}

impl<T: Copy> ValidatedUserPtr<T> {
    /// Copies value T from user space.
    ///
    /// Uses volatile reads to prevent compiler optimizations.
    ///
    /// # Errors
    /// Returns [`SyscallError::Fault`] if a page fault occurs
    /// (in a full kernel, handled via fixup table).
    pub fn read(&self) -> Result<T, SyscallError> {
        let mut value = mem::MaybeUninit::<T>::uninit();
        copy_from_user(
            value.as_mut_ptr() as *mut u8,
            self.addr as *const u8,
            mem::size_of::<T>(),
        )?;
        // SAFETY: copy_from_user populated all bytes of value if Ok.
        Ok(unsafe { value.assume_init() })
    }

    /// Writes value T to user space.
    pub fn write(&self, value: T) -> Result<(), SyscallError> {
        copy_to_user(
            self.addr as *mut u8,
            &value as *const T as *const u8,
            mem::size_of::<T>(),
        )
    }
}

impl<T> ValidatedUserPtr<T> {
    /// Returns the validated address.
    #[inline]
    pub fn as_raw(&self) -> u64 {
        self.addr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UserBuf — validated userspace byte slice
// ─────────────────────────────────────────────────────────────────────────────

/// Represents a fully validated user buffer (address + length).
pub struct UserBuf {
    addr: u64,
    len: usize,
}

impl UserBuf {
    /// Validates a buffer (address + length) in user space.
    ///
    /// - `ptr`: starting address (from a syscall register)
    /// - `len`: size in bytes
    /// - `max`: maximum allowed upper bound (`IO_BUF_MAX`, `PATH_MAX`, etc.)
    pub fn validate(ptr: u64, len: usize, max: usize) -> Result<Self, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        if len > max {
            record_fault();
            return Err(SyscallError::TooBig);
        }
        validate_user_range(ptr, len, 1)?;
        Ok(Self { addr: ptr, len })
    }

    /// Copies the user buffer content into `dst`.
    ///
    /// `dst` must have length exactly equal to `self.len`.
    pub fn read_into(&self, dst: &mut [u8]) -> Result<(), SyscallError> {
        if dst.len() != self.len {
            return Err(SyscallError::Invalid);
        }
        copy_from_user(dst.as_mut_ptr(), self.addr as *const u8, self.len)
    }

    /// Copies `src` into the user buffer.
    pub fn write_from(&self, src: &[u8]) -> Result<(), SyscallError> {
        if src.len() > self.len {
            return Err(SyscallError::TooBig);
        }
        copy_to_user(self.addr as *mut u8, src.as_ptr(), src.len())
    }

    /// Returns the validated length.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }
    /// Returns true if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Returns the validated address.
    #[inline]
    pub fn addr(&self) -> u64 {
        self.addr
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UserStr — null-terminated C string from userspace
// ─────────────────────────────────────────────────────────────────────────────

/// Represents a validated null-terminated C string from user space.
///
/// Content is copied into an internal kernel buffer during validation.
pub struct UserStr {
    /// Kernel buffer containing copied UTF-8 string (without '\0').
    ///
    /// Heap storage: paths/argv/envp must never materialize a
    /// PATH_MAX/STRING_MAX buffer in a syscall frame.
    buf: Vec<u8>,
}

impl UserStr {
    /// Reads and validates a null-terminated C string from userspace.
    ///
    /// - `ptr`: string address (from a syscall register)
    /// - `max`: maximum tolerated length (excluding '\0')
    ///
    /// # Errors
    /// - [`SyscallError::Fault`]   if `ptr` is invalid
    /// - [`SyscallError::TooBig`]  if no '\0' before `max` bytes
    /// - [`SyscallError::Invalid`] if string contains invalid bytes
    pub fn from_user(ptr: u64, max: usize) -> Result<Self, SyscallError> {
        VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);
        if ptr == 0 || ptr >= USER_ADDR_MAX {
            record_fault();
            return Err(SyscallError::Fault);
        }
        let capped_max = max.min(STRING_MAX);
        let mut buf = Vec::with_capacity(64);

        #[cfg(target_os = "none")]
        {
            use crate::memory::core::{phys_to_virt, PAGE_SIZE};
            let user_as = current_user_address_space().ok_or_else(|| {
                record_fault();
                SyscallError::Fault
            })?;
            let mut offset = 0usize;
            while offset < capped_max {
                let user_addr = ptr.checked_add(offset as u64).ok_or(SyscallError::Fault)?;
                if user_addr >= USER_ADDR_MAX {
                    record_fault();
                    return Err(SyscallError::Fault);
                }
                let page_off = (user_addr as usize) & (PAGE_SIZE - 1);
                let chunk = (PAGE_SIZE - page_off).min(capped_max - offset);
                let phys = resolve_user_page(user_as, user_addr, false)?;
                let kernel_slice = unsafe {
                    core::slice::from_raw_parts(phys_to_virt(phys).as_u64() as *const u8, chunk)
                };
                if let Some(nul_pos) = kernel_slice.iter().position(|&b| b == 0) {
                    buf.extend_from_slice(&kernel_slice[..nul_pos]);
                    COPY_FROM_USER_BYTES.fetch_add((offset + nul_pos + 1) as u64, Ordering::Relaxed);
                    return Ok(Self { buf });
                }
                buf.extend_from_slice(kernel_slice);
                offset += chunk;
            }
            record_fault();
            return Err(SyscallError::TooBig);
        }
        #[cfg(not(target_os = "none"))]
        {
            // Keep host loop for std unit tests
            let mut offset = 0usize;
            loop {
                if offset >= capped_max {
                    return Err(SyscallError::TooBig);
                }
                let byte = unsafe { core::ptr::read_volatile((ptr as *const u8).add(offset)) };
                if byte == 0 {
                    break;
                }
                buf.push(byte);
                offset += 1;
            }
            Ok(Self { buf })
        }
    }

    /// Returns the string as a byte slice (without trailing null).
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Returns the length excluding trailing null.
    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns true if the string is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Attempts to convert to &str (strict UTF-8).
    pub fn as_str(&self) -> Result<&str, SyscallError> {
        core::str::from_utf8(&self.buf).map_err(|_| SyscallError::Invalid)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SyscallError — validation errors propagated to dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Errors returned by validation functions.
/// Converted to Linux errno codes by [`SyscallError::to_errno`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SyscallError {
    /// Invalid user address (NULL, out of range, or unmapped)
    Fault,
    /// Invalid numeric argument (out-of-range value, unknown flag, etc.)
    Invalid,
    /// Size or length exceeds allowed limit
    TooBig,
    /// Permission denied (missing capability or permission)
    Access,
    /// Resource not found
    NotFound,
    /// Resource busy or lock unavailable
    Busy,
    /// Interrupted by signal
    Interrupted,
    /// Out of memory during kernel copy/validation
    NoMemory,
    /// Operation not supported
    NotSupported,
}

impl SyscallError {
    /// Converts to Linux errno code (negative value).
    #[inline]
    pub const fn to_errno(self) -> i64 {
        match self {
            SyscallError::Fault => EFAULT,
            SyscallError::Invalid => EINVAL,
            SyscallError::TooBig => E2BIG,
            SyscallError::Access => -13,     // EACCES
            SyscallError::NotFound => -2,    // ENOENT
            SyscallError::Busy => -16,       // EBUSY
            SyscallError::Interrupted => -4, // EINTR
            SyscallError::NoMemory => ENOMEM,
            SyscallError::NotSupported => -38, // ENOSYS
        }
    }
}

impl fmt::Display for SyscallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SyscallError::Fault => "bad address (EFAULT)",
            SyscallError::Invalid => "invalid argument (EINVAL)",
            SyscallError::TooBig => "argument too large (E2BIG)",
            SyscallError::Access => "permission denied (EACCES)",
            SyscallError::NotFound => "not found (ENOENT)",
            SyscallError::Busy => "resource busy (EBUSY)",
            SyscallError::Interrupted => "interrupted (EINTR)",
            SyscallError::NoMemory => "out of memory (ENOMEM)",
            SyscallError::NotSupported => "not supported (ENOSYS)",
        };
        f.write_str(s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal validation primitives
// ─────────────────────────────────────────────────────────────────────────────

/// Verifies that a range [addr, addr+len) is entirely in userspace.
///
/// Checked conditions:
/// 1. `addr` non-NULL (unless `len == 0`)
/// 2. `addr < USER_ADDR_MAX`
/// 3. `addr + len ≤ USER_ADDR_MAX` (no overflow, no kernel overlap)
/// 4. Alignment to `align` (must be a power of 2)
#[inline]
fn validate_user_range(addr: u64, len: usize, align: usize) -> Result<(), SyscallError> {
    // Special case for zero length: only NULL rejected for POSIX consistency
    if len == 0 {
        return Ok(());
    }
    // Null pointer
    if addr == 0 {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Start inside kernel
    if addr >= USER_ADDR_MAX {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Anti-wrap check: addr + len must not overflow u64 nor exceed USER_ADDR_MAX
    let end = addr.checked_add(len as u64).ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    if end > USER_ADDR_MAX {
        record_fault();
        return Err(SyscallError::Fault);
    }
    // Alignment check (align must be power of 2)
    debug_assert!(
        align.is_power_of_two(),
        "align doit être une puissance de 2"
    );
    if align > 1 && (addr as usize) % align != 0 {
        record_fault();
        return Err(SyscallError::Fault);
    }
    Ok(())
}

#[cfg(target_os = "none")]
fn current_user_address_space() -> Option<&'static crate::memory::virt::UserAddressSpace> {
    let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb_raw == 0 {
        return None;
    }

    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = crate::process::core::pid::Pid(tcb.pid.0);
    let pcb = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)?;
    let as_ptr = pcb.address_space_ptr();
    if as_ptr.is_null() {
        return None;
    }

    // SAFETY: address space belongs to living PCB of current thread.
    Some(unsafe { &*(as_ptr as *const crate::memory::virt::UserAddressSpace) })
}

#[cfg(target_os = "none")]
fn walk_user_mapping(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
) -> Option<(
    crate::memory::core::PhysAddr,
    crate::memory::virt::page_table::PageTableEntry,
)> {
    use crate::memory::core::{PhysAddr, VirtAddr, PAGE_SIZE};
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

    let virt = VirtAddr::new(addr);
    let walker = PageTableWalker::new(user_as.pml4_phys());
    match walker.walk_read(virt) {
        WalkResult::Leaf { entry, .. } => {
            let off = addr & (PAGE_SIZE as u64 - 1);
            Some((PhysAddr::new(entry.phys_addr().as_u64() + off), entry))
        }
        WalkResult::HugePage { entry, level } => {
            let page_size = level.page_size() as u64;
            let off = addr & (page_size - 1);
            Some((PhysAddr::new(entry.phys_addr().as_u64() + off), entry))
        }
        _ => None,
    }
}

#[cfg(target_os = "none")]
fn resolve_user_page(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Result<crate::memory::core::PhysAddr, SyscallError> {
    use crate::arch::x86_64::memory_iface::UserFaultAllocator;
    use crate::memory::core::{VirtAddr, PAGE_SIZE};
    use crate::memory::virt::fault::{handle_page_fault, FaultCause, FaultContext, FaultResult};

    if let Some((phys, entry)) = walk_user_mapping(user_as, addr) {
        if !entry.is_user() {
            record_fault();
            return Err(SyscallError::Fault);
        }
        if !write || (entry.is_writable() && !entry.is_cow()) {
            return Ok(phys);
        }
    }

    let page_addr = VirtAddr::new(addr & !(PAGE_SIZE as u64 - 1));
    let vma = match user_as.find_vma(page_addr) {
        Some(vma) => vma,
        None => {
            record_fault();
            return Err(SyscallError::Fault);
        }
    };
    let cause = if write {
        FaultCause::Write
    } else {
        FaultCause::Read
    };
    let ctx = FaultContext::new(page_addr, cause, false).with_vma(vma);
    let alloc = UserFaultAllocator::new(user_as);
    match handle_page_fault(&ctx, &alloc) {
        FaultResult::Handled => {}
        _ => {
            record_fault();
            return Err(SyscallError::Fault);
        }
    }

    if let Some((phys, entry)) = walk_user_mapping(user_as, addr) {
        if entry.is_user() && (!write || (entry.is_writable() && !entry.is_cow())) {
            return Ok(phys);
        }
    }

    record_fault();
    Err(SyscallError::Fault)
}

#[cfg(target_os = "none")]
fn copy_from_user_resolved(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    use crate::memory::core::{phys_to_virt, PAGE_SIZE};

    let user_as = current_user_address_space().ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    let mut copied = 0usize;
    while copied < len {
        let user_addr = (src as u64).saturating_add(copied as u64);
        let page_off = (user_addr as usize) & (PAGE_SIZE - 1);
        let chunk = core::cmp::min(PAGE_SIZE - page_off, len - copied);
        let phys = resolve_user_page(user_as, user_addr, false)?;
        let kernel_src = phys_to_virt(phys).as_u64() as *const u8;
        // SAFETY: resolve_user_page guarantees source is physmap alias
        // of current user page; dst covers len kernel bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(kernel_src, dst.add(copied), chunk);
        }
        copied += chunk;
    }
    Ok(())
}

#[cfg(target_os = "none")]
fn copy_to_user_resolved(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    use crate::memory::core::{phys_to_virt, PAGE_SIZE};

    let user_as = current_user_address_space().ok_or_else(|| {
        record_fault();
        SyscallError::Fault
    })?;
    let mut copied = 0usize;
    while copied < len {
        let user_addr = (dst as u64).saturating_add(copied as u64);
        let page_off = (user_addr as usize) & (PAGE_SIZE - 1);
        let chunk = core::cmp::min(PAGE_SIZE - page_off, len - copied);
        let phys = resolve_user_page(user_as, user_addr, true)?;
        let kernel_dst = phys_to_virt(phys).as_u64() as *mut u8;
        // SAFETY: resolve_user_page validated/resolved a writable userspace page
        // and returns its kernel physmap alias; src covers len kernel bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(copied), kernel_dst, chunk);
        }
        copied += chunk;
    }
    Ok(())
}

/// Copies `len` bytes from `src` (userspace) to `dst` (kernel).
///
/// # Safety
/// `dst` must point to a valid kernel buffer of length >= `len`.
pub fn copy_from_user(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    if len == 0 {
        return Ok(());
    }
    validate_user_range(src as u64, len, 1)?;
    #[cfg(target_os = "none")]
    {
        copy_from_user_resolved(dst, src, len)?;
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        for i in 0..len {
            let byte = core::ptr::read_volatile(src.add(i));
            core::ptr::write(dst.add(i), byte);
        }
    }
    COPY_FROM_USER_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    Ok(())
}

/// Copies `len` bytes from `src` (kernel) to `dst` (userspace).
///
/// # Safety
/// `src` must point to `len` bytes of valid kernel data.
pub fn copy_to_user(dst: *mut u8, src: *const u8, len: usize) -> Result<(), SyscallError> {
    if len == 0 {
        return Ok(());
    }
    validate_user_range(dst as u64, len, 1)?;
    #[cfg(target_os = "none")]
    {
        copy_to_user_resolved(dst, src, len)?;
    }
    #[cfg(not(target_os = "none"))]
    unsafe {
        for i in 0..len {
            let byte = core::ptr::read(src.add(i));
            core::ptr::write_volatile(dst.add(i), byte);
        }
    }
    COPY_TO_USER_BYTES.fetch_add(len as u64, Ordering::Relaxed);
    Ok(())
}

/// Internal increment of validation fault counter.
#[inline(always)]
fn record_fault() {
    VALIDATION_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// High-level public API — direct helpers for syscall handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Validates and reads a `T` from a userspace pointer.
///
/// Combines `UserPtr::validate()` and `ValidatedUserPtr::read()` into a single
/// operation. Returns errno on failure.
///
/// # Example
/// ```no_run
/// let timespec: Timespec = read_user_typed::<Timespec>(frame.rsi)?;
/// ```
pub fn read_user_typed<T: Copy>(ptr_raw: u64) -> Result<T, SyscallError> {
    UserPtr::<T>::from_raw(ptr_raw).validate()?.read()
}

/// Validates and writes a `T` to a userspace pointer.
pub fn write_user_typed<T: Copy>(ptr_raw: u64, value: T) -> Result<(), SyscallError> {
    UserPtr::<T>::from_raw(ptr_raw).validate()?.write(value)
}

/// Returns the PID of the currently scheduled userspace thread, if any.
pub fn current_user_pid() -> Option<u32> {
    #[cfg(target_os = "none")]
    {
        let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
        if tcb_raw == 0 {
            return None;
        }
        let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
        Some(tcb.pid.0)
    }
    #[cfg(not(target_os = "none"))]
    {
        None
    }
}

/// Reads a 4-byte-aligned `u32` from userspace and returns `(value, physical_addr)`.
pub fn resolve_user_read_u32_and_phys(uaddr: u64) -> Result<(u32, u64), SyscallError> {
    validate_user_range(uaddr, core::mem::size_of::<u32>(), core::mem::align_of::<u32>())?;
    #[cfg(target_os = "none")]
    {
        let user_as = current_user_address_space().ok_or_else(|| {
            record_fault();
            SyscallError::Fault
        })?;
        let phys = resolve_user_page(user_as, uaddr, false)?;
        let kernel_src = crate::memory::core::phys_to_virt(phys).as_u64() as *const u32;
        // SAFETY: `uaddr` is 4-byte aligned (verified above, so within a single 4K page)
        // and `resolve_user_page` resolved its present user mapping into the physmap alias.
        let val = unsafe { core::ptr::read_volatile(kernel_src) };
        COPY_FROM_USER_BYTES.fetch_add(4, Ordering::Relaxed);
        Ok((val, phys.as_u64()))
    }
    #[cfg(not(target_os = "none"))]
    {
        let val = read_user_typed::<u32>(uaddr)?;
        Ok((val, uaddr))
    }
}

/// Validates a buffer `(ptr, len)` and copies data into a kernel `Vec`.
///
/// `max` is the allowed size limit (e.g. `IO_BUF_MAX` for `read()`).
pub fn read_user_buf_to_vec(
    ptr: u64,
    len: usize,
    max: usize,
) -> Result<alloc::vec::Vec<u8>, SyscallError> {
    let buf = UserBuf::validate(ptr, len, max)?;
    let mut vec = Vec::new();
    vec.try_reserve_exact(len)
        .map_err(|_| SyscallError::NoMemory)?;
    vec.resize(len, 0);
    buf.read_into(&mut vec)?;
    Ok(vec)
}

/// Validates a null-terminated C file path from userspace.
///
/// Maximum length: [`PATH_MAX`] (4096 bytes).
pub fn read_user_path(ptr: u64) -> Result<UserStr, SyscallError> {
    UserStr::from_user(ptr, PATH_MAX)
}

/// Validates an integer argument as a file descriptor (≥0 and <65536).
#[inline]
pub fn validate_fd(raw: u64) -> Result<i32, SyscallError> {
    if raw > 65535 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as i32)
}

/// Validates a set of flags, verifying no unsupported bits are set.
#[inline]
pub fn validate_flags(raw: u64, allowed_mask: u64) -> Result<u64, SyscallError> {
    if raw & !allowed_mask != 0 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw)
}

/// Validates a PID (must be > 0 and < 4194304).
#[inline]
pub fn validate_pid(raw: u64) -> Result<u32, SyscallError> {
    if raw == 0 || raw >= 4194304 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

/// Validates a deliverable signal (1..=63, POSIX + real-time).
///
/// `kill(pid, 0)` is an existence check: it is handled by the `kill`
/// handler before this call, because signal 0 is never deliverable.
#[inline]
pub fn validate_signal(raw: u64) -> Result<u32, SyscallError> {
    if raw == 0 || raw > crate::process::signal::delivery::MAX_SIGNAL_NUMBER as u64 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

/// Validates a CLOCK_ID (standard POSIX values).
#[inline]
pub fn validate_clockid(raw: u64) -> Result<u32, SyscallError> {
    // POSIX clock IDs : 0=REALTIME, 1=MONOTONIC, 2=PROCESS_CPUTIME_ID,
    // 3=THREAD_CPUTIME_ID, 4=MONOTONIC_RAW, 5=REALTIME_COARSE, 6=MONOTONIC_COARSE,
    // 7=BOOTTIME
    if raw > 7 {
        return Err(SyscallError::Invalid);
    }
    Ok(raw as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests (cfg(test) → never compiled into kernel binary)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_null_ptr() {
        let result = validate_user_range(0, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_kernel_ptr() {
        let kernel_addr = 0xFFFF_8000_0000_0000u64;
        let result = validate_user_range(kernel_addr, 8, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_valid_range() {
        // Valid canonical userspace address
        let result = validate_user_range(0x0000_0000_0040_0000, 4096, 1);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_wrap_overflow() {
        // addr + len overflows u64
        let result = validate_user_range(u64::MAX - 4, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_end_past_usermax() {
        // valid addr but extends into kernel
        let result = validate_user_range(USER_ADDR_MAX - 8, 16, 1);
        assert!(matches!(result, Err(SyscallError::Fault)));
    }

    #[test]
    fn test_validate_fd() {
        assert!(validate_fd(0).is_ok());
        assert!(validate_fd(65534).is_ok());
        assert!(matches!(validate_fd(65536), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_validate_pid() {
        assert!(matches!(validate_pid(0), Err(SyscallError::Invalid)));
        assert!(validate_pid(1).is_ok());
        assert!(validate_pid(4194303).is_ok());
        assert!(matches!(validate_pid(4194304), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_validate_signal() {
        assert!(matches!(validate_signal(0), Err(SyscallError::Invalid)));
        assert!(validate_signal(1).is_ok());
        assert!(validate_signal(63).is_ok());
        assert!(matches!(validate_signal(64), Err(SyscallError::Invalid)));
    }

    #[test]
    fn test_syscall_error_to_errno() {
        assert_eq!(SyscallError::Fault.to_errno(), EFAULT);
        assert_eq!(SyscallError::Invalid.to_errno(), EINVAL);
        assert_eq!(SyscallError::TooBig.to_errno(), E2BIG);
    }
}
