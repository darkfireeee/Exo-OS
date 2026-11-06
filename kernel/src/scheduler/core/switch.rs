// kernel/src/scheduler/core/switch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH — Dispatch to switch_asm.s (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULES (GI-02 + Fixes):
//   RULE SWITCH-01: check_signal_pending() READS only — never delivers
//   RULE SWITCH-02: Lazy FPU BEFORE switch, CR0.TS=1 BEFORE ASM (TLA-01)
//   RULE SIGNAL-01: scheduler/ DOES NOT know process::signal::*
//                   It only reads the AtomicBool signal_pending flag of the TCB
//   RULE SWITCH-ASM: switch_asm.s saves rbx,rbp,r12-r15 ONLY (V7-C-02)
//                    NO MXCSR/FCW — handled by XSAVE/XRSTOR in fpu/
//                    CR3 switched in switch_asm BEFORE restoring registers (KPTI)
//   CORR-11: FS/GS base saved via rdmsr/wrmsr in context_switch()
//   V7-C-03: TSS.RSP0 updated after each switch (otherwise IRQ on wrong stack)
//   NO-ALLOC ZONE: no recurring allocation in this hot path
//                  (shadow KPTI built once per TCB if required)
// ═══════════════════════════════════════════════════════════════════════════════

use super::preempt::MAX_CPUS;
use super::task::{CpuId, SchedPolicy, TaskState, ThreadControlBlock};
use crate::arch::x86_64::{
    cpu::{
        features::cpu_features_or_none,
        msr::{
            self, MSR_FS_BASE, MSR_IA32_PL0_SSP, MSR_IA32_PRED_CMD, MSR_KERNEL_GS_BASE,
            PRED_CMD_IBPB,
        },
        tsc,
    },
    smp::percpu,
    tss,
};
use crate::memory::core::PhysAddr;
use crate::memory::virt::page_table::kpti_split::{build_user_shadow_pml4, user_cr3_for_cpu};
use crate::scheduler::fpu;
use core::sync::atomic::{AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// "Current thread" pointer per CPU — updated on each context switch
// ─────────────────────────────────────────────────────────────────────────────

/// Cross-CPU array: publishes the current TCB of each CPU after a switch.
///
/// Contract:
/// - local read: prefer `percpu::read_current_tcb()` via GS:[0x20]
/// - remote read: use this array with Release/Acquire pair
/// - publication: performed AFTER updating local GS slots
pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_CPUS];

/// Optional hook called on the outgoing thread before state transition.
///
/// Upper layers install it at boot; scheduler only keeps a
/// function pointer to avoid a direct dependency on security/.
pub type ContextSwitchOutHook = fn(&ThreadControlBlock);

static CONTEXT_SWITCH_OUT_HOOK: AtomicUsize = AtomicUsize::new(0);
#[cfg(all(target_arch = "x86_64", debug_assertions))]
#[allow(dead_code)]
static IDLE_SCHED_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(target_arch = "x86_64", debug_assertions))]
#[inline]
fn idle_sched_trace(_message: &[u8]) {
    // Debug tracing disabled to avoid terminal flood
}

#[cfg(not(all(target_arch = "x86_64", debug_assertions)))]
#[inline]
fn idle_sched_trace(_message: &[u8]) {}

#[inline]
pub fn install_context_switch_out_hook(hook: ContextSwitchOutHook) {
    CONTEXT_SWITCH_OUT_HOOK.store(hook as usize, Ordering::Release);
}

