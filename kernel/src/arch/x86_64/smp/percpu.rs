//! # arch/x86_64/smp/percpu.rs — Per-CPU Data (GS Segment)
//!
//! Each CPU possesses a private data area accessible via the GS register.
//! The GS layout is FIXED and shared between ASM (syscall_entry_asm, exception stubs)
//! and Rust code.
//!
//! ## GS Layout (fixed offsets — DO NOT change without updating ASM stubs)
//! ```
//! gs:[0x00]  = u64 : kernel_rsp    — syscall stack of current thread (kernel RSP)
//! gs:[0x08]  = u64 : user_rsp      — userspace RSP saved during syscall
//! gs:[0x10]  = u64 : cpu_id        — logical CPU identifier (0-based)
//! gs:[0x18]  = u64 : lapic_id      — current CPU APIC ID
//! gs:[0x20]  = u64 : current_tcb   — current thread TCB pointer
//! gs:[0x28]  = u64 : idle_rsp      — idle stack RSP for this CPU
//! gs:[0x30]  = u64 : preempt_shadow — shadow mirror of canonical scheduler counter
//! gs:[0x38]  = u64 : irq_depth     — IRQ nesting depth
//! gs:[0x40]  = u64 : kpti_kernel_cr3 — kernel CR3 for syscall entry
//! gs:[0x48]  = u64 : kpti_user_cr3   — user CR3 for syscall exit
//! gs:[0x50]  = u64 : syscall_scratch — ASM scratch during CR3 switch
//! ```

use crate::arch::x86_64::cpu::msr;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

pub const MAX_CPUS: usize = 256;

// ── Online CPU Counter ────────────────────────────────────────────────────────

static ONLINE_CPU_COUNT: AtomicU32 = AtomicU32::new(0);

/// Returns the number of currently initialized CPUs (BSP + reported APs)
#[inline(always)]
pub fn cpu_count() -> u32 {
    ONLINE_CPU_COUNT.load(Ordering::Acquire)
}

// ── PerCpuData Structure ──────────────────────────────────────────────────────

/// Per-CPU data — stored in .percpu section or in a static table
///
/// Aligned to 64 bytes to prevent false sharing between CPUs.
#[repr(C, align(64))]
pub struct PerCpuData {
    // Fields accessed from ASM — FIXED offsets (see module doc)
    pub kernel_rsp: u64,      // 0x00
    pub user_rsp: u64,        // 0x08
    pub cpu_id: u64,          // 0x10
    pub lapic_id: u64,        // 0x18
    pub current_tcb: u64,     // 0x20 (opaque pointer, cast to extern TCB)
    pub idle_rsp: u64,        // 0x28
    pub preempt_shadow: u64,  // 0x30 mirror of canonical scheduler counter
    pub irq_depth: u64,       // 0x38
    pub kpti_kernel_cr3: u64, // 0x40
    pub kpti_user_cr3: u64,   // 0x48
    pub syscall_scratch: u64, // 0x50

    // Rust data (offsets unconstrained by ASM)
    pub online: bool,
    pub bsp: bool,
    pub nmi_in_progress: bool,

    /// Context switch counter on this CPU
    pub ctx_switch_count: u64,
    /// IRQ counters per vector (256 entries, shared with idt::irq_counter)
    pub irq_counts: [u64; 256],
    /// TSC timestamp of last context switch
    pub last_switch_tsc: u64,
    /// TSC timestamp of last timer tick
    pub last_tick_tsc: u64,

    _pad: [u8; 0], // guarantees 64-byte alignment
}

const _: () = assert!(
    core::mem::size_of::<PerCpuData>() <= 4096,
    "PerCpuData must fit in one page"
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, current_tcb) == 0x20,
    "PerCpuData.current_tcb must stay at GS:0x20"
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, preempt_shadow) == 0x30,
    "PerCpuData.preempt_shadow must stay at GS:0x30"
);

impl PerCpuData {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        Self {
            kernel_rsp: 0,
            user_rsp: 0,
            cpu_id: 0,
            lapic_id: 0,
            current_tcb: 0,
            idle_rsp: 0,
            preempt_shadow: 0,
            irq_depth: 0,
            kpti_kernel_cr3: 0,
            kpti_user_cr3: 0,
            syscall_scratch: 0,
            online: false,
            bsp: false,
            nmi_in_progress: false,
            ctx_switch_count: 0,
            irq_counts: [0u64; 256],
            last_switch_tsc: 0,
            last_tick_tsc: 0,
            _pad: [],
        }
    }
}

// ── Global Per-CPU Data Table ─────────────────────────────────────────────────

/// Static table — one PerCpuData per potential CPU
///
/// Zero-initialized at boot, then configured by `init_percpu_for_bsp/ap`.
/// Placed in `.data.percpu` to remain outside `.rodata`: these slots are
/// mutable by design and take part in the syscall/IRQ path.
#[repr(align(64))]
struct PerCpuTable(UnsafeCell<[PerCpuData; MAX_CPUS]>);

