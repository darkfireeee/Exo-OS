// kernel/src/process/signal/queue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX.1b signal queue (RT signals 32-63) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// POSIX.1b guarantees that at least one occurrence of each RT signal can be
// queued. A fixed array per signal is used (max SIGQUEUE_DEPTH = 32
// entries) + atomic counter, without dynamic allocation.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Maximum queue depth per RT signal.
pub const SIGQUEUE_DEPTH: usize = 32;
/// Number of RT signals.
const RT_NSIG: usize = 32; // 32..63

/// Global overflow counter (statistics).
pub static SIGQUEUE_OVERFLOW: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// SigInfo — information attached to a signal (siginfo_t)
// ──────────────────────────────────────────────────═──────────────────────────

/// Information associated with a signal (siginfo_t equivalent).
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct SigInfo {
    /// Signal number (1..63).
    pub signo: u32,
    /// Signal code (SI_USER, SI_KERNEL, SI_QUEUE...).
    pub code: i32,
    /// Sender PID (0 if kernel).
    pub sender_pid: u32,
    /// Sender UID.
    pub sender_uid: u32,
    /// Value if POSIX.1b sigqueue() (integer).
    pub value_int: i32,
    /// Value if POSIX.1b sigqueue() (pointer, represented as u64).
    pub value_ptr: u64,
    // If code=SIGFPE/SIGSEGV: faulting address
    pub fault_addr: u64,
}

impl SigInfo {
    pub const SI_USER: i32 = 0;
    pub const SI_KERNEL: i32 = 0x80;
    pub const SI_QUEUE: i32 = -1;
    pub const SI_TIMER: i32 = -2;
    pub const SI_MESGQ: i32 = -3;
    pub const SI_ASYNCIO: i32 = -4;

    /// Creates a SigInfo for kill(2).
    #[inline]
    pub fn from_kill(sig: u8, sender_pid: u32, sender_uid: u32) -> Self {
        Self {
            signo: sig as u32,
            code: Self::SI_USER,
            sender_pid,
            sender_uid,
            ..Default::default()
        }
    }

