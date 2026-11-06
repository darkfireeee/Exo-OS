// kernel/src/scheduler/core/preempt.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// RAII PREEMPTION — PreemptGuard (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE PREEMPT-01 (DOC3): Never call preempt_disable/enable directly.
//   Always use PreemptGuard to guarantee re-enabling via Drop.
//   Reason: an exception between disable and enable would corrupt the
//   preemption counter and cause a deadlock or scheduler crash.
//
// RULE NO-ALLOC ZONE: this file must never allocate heap memory.
// RULE UNSAFE: all unsafe blocks documented by // SAFETY:
// ═══════════════════════════════════════════════════════════════════════════════

use core::marker::PhantomData;
use core::sync::atomic::{AtomicI32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU preemption counter
//
// Stored here as a per-CPU static simulated by an aligned array.
// In production on a real kernel, this would be a field in the per-CPU data
// structure, accessed via GS:offset. A static array approximation is used
// for a maximum of MAX_CPUS CPUs.
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of supported CPUs.
/// CORR-27: must be >= SSR_MAX_CORES_LAYOUT (256) — mandatory Phase 0.
/// WAS: 64 — FIXED: 256
pub const MAX_CPUS: usize = 256;

/// Per-CPU preemption counters — one counter per logical CPU.
///
/// Value 0: preemptible.
/// Value > 0: non-preemptible (nesting depth).
/// Value < 0: BUG — enable/disable imbalance (detected in debug).
#[repr(C, align(64))]
struct PreemptCounter(AtomicI32, [u8; 60]); // cache line padding

static PREEMPT_COUNT: [PreemptCounter; MAX_CPUS] =
    [const { PreemptCounter(AtomicI32::new(0), [0u8; 60]) }; MAX_CPUS];

/// Global preemption disable counter (instrumentation).
static PREEMPT_DISABLE_TOTAL: AtomicI32 = AtomicI32::new(0);

/// Returns current CPU ID.
/// In a real x86_64 kernel, GS:percpu_cpu_id would be read.
/// Here an external function provided by arch/ is called.
#[inline(always)]
fn current_cpu_id() -> usize {
    // SAFETY: read of a per-CPU variable via kernel ABI.
    // The value is always in [0, MAX_CPUS).
    #[cfg(test)]
    {
        0
    }
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        // Read cpu_id field in per-CPU data (GS:0).
        // In the absence of complete mechanism, return 0.
        extern "C" {
            fn arch_current_cpu() -> u32;
        }
        // SAFETY: arch_current_cpu() is defined in arch/x86_64 and always
        // returns a value in [0, MAX_CPUS).
        let id = unsafe { arch_current_cpu() } as usize;
        id.min(MAX_CPUS - 1)
    }
    #[cfg(all(not(target_arch = "x86_64"), not(test)))]
    {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Low-level internal API (PRIVATE)
// ─────────────────────────────────────────────────────────────────────────────

/// Disables preemption for the current CPU (increments counter).
/// PRIVATE — use PreemptGuard.
#[inline(always)]
fn preempt_disable_raw() {
    let cpu = current_cpu_id();
    let prev = PREEMPT_COUNT[cpu].0.fetch_add(1, Ordering::Acquire);
    publish_preempt_shadow(prev.saturating_add(1));
    PREEMPT_DISABLE_TOTAL.fetch_add(1, Ordering::Relaxed);
    // Positive overflow detection > 64 -> sign of a nesting bug.
    debug_assert!(
        prev < 64,
        "preempt_disable: compteur > 64, bug d'imbrication probable"
    );
}

/// Re-enables preemption for the current CPU (decrements counter).
/// PRIVATE — use PreemptGuard::drop.
#[inline(always)]
fn preempt_enable_raw() {
    let cpu = current_cpu_id();
    let prev = PREEMPT_COUNT[cpu].0.fetch_sub(1, Ordering::Release);
    publish_preempt_shadow(prev.saturating_sub(1));
    debug_assert!(
        prev > 0,
        "preempt_enable: compteur était <= 0 — déséquilibre disable/enable FATAL"
    );
}

#[inline(always)]
fn publish_preempt_shadow(depth: i32) {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    unsafe {
        extern "C" {
            fn arch_set_preempt_count_shadow(depth: i32);
        }
        arch_set_preempt_count_shadow(depth);
    }
    #[cfg(any(not(target_arch = "x86_64"), test))]
    {
        let _ = depth;
    }
}

/// Returns `true` if preemption is disabled on the current CPU.
#[inline(always)]
pub fn is_preempt_disabled() -> bool {
    let cpu = current_cpu_id();
    PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed) > 0
}

#[inline(always)]
pub(crate) fn preempt_disable_for_arch_compat() {
    preempt_disable_raw();
}

#[inline(always)]
pub(crate) fn preempt_enable_for_arch_compat() {
    preempt_enable_raw();
}

// ─────────────────────────────────────────────────────────────────────────────
// PreemptGuard — Mandatory RAII
// ─────────────────────────────────────────────────────────────────────────────

/// Disables preemption for the lifetime of this guard.
///
/// # Example
/// ```rust,ignore
/// {
///     let _guard = PreemptGuard::new();  // preemption disabled
///     // ... critical section relative to the scheduler ...
///     // Automatic drop -> preemption re-enabled
/// }
/// ```
///
/// # Usage rules
/// - NEVER store a `PreemptGuard` in a persistent struct field.`
/// - NEVER transfer a `PreemptGuard` across threads (not `Send`).
/// - **Minimal** duration: just enough to protect the critical section.
/// - FORBIDDEN to call functions that may sleep inside.
pub struct PreemptGuard {
    /// PhantomData *mut () makes this type !Send and !Sync.
    _phantom: PhantomData<*mut ()>,
}

impl Default for PreemptGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl PreemptGuard {
    /// Disables preemption and returns an RAII guard.
    #[inline(always)]
    #[must_use = "PreemptGuard doit être stocké dans une variable locale pour avoir un effet"]
    pub fn new() -> Self {
        preempt_disable_raw();
        Self {
            _phantom: PhantomData,
        }
    }

    /// Checks if preemption is currently disabled on this CPU.
    #[inline(always)]
    pub fn is_preempted_disabled() -> bool {
        is_preempt_disabled()
    }

    /// Returns current nesting depth (debug).
    #[inline(always)]
    pub fn depth() -> i32 {
        let cpu = current_cpu_id();
        PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed)
    }
}