/// Returns raw pointer to current thread's TCB on this CPU.
///
/// Reads canonical per-CPU slot `gs:[0x20]` initialized by `percpu::set_current_tcb()`.
///
/// # Safety
/// The pointer is non-null if scheduler is initialized and a thread is running.
#[inline]
pub fn current_thread_raw() -> *mut ThreadControlBlock {
    // BUG-C2A FIX: read from per-CPU gs:[0x20], not index 0.
    // During early boot phases, the GS slot may still be null while canonical
    // cross-CPU publication has already occurred.
    // We then fall back to CURRENT_THREAD_PER_CPU[cpu] to avoid false null
    // during BSP/AP seam.
    // SAFETY: read_current_tcb() reads TCB pointer from gs:[0x20] of current CPU
    // via Ring 0 MSR/segment access. No dereference here — we only convert the
    // published pointer to usize to test for null.
    let gs_tcb = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() as usize };
    if gs_tcb != 0 {
        return gs_tcb as *mut ThreadControlBlock;
    }

    let cpu_id = percpu::current_cpu_id() as usize;
    if cpu_id >= MAX_CPUS {
        return core::ptr::null_mut();
    }

    CURRENT_THREAD_PER_CPU[cpu_id].load(Ordering::Acquire) as *mut ThreadControlBlock
}