    /// Creates a SigInfo for internal kernel kill.
    #[inline]
    pub fn kernel(sig: u8) -> Self {
        Self {
            signo: sig as u32,
            code: Self::SI_KERNEL,
            ..Default::default()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SigQueue — standard signal queue (one bit per signal)
// ─────────────────────────────────────────────────────────────────────────────

/// Standard signal queue (signals 1..31).
/// One bit per signal; multiple deliveries of the same signal = 1 delivery.
#[repr(C)]
pub struct SigQueue {
    /// Bitmap of pending signals (bit i = signal i+1 is pending).
    pub pending: AtomicU64,
}

impl Default for SigQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl SigQueue {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU64::new(0),
        }
    }

    /// Initializes a standard queue directly in its final location.
    ///
    /// Used by thread allocations to avoid materializing large signal
    /// structures on the kernel stack.
    ///
    /// # Safety
    /// `dst` must point to valid, uninitialized memory for `SigQueue`.
    pub unsafe fn init_at(dst: *mut Self) {
        core::ptr::addr_of_mut!((*dst).pending).write(AtomicU64::new(0));
    }

    /// Enqueues a signal.
    #[inline(always)]
    pub fn enqueue(&self, sig: u8) {
        if sig == 0 || sig > 63 {
            return;
        }
        self.pending.fetch_or(1u64 << (sig - 1), Ordering::AcqRel);
    }

    /// Dequeues the first unblocked signal.
    /// Returns (signo, SigInfo) or None.
    pub fn dequeue(&self, mask: u64) -> Option<(u8, SigInfo)> {
        loop {
            let pending = self.pending.load(Ordering::Acquire);
            let unblocked = pending & !mask;
            if unblocked == 0 {
                return None;
            }
            let bit = unblocked.trailing_zeros() as u8;
            let old = self.pending.fetch_and(!(1u64 << bit), Ordering::AcqRel);
            if old & (1u64 << bit) != 0 {
                return Some((bit + 1, SigInfo::kernel(bit + 1)));
            }
            // The bit was cleared by another CPU in between: retry.
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.pending.load(Ordering::Acquire) == 0
    }

    /// Pending signals not blocked by `mask`.
    #[inline(always)]
    pub fn has_pending(&self, mask: u64) -> bool {
        self.pending.load(Ordering::Acquire) & !mask != 0
    }

    #[inline(always)]
    pub fn clear(&self) {
        self.pending.store(0, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RTSigQueue — real-time signal queue (POSIX.1b, signals 32..63)
// ─────────────────────────────────────────────────────────────────────────────

/// An entry in the RT queue.
#[derive(Copy, Clone, Debug, Default)]
struct RTEntry {
    info: SigInfo,
    valid: bool,
}

/// Circular ring buffer per RT signal — SIGQUEUE_DEPTH entries.
struct RTRing {
    /// Raw data.
    entries: UnsafeCell<[RTEntry; SIGQUEUE_DEPTH]>,
    /// Read index.
    head: UnsafeCell<usize>,
    /// Write index.
    tail: AtomicUsize,
    /// Number of queued elements.
    count: AtomicU32,
}

// SAFETY: Exo-OS operates in a uniprocessor kernel with a single producer/consumer
// per RT queue; exclusive access is guaranteed by the PCB lock or masked
// interrupt context. UnsafeCell fields are protected by convention.
unsafe impl Send for RTRing {}
unsafe impl Sync for RTRing {}

impl RTRing {
    const fn new() -> Self {
        Self {
            entries: UnsafeCell::new(
                [RTEntry {
                    info: SigInfo {
                        signo: 0,
                        code: 0,
                        sender_pid: 0,
                        sender_uid: 0,
                        value_int: 0,
                        value_ptr: 0,
                        fault_addr: 0,
                    },
                    valid: false,
                }; SIGQUEUE_DEPTH],
            ),
            head: UnsafeCell::new(0),
            tail: AtomicUsize::new(0),
            count: AtomicU32::new(0),
        }
    }

    unsafe fn init_at(dst: *mut Self) {
        core::ptr::addr_of_mut!((*dst).entries).write(UnsafeCell::new(
            [RTEntry {
                info: SigInfo {
                    signo: 0,
                    code: 0,
                    sender_pid: 0,
                    sender_uid: 0,
                    value_int: 0,
                    value_ptr: 0,
                    fault_addr: 0,
                },
                valid: false,
            }; SIGQUEUE_DEPTH],
        ));
        core::ptr::addr_of_mut!((*dst).head).write(UnsafeCell::new(0));
        core::ptr::addr_of_mut!((*dst).tail).write(AtomicUsize::new(0));
        core::ptr::addr_of_mut!((*dst).count).write(AtomicU32::new(0));
    }

    /// Enqueues a SigInfo.
    fn push(&self, info: SigInfo) -> bool {
        if self.count.load(Ordering::Acquire) as usize >= SIGQUEUE_DEPTH {
            SIGQUEUE_OVERFLOW.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let tail = self.tail.load(Ordering::Acquire);
        // SAFETY: exclusive access guaranteed by caller context (PCB lock).
        unsafe {
            (*self.entries.get())[tail % SIGQUEUE_DEPTH] = RTEntry { info, valid: true };
        }
        self.tail.store(tail + 1, Ordering::Release);
        self.count.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// Dequeues the next valid element.
    fn pop(&self) -> Option<SigInfo> {
        if self.count.load(Ordering::Acquire) == 0 {
            return None;
        }
        // SAFETY: exclusive access guaranteed by caller context.
        let info = unsafe {
            let head_ptr = self.head.get();
            let head = *head_ptr;
            let entry = &mut (*self.entries.get())[head % SIGQUEUE_DEPTH];
            if !entry.valid {
                return None;
            }
            let info = entry.info;
            entry.valid = false;
            *head_ptr += 1;
            info
        };
        self.count.fetch_sub(1, Ordering::AcqRel);
        Some(info)
    }

    fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }

    fn clear(&self) {
        unsafe {
            for entry in (*self.entries.get()).iter_mut() {
                entry.valid = false;
            }
            *self.head.get() = 0;
        }
        self.tail.store(0, Ordering::Release);
        self.count.store(0, Ordering::Release);
    }
}

/// Real-time signal queue for the 32 RT signals (SIGRTMIN..SIGRTMAX).
/// Each signal has its own circular ring buffer.
pub struct RTSigQueue {
    /// One queue per RT signal (index 0 = SIGRTMIN = signal 32).
    rings: [RTRing; RT_NSIG],
    /// Bitmap indicating which slots have pending elements.
    pub pending_mask: AtomicU64,
}

// SAFETY: same rationale as RTRing.
unsafe impl Send for RTSigQueue {}
unsafe impl Sync for RTSigQueue {}

impl Default for RTSigQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl RTSigQueue {
    pub const fn new() -> Self {
        Self {
            rings: [const { RTRing::new() }; RT_NSIG],
            pending_mask: AtomicU64::new(0),
        }
    }

    /// Initializes the RT queue directly in its final location.
    ///
    /// `RTSigQueue` contains `RT_NSIG * SIGQUEUE_DEPTH` fixed entries;
    /// constructing it as a temporary on a kstack can overflow the guard
    /// page during fork/exec. This primitive preserves the invariant without
    /// stack pressure.
    ///
    /// # Safety
    /// `dst` must point to valid, uninitialized memory for `RTSigQueue`.
    pub unsafe fn init_at(dst: *mut Self) {
        let rings = core::ptr::addr_of_mut!((*dst).rings) as *mut RTRing;
        for idx in 0..RT_NSIG {
            RTRing::init_at(rings.add(idx));
        }
        core::ptr::addr_of_mut!((*dst).pending_mask).write(AtomicU64::new(0));
    }

    /// Enqueues a real-time signal.
    /// `sig` must be in [32..63].
    pub fn enqueue(&self, sig: u8, info: SigInfo) -> bool {
        let idx = sig.wrapping_sub(32) as usize;
        if idx >= RT_NSIG {
            return false;
        }
        let ok = self.rings[idx].push(info);
        if ok {
            self.pending_mask.fetch_or(1u64 << idx, Ordering::AcqRel);
        }
        ok
    }

    /// Dequeues the first unblocked RT signal (mask = signal_mask >> 31).
    pub fn dequeue(&self, rt_mask: u32) -> Option<(u8, SigInfo)> {
        let pending = self.pending_mask.load(Ordering::Acquire) as u32;
        let unblocked = pending & !rt_mask;
        if unblocked == 0 {
            return None;
        }
        let idx = unblocked.trailing_zeros() as usize;
        let ring = &self.rings[idx];
        if let Some(info) = ring.pop() {
            if ring.is_empty() {
                self.pending_mask
                    .fetch_and(!(1u64 << idx), Ordering::AcqRel);
            }
            Some((idx as u8 + 32, info))
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending_mask.load(Ordering::Acquire) == 0
    }

    pub fn has_pending(&self, rt_mask: u32) -> bool {
        let p = self.pending_mask.load(Ordering::Acquire) as u32;
        p & !rt_mask != 0
    }

    pub fn clear(&self) {
        for ring in &self.rings {
            ring.clear();
        }
        self.pending_mask.store(0, Ordering::Release);
    }
}
