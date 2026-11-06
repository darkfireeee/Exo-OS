// kernel/src/process/lifecycle/exec.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// execve() — Replacing process image (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE PROC-01: process/ (layer 1.5) cannot import fs/ (layer 3).
// SOLUTION: ElfLoader trait registered by fs/ at boot.
//
// Sequence do_execve():
//   1. Check permissions (creds).
//   2. Call ElfLoader::load_elf() to load binary.
//   3. Close O_CLOEXEC fds.
//   4. Reset virtual address space.
//   5. Update TCB (entry_point, initial_rsp, TLS).
//   6. Update PCB (flags, counters).
//   7. Reset signals (handlers → SIG_DFL).
//   8. Return to userspace.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::memory::virt::UserAddressSpace;
use crate::memory::virt::VmaFlags;
use crate::memory::VirtAddr;
use crate::process::core::pcb::{process_flags, ProcessControlBlock, ProcessState};
use crate::process::core::tcb::{ProcessThread, ThreadAddress};
use crate::process::lifecycle::fork::notify_vfork_completion;
use crate::process::lifecycle::fork::AddressSpaceCloner;
use crate::process::signal::mask::block_all_except_kill;
use crate::process::signal::mask::reset_signals_on_exec;
use core::sync::atomic::Ordering;
use spin::Once;

#[cfg(target_os = "none")]
fn build_exec_kpti_shadow(kstack_top: u64, cr3: u64) -> Result<u64, ExecError> {
    if !crate::arch::x86_64::spectre::kpti::kpti_enabled() {
        return Ok(0);
    }

    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
    let trampoline_phys =
        crate::memory::core::PhysAddr::new(crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS);
    let user_cr3 = unsafe {
        crate::memory::virt::page_table::kpti_split::build_user_shadow_pml4(
            crate::memory::core::PhysAddr::new(cr3),
            cpu_id,
            trampoline_phys,
            kstack_top,
        )
    }
    .map_err(|_| ExecError::OutOfMemory)?;

    Ok(user_cr3.as_u64())
}

#[cfg(not(target_os = "none"))]
fn build_exec_kpti_shadow(_kstack_top: u64, _cr3: u64) -> Result<u64, ExecError> {
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait ElfLoader — RULE PROC-01: fs/ layer abstraction
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters resulting from ELF loading.
#[derive(Debug, Clone)]
pub struct ElfLoadResult {
    /// Entry address (entry point of the binary).
    pub entry_point: u64,
    /// Initial stack pointer (RSP at startup).
    pub initial_stack_top: u64,
    /// First userspace argument (RDI) injected at entry point.
    pub entry_arg0: u64,
    /// Base of loaded static TLS (.tdata/.tbss segment).
    pub tls_base: u64,
    /// Size of static TLS.
    pub tls_size: usize,
    /// Start address of brk heap (just after .bss segment).
    pub brk_start: u64,
    /// Physical address (CR3) of the newly created address space.
    pub cr3: u64,
    /// Opaque pointer to UserAddressSpace created by fs/.
    pub addr_space_ptr: usize,
    /// Virtual address of mapped SignalTcb (0 if absent) — for PROC-VMA/V-17.
    pub signal_tcb_vaddr: u64,
}

/// Errors returned by ElfLoader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfLoadError {
    /// File not found.
    NotFound,
    /// Invalid or corrupted ELF format.
    InvalidElf,
    /// Permission denied.
    PermissionDenied,
    /// Not enough memory to load.
    OutOfMemory,
    /// Unsupported architecture.
    UnsupportedArch,
    /// Interpreter (PT_INTERP) not found.
    InterpreterNotFound,
}

/// execve errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// ELF unloadable (wrapper).
    ElfLoadFailed(ElfLoadError),
    /// SCM / capability denied.
    PermissionDenied,
    /// Module signature invalid or missing (FIX-EXEC-SIG).
    SignatureVerificationFailed,
    /// Argument list too long (E2BIG).
    ArgListTooLong,
    /// File name too long.
    NameTooLong,
    /// Process already exiting.
    ProcessExiting,
    /// Multi-threaded exec denied until THREAD-06 terminates other threads.
    ThreadGroupNotSingle,
    /// Kernel allocation failed after ELF loading.
    OutOfMemory,
    /// No ElfLoader registered.
    NoLoader,
}

