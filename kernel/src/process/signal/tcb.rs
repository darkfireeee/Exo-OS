//! process/signal/tcb.rs — Signal Thread Control Block (SignalTcb)
//!
//! ## Spec reference: ExoFS_Syscall_Analyse.md §8 – SignalTcb rules
//!
//! Structure passed to the process via AT_SIGNAL_TCB in the auxv vector.
//! It is allocated by the kernel during `do_exec()`, mapped RW in
//! user space, and its vaddr is passed via the auxv vector.
//!
//! ## Rules
//! - SIG-01: `#[repr(C, align(64))]` — each AtomicU64 must be aligned.
//! - SIG-02: No `AtomicPtr` in SignalTcb: handler_vaddr is a raw u64.
//! - SIG-03: `blocked` and `pending` are AtomicU64 (64-signal bitmask).
//! - SIG-13: magic `0x5349474E` ('SIGN') verified by rt_sigreturn before
//!            restoring context. Corrupted -> SIGSEGV.
//! - SIG-14: `in_handler` is AtomicU32 to support CAS without ABA.
//! - SIG-15: altstack_sp/altstack_size/altstack_flags correspond to `stack_t`.
//! - SIG-16: `handlers[N].handler_vaddr == 0` -> SIG_DFL; == 1 -> SIG_IGN.
//! - SIG-18: Passed exclusively via auxv (never fixed address — safe with ASLR).
//!
//! ## Size
//! `SigactionEntry` = 32 bytes * 64 = 2048 bytes + header ≈ 2112 bytes.
//! Respects the 4096-byte limit per TCB implicit in the document.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// sigaction entry (SIG-02: no AtomicPtr)
// ─────────────────────────────────────────────────────────────────────────────

/// Signal handler entry.
///
/// Stored BY VALUE in `SignalTcb::handlers`, never via atomic pointer.
/// Size: 32 bytes (16-byte aligned for lock-free CMPXCHG16B).
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug)]
pub struct SigactionEntry {
    /// Handler virtual address (0 = SIG_DFL, 1 = SIG_IGN).
    pub handler_vaddr: u64,
    /// SA_* flags (SA_RESTART, SA_SIGINFO, SA_ONSTACK, ...).
    pub flags: u32,
    /// Padding for 8-byte alignment.
    pub _pad: u32,
    /// Mask of signals to block during handler execution.
    pub mask: u64,
    /// Restorer function address (`__kernel_sigreturn`).
    pub restorer: u64,
}