// SAFETY: mutable accesses are disciplined per-CPU via `per_cpu_mut()`.
unsafe impl Sync for PerCpuTable {}

#[link_section = ".data.percpu"]
static PER_CPU_TABLE: PerCpuTable = PerCpuTable(
    // SAFETY: [0u8; size_of::<[PerCpuData; MAX_CPUS]>] valid for #[repr(C,align(64))] zeros-init.
    UnsafeCell::new(unsafe {
        core::mem::transmute::<[u8; core::mem::size_of::<[PerCpuData; MAX_CPUS]>()], [PerCpuData; MAX_CPUS]>(
            [0u8; core::mem::size_of::<[PerCpuData; MAX_CPUS]>()],
        )
    }),
);

#[inline(always)]
fn per_cpu_ptr(cpu_id: usize) -> *mut PerCpuData {
    let low_ptr = unsafe { (*PER_CPU_TABLE.0.get()).as_mut_ptr().add(cpu_id) };

    #[cfg(target_os = "none")]
    {
        crate::memory::phys_to_virt(crate::memory::PhysAddr::new(low_ptr as u64)).as_u64()
            as *mut PerCpuData
    }

    #[cfg(not(target_os = "none"))]
    {
        low_ptr
    }
}

// ── Access by CPU ID ──────────────────────────────────────────────────────────

/// Returns an immutable reference to data for CPU `cpu_id`
///
/// # Panics
/// If `cpu_id >= MAX_CPUS`.
#[inline]
pub fn per_cpu(cpu_id: usize) -> &'static PerCpuData {
    // SAFETY: shared read of a per-CPU slot. Mutations of this slot are
    // reserved for the owner CPU or exclusive initialization phases.
    unsafe { &*per_cpu_ptr(cpu_id) }
}

/// Returns a mutable reference to data for CPU `cpu_id`
///
/// # Safety
/// Caller must guarantee that no other context simultaneously accesses
/// this CPU's data (invariant upheld as each CPU accesses its own data).
#[inline]
pub unsafe fn per_cpu_mut(cpu_id: usize) -> &'static mut PerCpuData {
    // SAFETY: PER_CPU_TABLE is a static table mutable via UnsafeCell;
    // each CPU exclusively accesses its own entry.
    &mut *per_cpu_ptr(cpu_id)
}

/// Returns current CPU ID by reading GS:[0x10]
#[inline]
pub fn current_cpu_id() -> u32 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        let id: u64;
        // SAFETY: GS:[0x10] is initialized during percpu init of this CPU
        unsafe {
            core::arch::asm!("mov {}, gs:[0x10]", out(reg) id, options(nostack, nomem));
        }
        id as u32
    }
}

#[inline(always)]
fn percpu_table_bounds() -> (u64, u64) {
    let base = per_cpu_ptr(0) as u64;
    let len = (core::mem::size_of::<PerCpuData>() * MAX_CPUS) as u64;
    (base, base.saturating_add(len))
}

#[inline(always)]
pub fn is_kernel_percpu_gs_base(gs_base: u64) -> bool {
    let (base, end) = percpu_table_bounds();
    gs_base >= base && gs_base < end
}

#[inline(always)]
pub fn try_current_cpu_id() -> Option<u32> {
    // SAFETY: privileged MSR read in Ring 0. We verify GS base before
    // any `gs:[...]` access to avoid a recursive fault in handler.
    let gs_base = unsafe { msr::read_msr(msr::MSR_GS_BASE) };
    if !is_kernel_percpu_gs_base(gs_base) {
        return None;
    }
    Some(current_cpu_id())
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — scheduler/core/preempt.rs interface (RULE SCHED-01 DOC3)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns logical ID of current CPU (C ABI export).
///
/// Called from `scheduler::core::preempt` to access per-CPU counter.
/// Reads GS:[0x10] lock-free (inherently atomic per-CPU).
///
/// # Safety
/// GS must be initialized on this CPU (guaranteed after `init_percpu_for_bsp/ap`).
#[no_mangle]
pub unsafe extern "C" fn arch_current_cpu() -> u32 {
    current_cpu_id()
}

/// Publishes canonical preemption counter mirror into GS:[0x30].
///
/// This slot exists strictly for low-level ASM code; the source of truth
/// remains `scheduler::core::preempt::PREEMPT_COUNT`.
#[no_mangle]
pub extern "C" fn arch_set_preempt_count_shadow(depth: i32) {
    #[cfg(test)]
    {
        let _ = depth;
    }
    #[cfg(not(test))]
    {
        let value = depth.max(0) as u64;
        // SAFETY: GS points to current per-CPU slot; offset 0x30 = preempt_shadow.
        unsafe {
            core::arch::asm!(
                "mov gs:[0x30], {}",
                in(reg) value,
                options(nostack, preserves_flags),
            );
        }
    }
}

/// Returns a reference to per-CPU data of current CPU
///
/// # Safety
/// Must be called from the context of this CPU only.
#[inline]
pub fn current_per_cpu() -> &'static PerCpuData {
    per_cpu(current_cpu_id() as usize)
}

