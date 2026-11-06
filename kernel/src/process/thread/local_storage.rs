// kernel/src/process/thread/local_storage.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TLS (Thread Local Storage) — FS segment management for x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// x86_64 Architecture:
//   The FS segment is used for user TLS data.
//   FS.base (MSR_FS_BASE = 0xC0000100) = static TLS block address.
//   SWAPGS only touches GS: the scheduler preserves user_gs_base separately.
//
// Simplified implementation:
//   • TlsBlock: copy of .tdata/.tbss initialized by execve() then copied by clone().
//   • set_fs_base(): writes ARCH_SET_FS via wrmsrl(MSR_FS_BASE, addr).
//   • Dynamic TLS (dlopen / __tls_get_addr): outside kernel scope.
// ═══════════════════════════════════════════════════════════════════════════════

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

/// Maximum size of a static TLS block (64 KiB).
pub const TLS_MAX_SIZE: usize = 65536;

/// Dynamic TLS key (pthread_key_t).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct TlsKey(pub u32);

impl TlsKey {
    pub const INVALID: Self = Self(u32::MAX);
    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

/// Static TLS block of a thread.
///
/// Contains the copy of .tdata initially loaded by execve(),
/// followed by .tbss (zero-initialized).
pub struct TlsBlock {
    /// TLS data (tdata + tbss).
    data: Box<[u8]>,
    /// Size of the tdata segment.
    tdata_size: usize,
    /// Total size (tdata + tbss).
    total_size: usize,
    /// Userspace base address of the TLS block.
    user_base: u64,
}

impl TlsBlock {
    /// Creates a TLS block by copying the provided tdata template.
    pub fn new(tdata: &[u8], tbss_size: usize, user_base: u64) -> Option<Self> {
        let total = tdata.len() + tbss_size;
        if total == 0 || total > TLS_MAX_SIZE {
            return None;
        }
        let mut data = Vec::new();
        data.try_reserve_exact(total).ok()?;
        data.resize(total, 0);
        data[..tdata.len()].copy_from_slice(tdata);
        let data = data.into_boxed_slice();
        // tbss is already zero-initialized by resize().
        Some(Self {
            data,
            tdata_size: tdata.len(),
            total_size: total,
            user_base,
        })
    }

    /// Pointer to data (for writing FS.base).
    #[inline(always)]
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    /// Userspace address of this block.
    #[inline(always)]
    pub fn user_base(&self) -> u64 {
        self.user_base
    }

    /// Total size in bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.total_size
    }

    /// Clones the TLS block for a new thread (fork or pthread_create).
    pub fn clone_for_thread(&self, new_user_base: u64) -> Option<Self> {
        let mut data = Vec::new();
        data.try_reserve_exact(self.data.len()).ok()?;
        data.extend_from_slice(&self.data);
        let data = data.into_boxed_slice();
        Some(Self {
            data,
            tdata_size: self.tdata_size,
            total_size: self.total_size,
            user_base: new_user_base,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TlsRegistry — global dynamic TLS key registry
// ─────────────────────────────────────────────────────────────────────────────

const MAX_TLS_KEYS: usize = 1024;

/// Dynamic TLS key table entry.
struct TlsKeyEntry {
    used: bool,
    destructor: Option<u64>, // userspace pointer to destructor function
}

/// Global registry of dynamic TLS keys (pthread_key_create/destroy).
pub struct TlsRegistry {
    keys: UnsafeCell<[TlsKeyEntry; MAX_TLS_KEYS]>,
    n_used: AtomicU32,
    lock: crate::scheduler::sync::spinlock::SpinLock<()>,
}

// SAFETY: TlsRegistry is accessed via SpinLock.
unsafe impl Sync for TlsRegistry {}

pub static TLS_REGISTRY: TlsRegistry = TlsRegistry {
    // SAFETY: TlsKeyEntry is not Copy, initialize manually for the 1024 entries.
    // Option: use a const construction via const fn.
    keys: UnsafeCell::new(
        [const {
            TlsKeyEntry {
                used: false,
                destructor: None,
            }
        }; MAX_TLS_KEYS],
    ),
    n_used: AtomicU32::new(0),
    lock: crate::scheduler::sync::spinlock::SpinLock::new(()),
};

impl TlsRegistry {
    /// Allocates a new dynamic TLS key (pthread_key_create).
    pub fn alloc_key(&self, destructor: Option<u64>) -> Option<TlsKey> {
        let _g = self.lock.lock();
        // SAFETY: access under spinlock is guaranteed exclusive.
        let keys = unsafe { &mut *self.keys.get() };
        for (i, entry) in keys.iter_mut().enumerate() {
            if !entry.used {
                entry.used = true;
                entry.destructor = destructor;
                self.n_used.fetch_add(1, Ordering::Relaxed);
                return Some(TlsKey(i as u32));
            }
        }
        None // EAGAIN
    }

    /// Frees a TLS key (pthread_key_delete).
    pub fn free_key(&self, key: TlsKey) -> bool {
        if key.0 as usize >= MAX_TLS_KEYS {
            return false;
        }
        let _g = self.lock.lock();
        // SAFETY: access under spinlock.
        let entry = unsafe { &mut (*self.keys.get())[key.0 as usize] };
        if entry.used {
            entry.used = false;
            entry.destructor = None;
            self.n_used.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Returns destructor associated with a key (None if nonexistent).
    pub fn get_destructor(&self, key: TlsKey) -> Option<u64> {
        if key.0 as usize >= MAX_TLS_KEYS {
            return None;
        }
        // SAFETY: lightweight read without lock (conservative).
        let entry = unsafe { &(*self.keys.get())[key.0 as usize] };
        if entry.used {
            entry.destructor
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GS.base — MSR ARCH_SET_GS x86_64
// ─────────────────────────────────────────────────────────────────────────────

/// MSR_FS_BASE (0xC0000100) = FS segment base (libc uses FS for TLS).
const MSR_FS_BASE: u32 = 0xC0000100;
/// MSR_GS_BASE (0xC0000101) = GS segment base.
#[allow(dead_code)]
const MSR_GS_BASE: u32 = 0xC0000101;
/// MSR_KERNEL_GS_BASE (0xC0000102) = GS backup (swapgs).
#[allow(dead_code)]
const MSR_KERNEL_GS_BASE: u32 = 0xC0000102;

/// Writes MSR_FS_BASE with TLS base address (called during context switch).
///
/// # Safety
/// Must be called from a kernel context (Ring 0) during context switch.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn set_fs_base(addr: u64) {
    // SAFETY: wrmsrl instruction valid in Ring 0.
    core::arch::asm!(
        "wrmsr",
        in("ecx") MSR_FS_BASE,
        in("eax") addr as u32,
        in("edx") (addr >> 32) as u32,
        options(nostack, nomem),
    );
}

/// Reads MSR_FS_BASE.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn get_fs_base() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdmsr instruction valid in Ring 0.
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_FS_BASE,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}