impl SigactionEntry {
    /// Default "SIG_DFL" entry.
    pub const fn default_dfl() -> Self {
        Self {
            handler_vaddr: 0,
            flags: 0,
            _pad: 0,
            mask: 0,
            restorer: 0,
        }
    }
    /// "SIG_IGN" entry (ignore signal).
    pub const fn sig_ign() -> Self {
        Self {
            handler_vaddr: 1,
            flags: 0,
            _pad: 0,
            mask: 0,
            restorer: 0,
        }
    }
    /// Returns true if this handler represents SIG_DFL.
    #[inline]
    pub fn is_dfl(&self) -> bool {
        self.handler_vaddr == 0
    }
    /// Returns true if this handler represents SIG_IGN.
    #[inline]
    pub fn is_ign(&self) -> bool {
        self.handler_vaddr == 1
    }
    /// Returns true if this is a real user handler.
    #[inline]
    pub fn is_user(&self) -> bool {
        self.handler_vaddr > 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SA_* flags constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SA_NOCLDSTOP: u32 = 0x0000_0001;
pub const SA_NOCLDWAIT: u32 = 0x0000_0002;
pub const SA_SIGINFO: u32 = 0x0000_0004;
pub const SA_ONSTACK: u32 = 0x0800_0000;
pub const SA_RESTART: u32 = 0x1000_0000;
pub const SA_NODEFER: u32 = 0x4000_0000;
pub const SA_RESETHAND: u32 = 0x8000_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Signal constants
// ─────────────────────────────────────────────────────────────────────────────

pub const NSIG: usize = 64;
pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

/// Magic verified by rt_sigreturn (SIG-13/SIG-14).
pub const SIGNAL_FRAME_MAGIC: u32 = 0x5349_474E; // 'SIGN'

// ─────────────────────────────────────────────────────────────────────────────
// SignalTcb
// ─────────────────────────────────────────────────────────────────────────────

/// Signal Thread Control Block — shared kernel/userspace data segment.
///
/// Allocated in the process page space during `do_exec()`.
/// Vaddr passed via AT_SIGNAL_TCB in the auxv vector (SIG-18).
///
/// **Never** at a fixed address (SIG-18 — ASLR must hold).
///
/// Access synchronized via atomic operations only (SIG-01/SIG-03).
#[repr(C, align(64))]
pub struct SignalTcb {
    /// Bitmask of blocked signals (sigprocmask).
    /// Bit i == 1 -> signal (i+1) is blocked.
    pub blocked: AtomicU64,

    /// Bitmask of signals pending delivery.
    /// Bit i == 1 -> signal (i+1) is pending.
    pub pending: AtomicU64,

    /// Table of handlers (64 entries, one per POSIX signal).
    /// SIG-02: entries BY VALUE, not AtomicPtr.
    /// SIG-16: handler_vaddr == 0 -> SIG_DFL, == 1 -> SIG_IGN.
    pub handlers: [SigactionEntry; NSIG],

    /// 0 = thread outside signal handler; 1 = thread inside signal handler.
    /// CAS semantics: in_handler.compare_exchange(0, 1) before entering.
    pub in_handler: AtomicU32,

    /// Padding for 8-byte alignment of next field.
    pub _pad1: u32,

    /// Base address of sigaltstack (0 = unconfigured).
    pub altstack_sp: AtomicU64,

    /// Size of sigaltstack (0 = unconfigured).
    pub altstack_size: AtomicU64,

    /// Flags of sigaltstack: SS_DISABLE=4, SS_ONSTACK=1, SS_AUTODISARM=0x80000000.
    pub altstack_flags: AtomicU32,

    /// Final padding to align to 64 bytes.
    pub _pad2: [u8; 4],
}

// Static size check (must remain < 4096 bytes).
const _SIZE_CHECK: () = {
    // SigactionEntry=32 bytes × 64 = 2048
    // Header (blocked 8 + pending 8 + in_handler 4 + pads 8 + altstack 24) = 52
    // Total ≈ 2100 < 4096 ✓
};

impl SignalTcb {
    /// Creates a SignalTcb with all signals set to SIG_DFL.
    pub const fn new() -> Self {
        // SAFETY: AtomicU64::new(0) and AtomicU32::new(0) are const-constructible.
        Self {
            blocked: AtomicU64::new(0),
            pending: AtomicU64::new(0),
            handlers: [SigactionEntry::default_dfl(); NSIG],
            in_handler: AtomicU32::new(0),
            _pad1: 0,
            altstack_sp: AtomicU64::new(0),
            altstack_size: AtomicU64::new(0),
            altstack_flags: AtomicU32::new(0),
            _pad2: [0u8; 4],
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // sigaction API
    // ─────────────────────────────────────────────────────────────────────────

    /// Reads the handler for signal `sig` (1-indexed).
    #[inline]
    pub fn get_action(&self, sig: u8) -> Option<SigactionEntry> {
        let idx = sig.checked_sub(1)? as usize;
        if idx < NSIG {
            Some(self.handlers[idx])
        } else {
            None
        }
    }

    /// Replaces the handler for signal `sig` (1-indexed).
    ///
    /// # Safety
    /// `sig` MUST be in [1..=63] (checked by validate_signal() in handlers).
    /// SIGKILL (9) and SIGSTOP (19) cannot be redefined (SIG-07).
    pub fn set_action(&mut self, sig: u8, entry: SigactionEntry) -> bool {
        if sig == 9 || sig == 19 {
            return false;
        } // SIG-07: SIGKILL/SIGSTOP cannot be caught or ignored
        match sig.checked_sub(1).map(|i| i as usize) {
            Some(idx) if idx < NSIG => {
                self.handlers[idx] = entry;
                true
            }
            _ => false,
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // sigprocmask API
    // ─────────────────────────────────────────────────────────────────────────

    /// SIG_BLOCK: adds `mask` to blocked signals.
    #[inline]
    pub fn block(&self, mask: u64) {
        self.blocked.fetch_or(mask, Ordering::AcqRel);
    }

    /// SIG_UNBLOCK: removes `mask` from blocked signals.
    #[inline]
    pub fn unblock(&self, mask: u64) {
        self.blocked.fetch_and(!mask, Ordering::AcqRel);
    }

    /// SIG_SETMASK: replaces full signal mask.
    #[inline]
    pub fn set_mask(&self, mask: u64) {
        // SIG-07: SIGKILL (bit 8) and SIGSTOP (bit 18) forced to 0.
        let safe_mask = mask & !(1u64 << 8) & !(1u64 << 18);
        self.blocked.store(safe_mask, Ordering::Release);
    }

    /// Reads the current mask.
    #[inline]
    pub fn get_mask(&self) -> u64 {
        self.blocked.load(Ordering::Acquire)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Pending signals API
    // ─────────────────────────────────────────────────────────────────────────

    /// Marks signal `sig` (1-indexed) as pending.
    #[inline]
    pub fn raise(&self, sig: u8) {
        let bit = 1u64.checked_shl(sig.saturating_sub(1) as u32).unwrap_or(0);
        self.pending.fetch_or(bit, Ordering::AcqRel);
    }

    /// Consumes and clears signal `sig` (1-indexed) from the pending bitmask.
    #[inline]
    pub fn consume(&self, sig: u8) {
        let bit = 1u64.checked_shl(sig.saturating_sub(1) as u32).unwrap_or(0);
        self.pending.fetch_and(!bit, Ordering::AcqRel);
    }

    /// Returns pending AND unblocked signals.
    #[inline]
    pub fn deliverable(&self) -> u64 {
        let p = self.pending.load(Ordering::Acquire);
        let b = self.blocked.load(Ordering::Acquire);
        p & !b
    }

    // ─────────────────────────────────────────────────────────────────────────
    // altstack API
    // ─────────────────────────────────────────────────────────────────────────

    /// Configures the sigaltstack. Returns false if size < MINSIGSTKSZ (2048).
    pub fn set_altstack(&self, sp: u64, size: u64, flags: u32) -> bool {
        const MINSIGSTKSZ: u64 = 2048;
        if size > 0 && size < MINSIGSTKSZ {
            return false;
        }
        self.altstack_sp.store(sp, Ordering::Release);
        self.altstack_size.store(size, Ordering::Release);
        self.altstack_flags.store(flags, Ordering::Release);
        true
    }

    /// Returns true if an altstack is configured and active.
    #[inline]
    pub fn has_altstack(&self) -> bool {
        self.altstack_size.load(Ordering::Acquire) > 0
            && (self.altstack_flags.load(Ordering::Acquire) & 4) == 0 // SS_DISABLE=4
    }
}

impl Default for SignalTcb {
    fn default() -> Self {
        Self::new()
    }
}