/// Writes current TCB pointer into `gs:[0x20]`.
///
/// Called by scheduler immediately after a context switch, ensuring
/// syscall/exception path always finds the proper current thread.
#[inline(always)]
pub fn set_current_tcb(tcb: *mut crate::scheduler::core::task::ThreadControlBlock) {
    #[cfg(test)]
    {
        let _ = tcb;
    }
    #[cfg(not(test))]
    {
        let tcb_ptr = tcb as u64;
        // SAFETY: GS points to a valid per-CPU area initialized at boot.
        // Offset 0x20 corresponds to `current_tcb` field of PerCpuData layout.
        unsafe {
            core::arch::asm!(
                "mov gs:[0x20], {}",
                in(reg) tcb_ptr,
                options(nostack, preserves_flags),
            );
        }
    }
}

/// Reads current value of slot `gs:[0x20]` (`current_tcb`).
///
/// Useful for instrumentation/debug and P2-7 tests.
///
/// # Safety
/// GS must already point to a valid per-CPU area.
#[inline(always)]
pub unsafe fn read_current_tcb() -> u64 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        let tcb: u64;
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb,
            options(nostack, preserves_flags),
        );
        tcb
    }
}

/// Reads `current_tcb` only if GS points to a valid kernel per-CPU area.
///
/// Exception handlers can be invoked in sensitive windows
/// (syscall/KPTI return, nested kernel fault). In those paths, reading
/// `gs:[0x20]` directly could cascade the initial fault into
/// `CR2=0x20`. This API makes the contract explicit.
///
/// # Safety
/// Executable strictly from Ring 0 in contexts where RDMSR is permitted.
#[inline(always)]
pub unsafe fn try_read_current_tcb() -> Option<u64> {
    let gs_base = unsafe { msr::read_msr(msr::MSR_GS_BASE) };
    if !is_kernel_percpu_gs_base(gs_base) {
        return None;
    }
    Some(unsafe { read_current_tcb() })
}

/// Updates slot `gs:[0x00]` with the incoming thread's kernel stack.
///
/// INVARIANT P1-5: called in `context_switch()` after each selection
/// of next thread, before resuming userspace/syscall execution.
///
/// # Safety
/// `kstack_top` must be a valid kernel stack address for the current
/// thread (ABI alignment, kernel mapping present).
#[inline(always)]
pub unsafe fn set_kernel_rsp(kstack_top: u64) {
    #[cfg(test)]
    {
        let _ = kstack_top;
    }
    #[cfg(not(test))]
    {
        core::arch::asm!(
            "mov gs:[0x00], {}",
            in(reg) kstack_top,
            options(nostack, preserves_flags),
        );
    }
}

/// Reads current value of slot `gs:[0x00]`.
///
/// Useful for instrumentation/debug and P2-7 test preparation.
///
/// # Safety
/// GS must already point to a valid per-CPU area.
#[inline(always)]
pub unsafe fn read_kernel_rsp() -> u64 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        let rsp: u64;
        core::arch::asm!("mov {}, gs:[0x00]", out(reg) rsp, options(nostack, preserves_flags));
        rsp
    }
}

/// Publishes CR3 values used by KPTI syscall stub.
#[inline(always)]
pub fn set_kpti_cr3_slots(kernel_cr3: u64, user_cr3: u64) {
    #[cfg(test)]
    {
        let _ = (kernel_cr3, user_cr3);
    }
    #[cfg(not(test))]
    {
        // SAFETY: GS points to a valid per-CPU area. Offsets 0x40/0x48
        // are read directly by syscall_entry_asm before/after Rust handler.
        unsafe {
            core::arch::asm!(
                "mov gs:[0x40], {kernel}",
                "mov gs:[0x48], {user}",
                kernel = in(reg) kernel_cr3,
                user = in(reg) user_cr3,
                options(nostack, preserves_flags),
            );
        }
    }
}

// ── Initialization ────────────────────────────────────────────────────────────