/// Blocks current thread.
///
/// To be called AFTER inserting thread into a wait queue (futex bucket,
/// IpcWaitQueue, etc.) to guarantee a wakeup is not missed.
///
/// The thread resumes from this call after `wake_enqueue()` is called
/// on its TCB by the waking entity.
///
/// # Safety
/// - Preemption must be disabled or caller must guarantee that a wakeup
///   cannot arrive between queue insertion and this call.
/// - Thread must have a wakeup mechanism in place (waiter.woken, etc.).
pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
        "block_current_thread: appelé avec PreemptGuard actif"
    );

    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        // Scheduler not yet initialized — short spin.
        for _ in 0..1_000 {
            core::hint::spin_loop();
        }
        return;
    }

    let tcb = &mut *tcb_ptr;
    let cpu_id = tcb.current_cpu();
    if (cpu_id.0 as usize) < MAX_CPUS {
        let rq = run_queue(cpu_id);
        match tcb.state() {
            TaskState::Runnable => {
                // SAFETY: tcb comes from current_thread_raw() (verified non-null)
                // and rq is the run-queue of the CPU owning this thread. Preemption
                // is disabled by the contract of block_current_thread() (unsafe fn).
                unsafe {
                    finish_preblock_wake(rq, tcb);
                }
                return;
            }
            TaskState::Running => return,
            TaskState::Sleeping
            | TaskState::Uninterruptible
            | TaskState::Stopped
            | TaskState::Dead => {}
            state => {
                debug_assert!(
                    false,
                    "block_current_thread: état inattendu {:?}; l'appelant doit transitionner le thread avant blocage",
                    state
                );
                return;
            }
        }
        schedule_block(rq, tcb);
        // Thread resumes here after wake_enqueue() is called.
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ASM context switch — embedded via global_asm! (switch_asm.s)
//
// Including full .s file in the Rust compilation unit avoids needing a
// build.rs/cc to compile an external .s. The linker sees the symbols
// `context_switch_asm` and `switch_to_new_thread` directly in the produced rlib.
// ─────────────────────────────────────────────────────────────────────────────
core::arch::global_asm!(include_str!("../asm/switch_asm.s"), options(att_syntax));

// ─────────────────────────────────────────────────────────────────────────────
// FFI to context switch ASM
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    /// Full ASM context switch.
    ///
    /// Saves/restores only ABI callee-saved registers (rbx, rbp, r12-r15)
    /// and outgoing thread kernel stack. FPU state (XSAVE/XRSTOR, including
    /// MXCSR and x87 FCW) is handled by `scheduler::fpu`.
    /// CR3 is switched if `new_cr3 != 0`.
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp`: `*mut u64` pointing to outgoing thread's `TCB::kstack_ptr`
    /// - `new_kernel_rsp`: value of incoming thread's `TCB::kstack_ptr`
    /// - `new_cr3`: CR3 register of incoming thread (0 = no CR3 switch)
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal pending check — HOT PATH, <= 5 cycles
// ─────────────────────────────────────────────────────────────────────────────

/// Checks if a signal is pending on this thread.
///
/// RULE SWITCH-01:
///   - READS only the `signal_pending` flag of the TCB.
///   - DOES NOT deliver signals — delivery occurs from arch/syscall.rs
///     or arch/exceptions.rs upon return to userspace.
///   - DOES NOT know `process::signal::*`.
///
/// On scheduler hot path, Ordering::Relaxed is correct: view coherence
/// will be established by the Acquire on return to userspace in arch/.
#[inline(always)]
pub fn check_signal_pending(tcb: &ThreadControlBlock) -> bool {
    tcb.has_signal_pending()
}

// ─────────────────────────────────────────────────────────────────────────────
// context_switch() — Rust entry point for switch
// ─────────────────────────────────────────────────────────────────────────────

/// Performs context switch from `prev` to `next`.
///
/// # Sequence (Full GI-02)
/// 1. Lazy FPU: if `prev` used FPU -> XSAVE (without touching MXCSR/FCW V7-C-02).
/// 2. Restore incoming FPU/SIMD state and keep CR0.TS clear for Ring 3.
/// 3. Save PKRS (Intel PKS).
/// 4. Save CET PL0_SSP if shadow stack active.
/// 5. Save FS.base and user_gs_base via rdmsr (CORR-11).
/// 6. Mark `prev` -> Runnable.
/// 7. Compute `next_cr3` and account for runtime of `prev`.
/// 8. Call `context_switch_asm(prev_rsp_ptr, next_rsp, next_cr3)`.
///    ASM saves/restores 6 callee-saved GPRs. CR3 switched if different.
/// 9. Restore PKRS of `next`.
/// 10. Restore CET PL0_SSP of `next` if shadow stack active.
/// 11. Mark `next` -> Running, then update kernel stack/TSS.RSP0.
/// 12. Restore FS.base then user_gs_base, finally publish GS current_tcb.
///
/// # Safety
/// - Called with preemption disabled (IrqGuard or PreemptGuard).
/// - `prev` and `next` MUST be valid pointers.
///
/// # ABSOLUTE RULE
/// This function MUST NEVER call `process::signal::*`.
/// It only reads `signal_pending` via `check_signal_pending()`.
pub unsafe fn context_switch(prev: &mut ThreadControlBlock, next: &mut ThreadControlBlock) {
    let features = cpu_features_or_none();
    let has_pks = features.is_some_and(|cpu| cpu.has_pks());
    let has_cet_ss = features.is_some_and(|cpu| cpu.has_cet_ss());
    // FIX-IBPB (Security_Audit_Passe2 §B-01): IBPB (Indirect Branch Predictor Barrier)
    // must be issued during cross-process context switch to prevent Spectre v2.
    // Gated on:
    //   1. CPU supporting IBPB (has_ibpb via CPUID SPEC_CTRL or IBPB_AMD)
    //   2. Process change (prev.pid != next.pid) — intra-process switches
    //      (threads of the same process) do not need IBPB.
    //   3. Ring 3 processes only (pid != 0; kthreads share Ring 0 BTB).
    let ibpb_needed =
        features.is_some_and(|cpu| cpu.has_ibpb()) && prev.pid != next.pid && next.pid.0 != 0;
    if ibpb_needed {
        // SAFETY: MSR_IA32_PRED_CMD (0x49) is guaranteed present by has_ibpb().
        // Write is atomic and visible on local CPU only.
        // PRED_CMD_IBPB (bit 0) flushes indirect branch predictor.
        unsafe { msr::write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB) };
    }

    // ── Step 1: outgoing thread FPU/SIMD save ────────────────────────────────
    // Ring 3 Rust binaries use SSE for struct copying even without explicit floats.
    // A lazy mode relying on #NM makes the initial shell fragile;
    // therefore we save/restore upon switch.
    if prev.fpu_loaded() {
        fpu::save_restore::xsave_current(prev);
    }
    prev.set_fpu_loaded(false);
    // SAFETY: Ring 0, preemption disabled (contract of context_switch, unsafe fn).
    // cr0_clear_ts() clears CR0.TS to enable SSE/AVX; xrstor_for(next)
    // restores FPU state of `next` from its aligned per-thread XSAVE area.
    unsafe {
        fpu::lazy::cr0_clear_ts();
        if next.is_kthread() {
            next.set_fpu_loaded(false);
        } else {
            fpu::save_restore::xrstor_for(next);
        }
    }

    // ── Step 3: Save PKRS (S6) if CPU supports PKS ─────────────────────────
    if has_pks {
        // SAFETY: Ring 0 MSR access, capability verified via CPUID.
        prev.pkrs = unsafe { msr::read_msr(msr::MSR_IA32_PKRS) as u32 };
    }

    // ── Step 4: Save MSR_IA32_PL0_SSP if CET Shadow Stack active ────────────
    //
    // Intel SDM Vol.1 §8.3.3: in software task switch mode, saving the
    // Shadow Stack Pointer is kernel responsibility. If CET is inactive,
    // MSR 0x6A4 does not exist -> short-circuit via has_cet_ss().
    if has_cet_ss {
        // SAFETY: MSR 0x6A4 exists if has_cet_ss() == true. Ring 0.
        let ssp = unsafe { msr::read_msr(MSR_IA32_PL0_SSP) };
        prev.set_pl0_ssp(ssp);
    }

    // ── Step 5: Save outgoing thread FS/GS base (CORR-11) ───────────────────
    //
    // In Ring 0 -> SWAPGS executed on kernel entry.
    //   MSR_FS_BASE        (0xC0000100) = current FS.base (prev userspace TLS)
    //   MSR_KERNEL_GS_BASE (0xC0000102) = shadow GS.base (prev userspace value)
    //     [current GS.base (0xC0000101) = per-CPU kernel data — do not save]
    //
    // SILENT ERROR S-06: saving 0xC0000101 instead of 0xC0000102
    //   -> Ring 3 TLS corrupted after each context switch between different threads.
    //
    // SAFETY: rdmsr in Ring 0 on valid and supported MSRs (x86_64 required).
    prev.fs_base = unsafe { msr::read_msr(MSR_FS_BASE) };
    prev.user_gs_base = unsafe { msr::read_msr(MSR_KERNEL_GS_BASE) };

    let hook = CONTEXT_SWITCH_OUT_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        // SAFETY: hook != 0 guarantees a valid function pointer was published
        // via install_context_switch_out_hook() (signature ContextSwitchOutHook).
        // usize -> fn pointer transmutation is sound because address originates
        // from a `fn` of the same ABI stored atomically.
        let hook_fn: ContextSwitchOutHook = unsafe { core::mem::transmute(hook) };
        hook_fn(prev);
    }

    // ── Step 6: State transition of prev ────────────────────────────────────
    // If outgoing thread was Running -> becomes Runnable (will be re-enqueued).
    // If it was in a blocking state (Sleeping, Uninterruptible) -> leave unchanged.
    let prev_state = prev.state();
    if prev_state == TaskState::Running {
        prev.set_state(TaskState::Runnable);
    }

    // ── Step 7: prepare CR3 + runtime, then Step 8: ASM context switch ──────
    // FIX-CR3-RESYNC: ALWAYS reload target thread CR3 (if it has one,
    // i.e. user thread). The old optimization "skip if prev.cr3 == next.cr3"
    // assumed hardware CR3 was already prev.cr3_phys — false once a path
    // (execve/handoff/fork-trampoline) changes hardware CR3 without going through
    // this switch. Observed result: init (PID 1) ran on a CR3 != its
    // `tcb.cr3_phys`, so demand-paging mapped into wrong table ->
    // #PF instruction-fetch loop. `tcb.cr3_phys` is the ground truth
    // (updated by execve); hardware must follow on each switch.
    // new_cr3 == 0 => ASM does not reload (kernel threads without user space).
    let new_cr3 = if next.cr3_phys != 0 { next.cr3_phys } else { 0 };

    // Account for actual time spent in Running by `prev`.
    let now_tsc = tsc::read_tsc();
    let cpu_idx = percpu::current_cpu_id() as usize;
    if cpu_idx < MAX_CPUS {
        // SAFETY: cpu_idx < MAX_CPUS bounds access to static per-CPU array;
        // each core writes only its own slot (no cross-CPU mutable sharing), so
        // &mut is exclusive on this CPU. Preemption disabled.
        let cpu_data = unsafe { percpu::per_cpu_mut(cpu_idx) };
        let last = cpu_data.last_switch_tsc;
        if last != 0 {
            let delta_ns = tsc::tsc_cycles_to_ns(now_tsc.wrapping_sub(last));
            prev.run_time_acc = prev.run_time_acc.saturating_add(delta_ns);
            match prev.policy {
                SchedPolicy::Normal | SchedPolicy::Batch => {
                    prev.advance_vruntime(delta_ns, prev.priority.cfs_weight());
                }
                _ => {}
            }
        }
    }

    // Publish `next` BEFORE ASM jump.
    //
    // A never-scheduled thread does not return to instruction following
    // `context_switch_asm`: its first `ret` jumps to its startup trampoline.
    // Per-CPU state must therefore already designate `next` before loading
    // its kernel stack.
    next.assign_cpu(CpuId(cpu_idx as u32));

    let next_kstack_top = next.kstack_top();

    if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        let mut user_cr3 = next.kpti_user_cr3();
        if user_cr3 == 0 && next.pid.0 != 0 && next.cr3_phys != 0 {
            let trampoline_phys = PhysAddr::new(crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS);
            // SAFETY: builds KPTI shadow PML4 for `next` from its kernel CR3
            // (non-null, pid != 0 checked above), SMP physical trampoline, and
            // valid kernel stack top. Ring 0, mappings controlled by table walker;
            // failure -> halt CPU.
            user_cr3 = unsafe {
                match build_user_shadow_pml4(
                    PhysAddr::new(next.cr3_phys),
                    cpu_idx,
                    trampoline_phys,
                    next_kstack_top,
                ) {
                    Ok(cr3) => cr3.as_u64(),
                    Err(_err) => {
                        crate::arch::x86_64::terminal::debug_write(
                            b"KPTI: echec construction user shadow\n",
                        );
                        crate::arch::x86_64::halt_cpu();
                    }
                }
            };
            next.set_kpti_user_cr3(user_cr3);
        }
        if user_cr3 == 0 {
            user_cr3 = user_cr3_for_cpu(cpu_idx).unwrap_or(next.cr3_phys);
        }
        if user_cr3 == 0 {
            crate::arch::x86_64::terminal::debug_write(b"KPTI: user CR3 absent\n");
            crate::arch::x86_64::halt_cpu();
        }
        crate::arch::x86_64::spectre::kpti::set_current_cr3(next.cr3_phys, user_cr3);
    }

    if has_pks {
        // SAFETY: Ring 0 MSR access, capability verified via CPUID.
        unsafe { msr::write_msr(msr::MSR_IA32_PKRS, next.pkrs as u64) };
    }

    if has_cet_ss {
        let ssp = next.pl0_ssp();
        // SAFETY: MSR 0x6A4 exists if has_cet_ss(), Ring 0, per-thread value.
        unsafe { msr::write_msr(MSR_IA32_PL0_SSP, ssp) };
    }

    next.set_state(TaskState::Running);
    next.switch_count = next.switch_count.wrapping_add(1);

    // SAFETY: next_kstack_top is kernel stack top of `next` (allocated at
    // spawn). We publish this RSP0 in per-CPU state and TSS of current core
    // (cpu_idx < MAX_CPUS) so next Ring3->Ring0 entry uses the right stack.
    // Ring 0, preemption disabled.
    unsafe {
        percpu::set_kernel_rsp(next_kstack_top);
        tss::update_rsp0(cpu_idx, next_kstack_top);
    }

    // SAFETY: wrmsr in Ring 0 on valid and supported MSRs.
    unsafe {
        msr::write_msr(MSR_FS_BASE, next.fs_base);
        msr::write_msr(MSR_KERNEL_GS_BASE, next.user_gs_base);
    }

    // Publish only after FS then user-GS, conforming to ContextSwitch.tla.
    core::sync::atomic::fence(Ordering::SeqCst);
    percpu::set_current_tcb(next as *mut ThreadControlBlock);

    if cpu_idx < MAX_CPUS {
        // SAFETY: cpu_idx < MAX_CPUS; exclusive access to current core's per-CPU
        // slot (no cross-CPU mutable sharing). Ring 0, preemption off.
        let cpu_data = unsafe { percpu::per_cpu_mut(cpu_idx) };
        cpu_data.ctx_switch_count = cpu_data.ctx_switch_count.wrapping_add(1);
        cpu_data.last_switch_tsc = tsc::read_tsc();
        CURRENT_THREAD_PER_CPU[cpu_idx]
            .store(next as *mut ThreadControlBlock as usize, Ordering::Release);
    }

    // SAFETY: prev.kstack_ptr and next.kstack_ptr point to valid kernel stacks,
    // allocated at boot and never freed during thread lifetime.
    // context_switch_asm guarantees complete save of ABI callee-saved registers.
    if next.is_kthread() {
        idle_sched_trace(b"context_switch: ->kthread\n");
    }

    context_switch_asm(&mut prev.kstack_ptr as *mut u64, next.kstack_ptr, new_cr3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Voluntary yield
// ─────────────────────────────────────────────────────────────────────────────

/// Voluntary yield of current thread.
/// Places current thread at end of CFS queue before calling context_switch.
///
/// Called from: sys_sched_yield(), mutex_lock() (contention), condvar_wait().
///
/// # Safety
/// `rq` and `current` must be valid, executed in context of current thread.
pub unsafe fn schedule_yield(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) -> bool {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();

        // Re-enqueue current BEFORE picking next (CFS round-robin).
        // SAFETY: current is a valid mutable reference, non-null by construction.
        if current.is_queued() {
            let _ = rq.remove(ptr);
            current.clear_queued();
        }
        current.set_state(TaskState::Runnable);
        if !rq.enqueue(ptr) {
            current.set_state(TaskState::Running);
            return false;
        }

        let selected = match pick_next_task(rq, None) {
            PickResult::Switch(next) if next != ptr => Some(next),
            PickResult::Switch(_) => {
                // Current was chosen and dequeued. If another thread is ready,
                // pick it and re-enqueue current behind it; otherwise yield is a no-op.
                let replacement = if rq.nr_running_usize() > 0 {
                    match pick_next_task(rq, None) {
                        PickResult::Switch(next) if next != ptr => Some(next),
                        _ => None,
                    }
                } else {
                    None
                };

                if let Some(next) = replacement {
                    if rq.enqueue(ptr) {
                        Some(next)
                    } else {
                        current.set_state(TaskState::Running);
                        rq.enqueue(next);
                        None
                    }
                } else {
                    current.set_state(TaskState::Running);
                    None
                }
            }
            PickResult::KeepRunning | PickResult::GoIdle => {
                let _ = rq.remove(ptr);
                current.set_state(TaskState::Running);
                None
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        return false;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    // SAFETY: next comes from run queue, always valid.
    let next_ref = &mut *next.as_ptr();
    if !core::ptr::eq(current, next_ref) {
        context_switch(current, next_ref);
        // SAFETY: restores IF flag captured by IrqGuard on entry to
        // critical section; irq_flags is locally saved RFLAGS, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        true
    } else {
        // SAFETY: restores IF flag captured by IrqGuard on entry to
        // critical section; irq_flags is locally saved RFLAGS, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        current.set_state(TaskState::Running);
        false
    }
}

unsafe fn schedule_current(force: bool) -> bool {
    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        return false;
    }

    let current = &mut *tcb_ptr;
    let cpu_id = current.current_cpu();
    if (cpu_id.0 as usize) >= MAX_CPUS {
        return false;
    }

    let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
    let should_schedule = if force {
        // A cooperative yield is an explicit selection request. Do not
        // condition it on nr_running: this counter may be transiently
        // behind the CFS/RT structure during fork/IPC paths.
        true
    } else {
        current.need_resched()
    };
    if !should_schedule {
        return false;
    }

    let _ = current.take_need_resched();
    schedule_yield(rq, current)
}

/// Honors `NEED_RESCHED` set by tick timer or IPI before returning
/// to userspace.
///
/// Helper is intentionally small: only triggers a switch if current TCB
/// explicitly requests preemption.
///
/// # Safety
/// CPL 0, interrupt context or system call exit.
pub unsafe fn schedule_current_if_needed() -> bool {
    schedule_current(false)
}

/// Cooperative rescheduling point for syscalls spinning in short loops
/// while true sleep queues are not yet wired.
///
/// # Safety
/// CPL 0, called outside non-preemptible critical sections.
pub unsafe fn cooperative_reschedule() -> bool {
    schedule_current(true)
}

/// A scheduling attempt from current CPU's boot idle thread.
///
/// `kernel_main` arrives here on BSP boot stack, with idle TCB already
/// published as `current_tcb`. End of boot must not do `cli; hlt`,
/// otherwise already enqueued kthreads and future user processes
/// can never acquire the CPU. This helper picks a runnable thread, releases
/// preempt section before switch, then transitions from current idle TCB.
///
/// Returns `true` if context switch was attempted.
///
/// # Safety
/// Scheduler and CPU run queue must be initialized, and current TCB
/// must be the published idle TCB for this CPU or a valid kernel TCB.
pub unsafe fn schedule_idle_cpu_once(cpu_id: CpuId) -> bool {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use crate::scheduler::core::runqueue::run_queue;
    use core::ptr::NonNull;

    let current_ptr = current_thread_raw();
    let Some(current_nn) = NonNull::new(current_ptr) else {
        idle_sched_trace(b"idle_sched: no current\n");
        return false;
    };
    idle_sched_trace(b"idle_sched: enter\n");

    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();
        let rq = run_queue(cpu_id);
        let current = &mut *current_nn.as_ptr();

        if rq.nr_running_usize() == 0 && !current.need_resched() {
            idle_sched_trace(b"idle_sched: empty\n");
            return false;
        }
        let _ = current.take_need_resched();

        let selected = match pick_next_task(rq, Some(current_nn)) {
            PickResult::Switch(next) if next != current_nn => {
                idle_sched_trace(b"idle_sched: picked\n");
                Some(next)
            }
            PickResult::Switch(_) | PickResult::KeepRunning | PickResult::GoIdle => {
                idle_sched_trace(b"idle_sched: no next\n");
                None
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        return false;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    let current = &mut *current_nn.as_ptr();
    idle_sched_trace(b"idle_sched: switch\n");
    context_switch(current, &mut *next.as_ptr());
    // SAFETY: restores IF flag captured by IrqGuard on entry to
    // critical section; irq_flags is locally saved RFLAGS, Ring 0.
    unsafe {
        IrqGuard::restore_irq_flags(irq_flags);
    }
    true
}
// ─────────────────────────────────────────────────────────────────────────────
// schedule_block — blocking current thread (without re-enqueuing)
// ─────────────────────────────────────────────────────────────────────────────

/// Completes a wakeup race that arrived before actual blocking.
///
/// Sleep/wait protocol is two-phase: caller first publishes thread as
/// `Sleeping`, then calls `schedule_block()`. A timer or waker can legally
/// perform `Sleeping -> Runnable` and enqueue it between these two operations.
/// Since thread is still running on this CPU, the runqueue entry must be removed
/// and state returned to `Running` instead of blocking.
///
/// # Safety
/// `current` must be current TCB of CPU associated with `rq`.
pub unsafe fn finish_preblock_wake(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let current_ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    let _irq = IrqGuard::new();
    let _ = rq.remove(current_ptr);
    current.set_state(TaskState::Running);
}

/// Blocks current thread without re-enqueuing into run queue.
///
/// To be called after inserting thread into a WaitQueue and
/// setting its state to `Sleeping` or `Uninterruptible`.
/// Thread will only be rescheduled when a call to `wake_one`/`wake_all`
/// transitions state to `Runnable` AND re-enqueues it into RQ.
///
/// # Safety
/// - Preemption disabled required (PreemptGuard or IrqGuard).
/// - `current` must already have state set to Sleeping/Uninterruptible.
///   Do NOT call if intending to preserve Running or Runnable state.
pub unsafe fn schedule_block(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    current: &mut ThreadControlBlock,
) {
    use crate::scheduler::core::pick_next::{pick_next_task, PickResult};
    use crate::scheduler::core::preempt::IrqGuard;
    use core::ptr::NonNull;

    let current_ptr = NonNull::new_unchecked(current as *mut ThreadControlBlock);
    match current.state() {
        TaskState::Runnable => {
            finish_preblock_wake(rq, current);
            return;
        }
        TaskState::Running => return,
        TaskState::Sleeping | TaskState::Uninterruptible | TaskState::Stopped | TaskState::Dead => {
        }
        _ => return,
    }
    let mut idle_thread = match rq.idle_thread {
        Some(idle) => Some(idle),
        None => {
            let recovered = crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0);
            if let Some(idle) = recovered {
                rq.set_idle_thread(idle);
                if rq.current.is_none() {
                    rq.current = Some(idle);
                }
            }
            recovered
        }
    };

    if idle_thread.is_none() {
        for _ in 0..1024 {
            if let Some(idle) = crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0) {
                rq.set_idle_thread(idle);
                if rq.current.is_none() {
                    rq.current = Some(idle);
                }
                idle_thread = Some(idle);
                break;
            }
            core::hint::spin_loop();
        }
    }

    let (selected_next, irq_flags) = {
        let irq = IrqGuard::new();
        if current.is_queued() {
            let _ = rq.remove(current_ptr);
            current.clear_queued();
        }
        let selected = match pick_next_task(rq, Some(current_ptr)) {
            PickResult::Switch(next) if !core::ptr::eq(current, next.as_ptr()) => Some(next),
            PickResult::Switch(_) | PickResult::KeepRunning | PickResult::GoIdle => {
                idle_thread.filter(|idle| !core::ptr::eq(current, idle.as_ptr()))
            }
        };

        if selected.is_some() {
            let flags = irq.release_keep_irqs_disabled();
            (selected, Some(flags))
        } else {
            (None, None)
        }
    };

    let Some(next) = selected_next else {
        current.set_state(TaskState::Running);
        return;
    };
    let irq_flags = irq_flags.unwrap_or(0);

    if !matches!(
        current.state(),
        TaskState::Sleeping | TaskState::Uninterruptible | TaskState::Stopped | TaskState::Dead
    ) {
        // SAFETY: restores IF flag captured by IrqGuard on entry to
        // critical section; irq_flags is locally saved RFLAGS, Ring 0.
        unsafe {
            IrqGuard::restore_irq_flags(irq_flags);
        }
        return;
    }

    // SAFETY: next comes from run queue or published idle TCB for this CPU.
    context_switch(current, &mut *next.as_ptr());
    // SAFETY: restores IF flag captured by IrqGuard on entry to
    // critical section; irq_flags is locally saved RFLAGS, Ring 0.
    unsafe {
        IrqGuard::restore_irq_flags(irq_flags);
    }
}

/// Enqueues a TCB after wakeup from WaitQueue.
/// To be called from `wake_one`/`wake_all` so thread gets rescheduled.
///
/// # Safety
/// Preemption disabled required.
#[inline(always)]
pub unsafe fn wake_enqueue(
    rq: &mut crate::scheduler::core::runqueue::PerCpuRunQueue,
    tcb: core::ptr::NonNull<ThreadControlBlock>,
) {
    use crate::scheduler::core::task::TaskState;
    let tcb_ref = &*tcb.as_ptr();
    let should_enqueue = match tcb_ref.state() {
        TaskState::Sleeping => tcb_ref.try_transition(TaskState::Sleeping, TaskState::Runnable),
        TaskState::Uninterruptible => {
            tcb_ref.try_transition(TaskState::Uninterruptible, TaskState::Runnable)
        }
        TaskState::Stopped => tcb_ref.try_transition(TaskState::Stopped, TaskState::Runnable),
        TaskState::Runnable => true,
        TaskState::Running | TaskState::Zombie | TaskState::Dead => false,
    };
    if should_enqueue {
        rq.enqueue(tcb);
    }
}