impl Drop for PreemptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        preempt_enable_raw();
    }
}

// PreemptGuard is !Send (PhantomData<*mut ()>), implicit !Sync.
// Intentional lack of Send/Sync impl.

// ─────────────────────────────────────────────────────────────────────────────
// IrqGuard — disables interrupts + preemption (IRQ-safe sections)
// ─────────────────────────────────────────────────────────────────────────────

/// Disables hardware IRQs AND preemption.
/// Used for IRQ-safe spinlocks (scheduler tick, IPI handlers).
pub struct IrqGuard {
    /// Saved RFLAGS (IF bit).
    rflags: u64,
    _phantom: PhantomData<*mut ()>,
}

impl Default for IrqGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl IrqGuard {
    /// Saves RFLAGS, clears IRQs, disables preemption.
    #[inline(always)]
    #[must_use]
    pub fn new() -> Self {
        let rflags: u64;
        #[cfg(target_arch = "x86_64")]
        // SAFETY: pushfq/popq read RFLAGS; cli clears IRQs; correct order (save before clearing).
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {flags}",
                "cli",
                flags = out(reg) rflags,
                options(nomem)
            );
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            rflags = 0;
        }

        preempt_disable_raw();
        Self {
            rflags,
            _phantom: PhantomData,
        }
    }

    /// True if IRQs were enabled before this guard (IF bit = 9 of RFLAGS).
    #[inline(always)]
    pub fn irqs_were_enabled(&self) -> bool {
        self.rflags & (1 << 9) != 0
    }

    /// Releases preemption depth while keeping IRQs disabled.
    ///
    /// Context switch cannot keep an RAII guard alive across the
    /// stack switch: its Drop would run in the old context when
    /// this thread is rescheduled. This helper covers the scheduler case:
    /// selection under disabled preemption, then ASM jump with IF=0 and
    /// preemption counter already restored.
    #[inline(always)]
    pub fn release_keep_irqs_disabled(self) -> u64 {
        let rflags = self.rflags;
        preempt_enable_raw();
        core::mem::forget(self);
        rflags
    }

    /// Restores IRQ state captured by `release_keep_irqs_disabled`.
    ///
    /// # Safety
    /// `rflags` must originate from an `IrqGuard` acquired on this CPU.
    #[inline(always)]
    pub unsafe fn restore_irq_flags(rflags: u64) {
        if rflags & (1 << 9) != 0 {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
            }
        }
    }
}

impl Drop for IrqGuard {
    #[inline(always)]
    fn drop(&mut self) {
        preempt_enable_raw();
        if self.rflags & (1 << 9) != 0 {
            #[cfg(target_arch = "x86_64")]
            // SAFETY: sti restores IF bit; rflags comes from pushfq/popq in new(); atomic restore.
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes per-CPU preemption counters for all CPUs.
/// Called from scheduler initialization sequence (step 1).
pub fn init() {
    for i in 0..MAX_CPUS {
        PREEMPT_COUNT[i].0.store(0, Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consistency assertions (verified in debug mode)
// ─────────────────────────────────────────────────────────────────────────────

/// Panics if preemption is not disabled on current CPU.
/// Used to verify invariants in the scheduler.
#[inline(always)]
pub fn assert_preempt_disabled() {
    assert!(
        PreemptGuard::is_preempted_disabled(),
        "Assertion préemption désactivée : ÉCHEC — appel hors section protégée"
    );
}

/// Panics if preemption is disabled (to verify sleep is allowed).
#[inline(always)]
pub fn assert_preempt_enabled() {
    assert!(
        !PreemptGuard::is_preempted_disabled(),
        "Assertion préemption activée : ÉCHEC — can_sleep() hors section dormable"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Returns total number of times preemption has been disabled
/// since boot (across all CPUs, instrumentation).
pub fn total_preempt_disable_count() -> i32 {
    PREEMPT_DISABLE_TOTAL.load(Ordering::Relaxed)
}