/// Trait that `fs/` must implement and register via `register_elf_loader()`.
///
/// The process/ layer does NOT know fs/ types — only this trait.
pub trait ElfLoader: Send + Sync {
    /// Loads an ELF from the given path into current address space.
    ///
    /// # Arguments
    /// * `path`   — absolute path in VFS.
    /// * `argv`   — argument vector (argv[0] = binary).
    /// * `envp`   — environment variables.
    /// * `cr3_in` — CR3 of existing address space to reset.
    ///
    /// # Returns
    /// `ElfLoadResult` describing populated address space.
    fn load_elf(
        &self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
        cr3_in: u64,
    ) -> Result<ElfLoadResult, ElfLoadError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Global ELF loader registry
// ─────────────────────────────────────────────────────────────────────────────

static ELF_LOADER: Once<&'static dyn ElfLoader> = Once::new();
pub type CloseExecHandlesHook = fn(pid: u32, handles: &[u64]);
static CLOSE_EXEC_HANDLES_HOOK: Once<CloseExecHandlesHook> = Once::new();

/// Registers ELF loader (called once from fs/ at boot).
///
/// # Safety
/// `loader` must have `'static` lifetime (typically a reference
/// to a statically allocated object in fs/).
pub fn register_elf_loader(loader: &'static dyn ElfLoader) {
    ELF_LOADER.call_once(|| loader);
}

pub fn register_close_exec_handles_hook(hook: CloseExecHandlesHook) {
    let _ = CLOSE_EXEC_HANDLES_HOOK.call_once(|| hook);
}

#[inline]
fn close_exec_handles(pid: u32, handles: &[u64]) {
    if let Some(hook) = CLOSE_EXEC_HANDLES_HOOK.get() {
        hook(pid, handles);
    }
}

/// Loads an ELF binary for userspace bootstrap, before a calling thread
/// exists. This path is only used to build PID 1.
pub fn load_elf_for_boot(
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<ElfLoadResult, ExecError> {
    if path.len() > 4096 {
        return Err(ExecError::NameTooLong);
    }
    let total_arg_len: usize = argv.iter().map(|s| s.len() + 1).sum::<usize>()
        + envp.iter().map(|s| s.len() + 1).sum::<usize>();
    if total_arg_len > 128 * 1024 {
        return Err(ExecError::ArgListTooLong);
    }

    let loader = ELF_LOADER.get().ok_or(ExecError::NoLoader)?;
    loader
        .load_elf(path, argv, envp, 0)
        .map_err(ExecError::ElfLoadFailed)
}

// ─────────────────────────────────────────────────────────────────────────────
// do_execve — main implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Executes a new ELF binary in the context of the current thread.
///
/// This function is called from the execve() syscall after validation
/// of user parameters.
///
/// # Safety
/// `thread` must point to the calling thread's ProcessThread.
/// The address space will be rebuilt — no userspace access after the call.
pub fn do_execve(
    thread: &mut ProcessThread,
    pcb: &ProcessControlBlock,
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<(), ExecError> {
    // Verify that the process is not exiting.
    if pcb.is_exiting() {
        return Err(ExecError::ProcessExiting);
    }
    if pcb.thread_count.load(Ordering::Acquire) != 1 {
        return Err(ExecError::ThreadGroupNotSingle);
    }

    // Validate path and argument length.
    if path.len() > 4096 {
        return Err(ExecError::NameTooLong);
    }
    let total_arg_len: usize = argv.iter().map(|s| s.len() + 1).sum::<usize>()
        + envp.iter().map(|s| s.len() + 1).sum::<usize>();
    if total_arg_len > 128 * 1024 {
        return Err(ExecError::ArgListTooLong);
    }

    // Get ELF loader (registered from fs/ at boot).
    let loader = ELF_LOADER.get().ok_or(ExecError::NoLoader)?;
    let saved_signal_mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
    let old_as_ptr = pcb.address_space.load(Ordering::Acquire);
    let old_process_flags = pcb.flags.load(Ordering::Acquire);
    let old_as_is_vfork_shared = old_process_flags & process_flags::VFORK_SHARED_AS != 0;

    // Step 3 (LAC-08 / PROC-03): block ALL signals except SIGKILL/SIGSTOP
    // BEFORE ELF loading. Prevents a signal delivered between load_elf() and
    // reset_signals_on_exec() from invoking the old handler in a
    // partially replaced address space. The calling mask is restored after purging
    // pending signals of the old image.
    block_all_except_kill(&thread.sched_tcb);

    // Load new binary into address space.
    let cr3_current = thread.sched_tcb.cr3_phys;
    let elf_result = match loader.load_elf(path, argv, envp, cr3_current) {
        Ok(result) => result,
        Err(err) => {
            thread
                .sched_tcb
                .signal_mask
                .store(saved_signal_mask, Ordering::Release);
            return Err(ExecError::ElfLoadFailed(err));
        }
    };

    // FIX-EXEC-SIG (Security_Audit_Passe2 §C-01): module signature verification
    // before replacing address space.
    // ElfLoadResult does not directly provide ModuleHeader — verification
    // is done via is_chain_verified() which confirms that the ExoSeal trust chain
    // successfully validated this binary during initial load from ExoFS.
    //
    // In v0.2.0 dev (kernel_a_hash_is_zero()), verification is logged but
    // non-blocking. In production (EXOPHOENIX_REQUIRE_HASHES=1), it is strict.
    if crate::security::is_chain_verified() {
        // Trust chain is active — verify binary is signed.
        // check_chain_of_trust() verifies Ed25519 binary signature via ExoSeal.
        if let Err(_e) = crate::security::check_chain_of_trust() {
            // Log but do not block in dev
            #[cfg(not(feature = "strict_exec_signatures"))]
            {
                // Dev mode: warning only
                crate::arch::x86_64::terminal::debug_write(
                    b"exec: WARNING unsigned binary executed\n",
                );
            }
            #[cfg(feature = "strict_exec_signatures")]
            {
                thread
                    .sched_tcb
                    .signal_mask
                    .store(saved_signal_mask, Ordering::Release);
                return Err(ExecError::SignatureVerificationFailed);
            }
        }
    }
    #[cfg(target_os = "none")]
    unsafe {
        crate::memory::virt::address_space::KERNEL_AS
            .sync_kernel_half_into(crate::memory::core::PhysAddr::new(elf_result.cr3));
    }

    let exec_kpti_user_cr3 =
        match build_exec_kpti_shadow(thread.sched_tcb.kstack_top(), elf_result.cr3) {
            Ok(cr3) => cr3,
            Err(err) => {
                thread
                    .sched_tcb
                    .signal_mask
                    .store(saved_signal_mask, Ordering::Release);
                if elf_result.addr_space_ptr != 0 && elf_result.addr_space_ptr != old_as_ptr {
                    crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER
                        .free_addr_space(elf_result.addr_space_ptr);
                }
                return Err(err);
            }
        };

    // PROC-VMA (V-17): mark SignalTcb VMA as DONTEXPAND (and not DONTCOPY to
    // preserve the SignalTcb page in CoW during a subsequent fork()).
    // Prevents an attacker from using mremap to expand the SignalTcb VMA.
    if elf_result.signal_tcb_vaddr != 0 {
        // SAFETY: addr_space_ptr created by ElfLoader::load_elf() as Box<UserAddressSpace>;
        // we are the sole holder of this AS (old AS has been replaced).
        let addr_space = unsafe { &*(elf_result.addr_space_ptr as *const UserAddressSpace) };
        addr_space.set_vma_flags(
            VirtAddr::new(elf_result.signal_tcb_vaddr),
            VmaFlags::DONTEXPAND,
        );
    }

    // Close O_CLOEXEC fds.
    let closed_handles = {
        let mut files = pcb.files.lock();
        files.close_on_exec()
    };
    let closed_handles = match closed_handles {
        Ok(handles) => handles,
        Err(()) => {
            thread
                .sched_tcb
                .signal_mask
                .store(saved_signal_mask, Ordering::Release);
            if elf_result.addr_space_ptr != 0 && elf_result.addr_space_ptr != old_as_ptr {
                crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER
                    .free_addr_space(elf_result.addr_space_ptr);
            }
            return Err(ExecError::OutOfMemory);
        }
    };
    close_exec_handles(pcb.pid.0, &closed_handles);

    // Reset signals and purge signals from old image.
    reset_signals_on_exec(&thread.sched_tcb);
    thread.sig_queue.clear();
    thread.rt_sig_queue.clear();
    thread
        .sched_tcb
        .signal_mask
        .store(saved_signal_mask, Ordering::Release);

    // Update address space in scheduler TCB.
    thread.sched_tcb.cr3_phys = elf_result.cr3;
    // FIX #25 (Audit 6.1) — Free old KPTI shadow PML4 before
    // overwriting it. Without this cleanup, each `fork()`+`execve()` orphans
    // a 4 KiB physical page.
    {
        let old_kpti_cr3 = thread.sched_tcb.kpti_user_cr3();
        if old_kpti_cr3 != 0 && old_kpti_cr3 != exec_kpti_user_cr3 {
            // SAFETY: old shadow PML4 was allocated via
            // `build_user_shadow_pml4` (itself `buddy::alloc_page(ZEROED)`).
            // It is no longer referenced anywhere after this point.
            let _ = crate::memory::physical::allocator::buddy::free_pages(
                crate::memory::core::Frame::containing(crate::memory::core::PhysAddr::new(
                    old_kpti_cr3,
                )),
                0,
            );
        }
    }
    thread.sched_tcb.set_kpti_user_cr3(exec_kpti_user_cr3);
    thread.sched_tcb.fs_base = elf_result.tls_base;
    thread.sched_tcb.user_gs_base = 0;

    // Update thread addresses.
    // FIX P2-03: compute and propagate stack_base and stack_size instead of 0
    const PAGE_SIZE_U64: u64 = crate::memory::core::PAGE_SIZE as u64;
    const DEFAULT_STACK_SIZE: u64 = crate::memory::core::layout::USER_STACK_BOOTSTRAP_SIZE as u64;

    let stack_top = elf_result.initial_stack_top;
    let stack_base = (stack_top.saturating_sub(DEFAULT_STACK_SIZE)) & !(PAGE_SIZE_U64 - 1);
    let stack_size = stack_top.saturating_sub(stack_base);

    thread.addresses = ThreadAddress {
        stack_base, // FIX P2-03
        stack_size, // FIX P2-03
        entry_point: elf_result.entry_point,
        initial_rsp: elf_result.initial_stack_top,
        tls_base: elf_result.tls_base,
        entry_arg0: elf_result.entry_arg0,
        pthread_ptr: 0,
        sigaltstack_base: 0,
        sigaltstack_size: 0,
    };
    thread
        .tls_gs_base
        .store(elf_result.tls_base, Ordering::Release);
    thread.tls_size = elf_result.tls_size;

    // P1-05 / PROC-01: execve() does not replace the kernel stack of current thread,
    // but we explicitly republish this kstack in local arch slots to keep
    // TSS.RSP0 / GS:[0x00] invariant visible and self-documented.
    #[cfg(target_os = "none")]
    unsafe {
        let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
        let kstack_top = thread.sched_tcb.kstack_top();
        crate::memory::virt::address_space::KERNEL_AS
            .sync_kernel_half_into(crate::memory::core::PhysAddr::new(elf_result.cr3));
        crate::arch::x86_64::smp::percpu::set_kernel_rsp(kstack_top);
        crate::arch::x86_64::tss::update_rsp0(cpu_id, kstack_top);
        if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
            crate::arch::x86_64::spectre::kpti::set_current_cr3(elf_result.cr3, exec_kpti_user_cr3);
        }
        crate::arch::x86_64::write_cr3(elf_result.cr3);
    }

    // BUG-04 / PROC-01 fix: write IA32_FS_BASE MSR immediately.
    // do_execve() executes in kernel context on current CPU of thread;
    // new TLS base must be active before first userspace instruction.
    // Without this WRMSR, %fs points to old base -> userspace crash upon
    // first TLS access (thread_local!, pthread_self...).
    #[cfg(target_os = "none")]
    unsafe {
        // SAFETY: WRMSR on MSR_FS_BASE = 0xC000_0100 is always defined on
        // x86_64 (AMD64 v0), and MSR is writable from Ring 0. Context
        // is that of the current thread on this CPU — local effect, no contention.
        crate::arch::x86_64::cpu::msr::write_msr(
            crate::arch::x86_64::cpu::msr::MSR_FS_BASE,
            elf_result.tls_base,
        );
        // execve() starts fresh: do not re-inject old userspace GS.
        crate::arch::x86_64::cpu::msr::write_msr(
            crate::arch::x86_64::cpu::msr::MSR_KERNEL_GS_BASE,
            0,
        );
    }

    // Update PCB with new address space.
    pcb.cr3.store(elf_result.cr3, Ordering::Release);
    pcb.address_space
        .store(elf_result.addr_space_ptr, Ordering::Release);
    pcb.signal_tcb_vaddr
        .store(elf_result.signal_tcb_vaddr, Ordering::Release);
    pcb.brk_start.store(elf_result.brk_start, Ordering::Release);
    pcb.brk_current
        .store(elf_result.brk_start, Ordering::Release);
    pcb.set_name_from_path(path.as_bytes());

    // Remove FORKED/VFORK_SHARED_AS now, but only publish EXEC_DONE/VFORK_DONE
    // after teardown of old address space: vfork parent must not be able to
    // resume during `free_addr_space()`.
    pcb.flags.fetch_and(
        !(process_flags::FORKED | process_flags::VFORK_SHARED_AS),
        Ordering::Release,
    );

    if old_as_ptr != 0 && old_as_ptr != elf_result.addr_space_ptr && !old_as_is_vfork_shared {
        crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER.free_addr_space(old_as_ptr);
    }

    pcb.flags.fetch_or(
        process_flags::EXEC_DONE | process_flags::VFORK_DONE,
        Ordering::Release,
    );

    // Transition to Running.
    pcb.set_state(ProcessState::Running);
    // ExoLedger: immutable trace of each binary load (identity change).
    crate::security::exoledger::exo_ledger_append(crate::security::exoledger::ActionTag::Custom {
        tag: 0x6578_6563, // b"exec"
        data: pcb.pid.0 as u64,
    });
    notify_vfork_completion(pcb.pid);

    Ok(())
}