/// Initializes per-CPU data for BSP (CPU 0)
///
/// Called from `boot::early_init` before any other use of GS.
pub fn init_percpu_for_bsp(kernel_stack_top: u64, lapic_id: u32) {
    // SAFETY: exclusive access to CPU 0 during boot (APs not yet started)
    let data = unsafe { per_cpu_mut(0) };
    data.cpu_id = 0;
    data.lapic_id = lapic_id as u64;
    // NOTE P1-5: boot thread initialization only.
    // After first context switch, `set_kernel_rsp()` maintains
    // invariant gs:[0x00] == kernel stack of current thread.
    data.kernel_rsp = kernel_stack_top;
    data.online = true;
    data.bsp = true;

    // Point GS_BASE to physmap alias of this structure.
    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR_GS_BASE write from Ring 0
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
    }
    // MSR_KERNEL_GS_BASE contains userspace "shadow" GS.
    // At boot no valid userspace TLS exists yet, so initial value
    // must remain zero and specifically not point to per-CPU area.
    // SAFETY: MSR_KERNEL_GS_BASE write
    unsafe {
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

/// Initializes per-CPU data for an AP
///
/// Called from AP trampoline, before scheduler activation.
pub fn init_percpu_for_ap(cpu_id: u32, kernel_stack_top: u64, lapic_id: u32) {
    if cpu_id as usize >= MAX_CPUS {
        return;
    }
    // SAFETY: unique cpu_id, AP initializes its own data
    let data = unsafe { per_cpu_mut(cpu_id as usize) };
    data.cpu_id = cpu_id as u64;
    data.lapic_id = lapic_id as u64;
    // NOTE P1-5: initial value for bootstrap AP thread.
    // Scheduler refreshes this slot subsequently via `set_kernel_rsp()`.
    data.kernel_rsp = kernel_stack_top;
    data.online = true;
    data.bsp = false;

    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR writes from Ring 0 on current AP
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

// ── Preemption ────────────────────────────────────────────────────────────────

/// Disables preemption (increments preempt_count)
#[inline]
#[deprecated(note = "utiliser scheduler::core::preempt::PreemptGuard")]
pub fn preempt_disable() {
    crate::scheduler::core::preempt::preempt_disable_for_arch_compat();
}

/// Enables preemption (decrements preempt_count)
#[inline]
#[deprecated(note = "utiliser le Drop de scheduler::core::preempt::PreemptGuard")]
pub fn preempt_enable() {
    crate::scheduler::core::preempt::preempt_enable_for_arch_compat();
}

/// Returns `true` if preemption is disabled on current CPU
#[inline]
#[deprecated(note = "utiliser scheduler::core::preempt::is_preempt_disabled")]
pub fn preempt_is_disabled() -> bool {
    preempt_is_disabled_canonical()
}

/// Canonical read of preemption state.
#[inline]
pub fn preempt_is_disabled_canonical() -> bool {
    crate::scheduler::core::preempt::is_preempt_disabled()
}

// ── P2-7 Preparation: kernel_rsp template test ───────────────────────────────

#[cfg(test)]
mod p2_7_gs_tests {
    use super::*;

    /// P2-7 / Test 1 — gs:[0x20] current_tcb: write then read back.
    ///
    /// Requires Ring 0 + initialized per-CPU GS → ignored on host.
    /// Run under QEMU via kernel harness.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 + GS requis")]
    fn current_tcb_write_read_roundtrip() {
        let fake_tcb_a: u64 = 0xFFFF_8000_DEAD_0000;
        let fake_tcb_b: u64 = 0xFFFF_8000_BEEF_0000;

        unsafe {
            set_current_tcb(fake_tcb_a as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_eq!(
                read_current_tcb(),
                fake_tcb_a,
                "gs:[0x20] doit refléter la valeur écrite (TCB thread A)"
            );

            set_current_tcb(fake_tcb_b as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_eq!(
                read_current_tcb(),
                fake_tcb_b,
                "gs:[0x20] doit changer après switch vers thread B"
            );
        }
    }

    /// P2-7 / Test 2 — gs:[0x00] kernel_rsp: invariant updated upon switch.
    ///
    /// Requires Ring 0 + initialized per-CPU GS → ignored on host.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 + GS requis")]
    fn kernel_rsp_updated_on_switch() {
        let kstack_a: u64 = 0xFFFF_8000_0000_0000;
        let kstack_b: u64 = 0xFFFF_8000_0001_0000;
        let fake_tcb: u64 = 0xFFFF_8000_ABCD_0000;

        unsafe {
            set_kernel_rsp(kstack_a);
            assert_eq!(
                read_kernel_rsp(),
                kstack_a,
                "gs:[0x00] doit refléter kstack du thread A"
            );

            set_kernel_rsp(kstack_b);
            assert_eq!(
                read_kernel_rsp(),
                kstack_b,
                "gs:[0x00] doit changer après switch vers thread B"
            );

            // Verify that both slots are independent
            set_current_tcb(fake_tcb as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_ne!(
                read_kernel_rsp(),
                read_current_tcb(),
                "gs:[0x00] et gs:[0x20] sont des slots distincts"
            );
        }
    }
}
