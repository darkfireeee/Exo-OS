//! # Exo-OS Kernel — Public Re-exports
//!
//! `lib.rs` exposes the kernel's public API to external crates and tests.
//! The actual entry point (`_start` → `kernel_main`) is in `main.rs`.
//!
//! ## Layer Architecture (docs/refonte)
//! ```
//! Layer 0   : memory/    — no kernel dependencies
//! Layer 1   : scheduler/ — depends on memory/
//! Layer 1.5 : process/   — depends on memory/ + scheduler/
//! Layer 2a  : security/  — depends on memory/ + scheduler/process bootstrap
//! Layer 2b  : ipc/       — depends on memory/ + scheduler/ + process/ + security/
//! Layer 3   : fs/        — depends on everything except direct ipc/
//! Cross-cutting : arch/  — can call any layer
//! ```
//!
//! ## Absolute Rules
//! - `unsafe` → `// SAFETY:` mandatory (regle_bonus.md)
//! - scheduler/core/ + ISR → NO-ALLOC (regle_bonus.md)
//! - Lock ordering : Memory → Scheduler → Security → IPC → FS (regle_bonus.md)
//! - signal/ ∈ process/ only (DOC1 RULE SIGNAL-01)
//! - capability/ ∈ security/ only (DOC1 RULE CAP-01)
//! - futex ∈ memory/utils/futex_table.rs (DOC3 RULE SCHED-03)
//!

#![cfg_attr(all(target_os = "none", not(kani)), no_std)]
// `#[alloc_error_handler]` remains a nightly API for `no_std` targets.
// Kernel A and Kernel B share this crate; declaring the feature under the same
// condition prevents the bare-metal build from failing before any test #25,
// without exposing it to host/Kani tests.
#![cfg_attr(all(target_os = "none", not(kani)), feature(alloc_error_handler))]
#![allow(binary_asm_labels)]
#![allow(unexpected_cfgs)]
#![allow(clippy::redundant_field_names)]
#![allow(clippy::manual_div_ceil)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::explicit_auto_deref)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::unnecessary_safety_doc)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::declare_interior_mutable_const)]
#![allow(clippy::mut_from_ref)]

#[cfg(all(not(target_arch = "x86_64"), not(test)))]
compile_error!("ExoOS kernel v0.2.0 supporte uniquement x86_64 comme cible de boot.");

// Safeguard against traps: kernel crate unit tests must be
// compiled on a host target (std), not on the bare-metal no_std target.
// Without this guard, `cargo test --target x86_64-unknown-none` produces an
// avalanche of secondary errors (`std`/prelude/macros missing).
#[cfg(all(test, target_os = "none"))]
compile_error!(
    "exo-os-kernel: `cargo test` sur cible bare-metal est non supporté. \
Utiliser une cible hôte pour les tests (ex: --target x86_64-unknown-linux-gnu), \
ou utiliser le check bare-metal via run_tests.sh/Makefile."
);

// ── External Crates (no_std) ──────────────────────────────────────────────────

extern crate alloc;
// Some kernel modules have a local submodule named `core`.
// This alias preserves unambiguous access to the Rust `core` library.
extern crate core as rust_core;

#[cfg(all(test, not(target_os = "none")))]
extern crate std;

// ── Kernel Modules ────────────────────────────────────────────────────────────

/// Cross-cutting layer: architecture-specific code
pub mod arch;

/// Layer 0: physical, virtual memory management, heap, DMA
pub mod memory;

/// Layer 1: scheduler, context switch, scheduling policies
pub mod scheduler;

/// Layer 1.5: processes, threads, signals, lifecycle
pub mod process;

/// Layer 2a: zero-copy IPC, channels, shared memory
pub mod ipc;

/// Layer 2b: capabilities, isolation, integrity
pub mod security;

/// Layer 3: virtual file system + exofs
pub mod fs;

/// GI-03: canonical IRQ/DMA/PCI/IOMMU wrappers
pub mod drivers;

/// ExoPhoenix (Kernel B): SSR shared state + sentinel orchestration.
pub mod exophoenix;

/// Syscall interface → dispatch to upper layers
pub mod syscall;

/// Bootstrap of Ring 1 servers and PID 1.
pub mod userspace_boot;

// ── Public Re-exports ─────────────────────────────────────────────────────────
// Only symbols required by external crates (tests, tools) are exported.
// The kernel_main binary uses these modules directly via `exo_os_kernel::`.

// FIX-LIB-CFG: addition of missing #[cfg(target_arch = "x86_64")] guard.
// Without this guard, accidental removal of compile_error! on line 30
// would produce cryptic symbol resolution errors on AArch64
// rather than a clear message.
#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::{
    // Architecture information
    arch_info,
    // Architecture initialization entry point
    boot::early_init::arch_boot_init,
    // Exposed low-level primitives
    halt_cpu,
    memory_barrier,
    KERNEL_BASE,
    // Constants
    PAGE_SIZE,
};
#[cfg(target_arch = "x86_64")]
pub use arch::ArchInfo;

#[cfg(target_arch = "x86_64")]
pub use arch::x86_64::cpu::{
    features::{CpuFeatures, CPU_FEATURES},
    tsc::read_tsc,
};

/// Layer initialization sequence (called from `kernel_main` in main.rs).
///
/// Follows DOC2 + DOC3 order:
/// 1. arch          (already done prior to this call)
/// 2. memory/       — EmergencyPool FIRST (RULE EMERGENCY-01)
/// 3. scheduler/    — after memory
/// 4. process/      — after scheduler
/// 5. security/     — after process bootstrap (RULE SEC-BOOT-GAP: before IPC)
/// 6. ipc/          — after process + security
/// 7. fs/           — last
///
/// # Safety
/// Must be called exactly once, from the BSP, after `arch_boot_init`.
///
/// DIAG-V11 (bisect OOM run 12 — last_output_13): `<BOOT-FF>` (main.rs, right
/// after returning from this function) shows the buddy pool already at ~3.6% free
/// BEFORE any user ELF loading — the deficit (~3.85 GiB out of ~4 GiB)
/// is thus entirely inside `kernel_init()`. An `<FF-<STAGE>>`
/// is added after each `stage_ok()` below to locate the responsible step(s)
/// on run 13: compare consecutive free= deltas, the first large drop identifies
/// the faulty subsystem.
pub unsafe fn kernel_init(cpu_count: usize) {
    #[inline(always)]
    unsafe fn kdb(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }

    struct IrqStateGuard(u64);

    impl Drop for IrqStateGuard {
        fn drop(&mut self) {
            crate::arch::x86_64::irq_restore(self.0);
        }
    }
    // ── Phase 2a: EmergencyPool — ABSOLUTELY FIRST (RULE EMERGENCY-01) ─────────
    crate::memory::physical::frame::emergency_pool::init();
    kdb(b'2'); // Phase 2a done

    // ── Phase 2b: Heap allocator (SLUB + large) ────────────────────────────
    crate::memory::heap::allocator::hybrid::init();
    crate::memory::cow::init();
    let _ = crate::arch::x86_64::apic::local_apic::init_lapic_fixmap_post_memory();
    kdb(b'3'); // Phase 2b done
    crate::arch::x86_64::boot_display::stage_ok("MEMORY");

    // ── Phase 2c: Time subsystem (HPET + TSC calibration + ktime seqlock) ────────
    // Replaces the 3 direct calls with time_init() which orchestrates:
    //   init_hpet_post_memory() → calibrate_tsc() → pll_init → init_ktime() → clock::init()
    // FIX TIME-02/03: calibration via real time window (loop = HPET ticks).
    // FIX TIME-01   : ktime protected by ISR-safe seqlock.
    crate::arch::x86_64::time::time_init();
    kdb(b'4'); // Phase 2c done
    crate::arch::x86_64::boot_display::stage_ok("TIME");

    // ── Phase 2d: GI-03 drivers (IOMMU queues + kernel notifications) ────────
    crate::drivers::init();
    kdb(b'D'); // Phase 2d done
    crate::arch::x86_64::boot_display::stage_ok("DRIVERS");

    // ── Phase 2e: ExoSeal/ExoCage pre-scheduler (CORR-82 / S-03) ───────────
    // ExoSeal phase0 is idempotent and must lock CET/PKS/IOMMU before
    // runqueues initialization and thread creation.
    unsafe {
        crate::security::exoseal::exoseal_boot_phase0();
    }
    kdb(b'C');
    crate::arch::x86_64::boot_display::stage_ok("EXOSEAL0");

    // ── Phase 2f: cgroup root before runqueues (CORR-77) ─────────────
    crate::process::resource::cgroup::init();
    kdb(b'c'); // cgroup root ready before scheduler runqueues

    // ── Phase 3: Scheduler ───────────────────────────────────────────
    let sched_cpus = if crate::arch::x86_64::smp::init::smp_boot_complete() {
        cpu_count.max(1)
    } else {
        1
    };
    crate::scheduler::init(&crate::scheduler::SchedInitParams {
        nr_cpus: sched_cpus,
        nr_nodes: 1,
    });
    kdb(b'5'); // Phase 3 done

    // ── Phase 3b: Bootstrap idle thread (BSP + APs already online) ─────────
    let _ = crate::scheduler::core::publish_current_boot_idle(
        0,
        crate::arch::x86_64::boot::early_init::boot_stack_top(),
    );
    crate::scheduler::core::bind_boot_idle_threads(sched_cpus);
    kdb(b'6'); // idle thread done

    // ── Phase 3c: Address space cloner registration ───────────────
    // CORRECTION P0-01: register cloner for fork() BEFORE process::init()
    {
        use crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER;
        use crate::process::lifecycle::fork::register_addr_space_cloner;
        register_addr_space_cloner(&KERNEL_AS_CLONER);
    }
    kdb(b'F'); // fork cloner registered
    crate::arch::x86_64::boot_display::stage_ok("SCHEDULER");

    // ── Phase 4: Process ────────────────────────────────────────────────────
    // FIX: the GPF "f000ff53f000ff53" crash observed previously was caused
    // by the LAPIC LVT LINT0 bug (unmasked vector 0x8E, delivered by QEMU BIOS).
    // Fixed by init_local_apic() which masks all LVT entries (commit 40da75e).
    //
    // Additionally, create_kthread() now uses the proper stack frame for
    // context_switch_asm (MXCSR+FCW + 6 regs + kthread_trampoline) instead of
    // the 2 incorrect u64 values that corrupted the stack on first context switch.
    //
    // process::init() orchestrates:
    //   1. pid::init()              — reserves PID 0 (idle) and PID 1 (init)
    //   2. registry::init()         — allocates PCB table (32768 slots)
    //   3. lifecycle::reap::init_reaper() — enqueues reaper kthread
    //   4. state::wakeup::register_with_dma() — registers DMA handler
    //   5. resource::cgroup::init() — initializes root cgroup
    //
    // NOTE: process::init() calls cgroup::init() which references CGROUP_TABLE (~28KB .data).
    // This section is correctly initialized but increases binary size.
    // Only critical subsystems for boot are enabled here:
    // GUARD: disable interrupts during process subsystem init
    // to prevent timer interrupts from triggering a context switch with partially
    // initialized data structures.
    let process_irq_guard = IrqStateGuard(crate::arch::x86_64::irq_save());
    kdb(b'a'); // before process::init
    crate::process::init(&crate::process::ProcessInitParams::default());
    drop(process_irq_guard);
    kdb(b'P'); // Phase 4 done (process init + cgroup root + OOM hooks)
    crate::arch::x86_64::boot_display::stage_ok("PROCESS");

    // ── Phase 5: Security ──────────────────────────────────────────────────
    // If security has already been initialized in early boot (SECURITY_READY=true),
    // do not reinitialize capability/crypto (double init -> panic).
    if !crate::security::is_security_ready() {
        let mut kaslr_entropy = crate::arch::x86_64::cpu::tsc::read_tsc()
            ^ crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR.rotate_left(17);
        let mut rdrand = [0u8; 8];
        if crate::security::crypto::rng::rdrand_fill(&mut rdrand).is_ok() {
            kaslr_entropy ^= u64::from_le_bytes(rdrand);
        }
        crate::security::security_init(
            kaslr_entropy,
            crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR,
        );
    }
    kdb(b'7'); // security done

    // ERR-05 fix: Init SipHash seed of the futex table (anti-DoS hash collision).
    {
        let mut seed = [0u8; 16];
        if crate::security::crypto::rng_fill(&mut seed).is_ok() {
            crate::memory::utils::futex_table::init_futex_seed(seed);
        }
    }
    kdb(b'8'); // futex seed done
    crate::arch::x86_64::boot_display::stage_ok("SECURITY");

    // TIER 2.1-a: runtime integrity monitor kthread (observe mode). Periodically
    // checks `.text/.rodata` once security boot is finished; logs
    // any discrepancy into ExoLedger without panicking (boot-safe). Best-effort.
    crate::security::start_integrity_monitor();

    // ── Phase 5b: ExoPhoenix Stage0 (IOMMU blocking domain) ─────────────
    // FIX-STAGE0 (rapport_analyse_kernel_exo_os.md §4.1):
    // stage0_init_all_steps() was never called, leaving
    // IOMMU_BLOCKED_DOMAIN_ID = 0 (default VT-d identity domain).
    // During an ExoPhoenix handoff, stage_soft_revoke_iommu() flushed
    // domain 0 instead of the dedicated blocking domain → isolation silently inoperative.
    //
    // Must be after security_init() (PhasePhoenix requires the security
    // subsystem for IOMMU capabilities) and before ipc_init() (Phase 6).
    //
    // SAFETY: called once from the BSP, after security_init().
    // FIX-BOOT-STAGE0-HANG: kernel_a_boot=true ⇒ skips Kernel-B steps that
    // clobber BSP CPU state (TSS/IDT/APIC timer), cause of hang after SECURITY.
    let _stage0_summary = crate::exophoenix::stage0::stage0_init_all_steps(true);
    kdb(b'S'); // Stage0 ExoPhoenix done
    crate::arch::x86_64::boot_display::stage_ok("STAGE0");

    // ── Phase 6: IPC ────────────────────────────────────────────────────────
    ipc::ring::spsc::init_spsc_rings();
    // BUG-C2B FIX: reserve a dedicated physical SHM pool before IPC initialization.
    const SHM_POOL_ORDER: usize = 8; // 2^8 pages = 256 pages = 1 MiB
    let shm_pool =
        match crate::memory::alloc_pages(SHM_POOL_ORDER, crate::memory::AllocFlags::ZEROED) {
            Ok(pool) => pool,
            Err(err) => {
                panic!(
                    "ipc shm pool allocation failed: order={} err={:?}",
                    SHM_POOL_ORDER, err
                );
            }
        };
    crate::ipc::ipc_init(
        shm_pool.start_address().as_u64(),
        1, // nr_numa_nodes — to be read from ACPI NUMA if available
    );
    crate::ipc::ipc_install_scheduler_hooks(crate::scheduler::core::switch::block_current_thread);
    // P1-02: install VMM hooks for SHM mapping in process address spaces.
    // Must be done after ipc_init() and before any call to shm_map()
    // (i.e. before starting Ring 1 servers).
    crate::ipc::ipc_install_vmm_hooks(
        crate::arch::x86_64::memory_iface::shm_vmm_map_page,
        crate::arch::x86_64::memory_iface::shm_vmm_unmap_page,
    );
    kdb(b'9'); // IPC done
    crate::arch::x86_64::boot_display::stage_ok("IPC");

    // ── Phase 7: FS ─────────────────────────────────────────────────────────
    let exofs_ready = crate::fs::exofs::exofs_init(
        crate::fs::exofs::storage::virtio_adapter::default_global_disk_size_bytes(),
    )
    .is_ok();

    kdb(b'R'); // exofs_init returned (post-#6)
    if exofs_ready {
        let _ = crate::exophoenix::forge::seed_kernel_a_image_blob();
    }
    kdb(b'T'); // seed_kernel_a_image_blob done

    // CORRECTION P0-02: register ELF loader after exofs_init
    {
        use crate::fs::elf_loader_impl::EXO_ELF_LOADER;
        use crate::memory::virt::fault::demand_paging::{
            register_file_fault_provider, FileFaultProvider,
        };
        use crate::process::lifecycle::exec::register_elf_loader;
        register_elf_loader(&EXO_ELF_LOADER);

        let provider: &dyn FileFaultProvider = &EXO_ELF_LOADER;
        let (data, vtable): (*const (), *const ()) = unsafe { core::mem::transmute(provider) };
        unsafe {
            register_file_fault_provider(data, vtable);
        }
    }
    kdb(b'E'); // ELF loader registered

    // BUG-02 FIX: enable syscall→fs bridge after exofs_init
    // SAFETY: exofs_init() completed, called once from BSP
    unsafe {
        crate::syscall::fs_bridge::fs_bridge_init();
        crate::syscall::net_bridge::net_bridge_preinit();
    }
    kdb(b'@'); // fs_bridge/net_bridge active
    crate::arch::x86_64::boot_display::stage_ok("FS");
}

#[cfg(all(target_os = "none", not(kani)))]
#[panic_handler]
fn kernel_panic(info: &core::panic::PanicInfo) -> ! {
    // BUG-5 FIX: the old handler performed a silent halt without any diagnostics.
    // Any unwrap()/expect() stopped the system without a message → debugging impossible.
    //
    // Port 0xE9 = QEMU ISA debug device: direct output without initialization required.
    // Available from reset, even before serial drivers initialization.
    #[inline(always)]
    unsafe fn debug_byte(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }
    #[inline(always)]
    unsafe fn debug_str(s: &[u8]) {
        for &b in s {
            debug_byte(b);
        }
    }
    #[inline(always)]
    unsafe fn debug_u32(mut n: u32) {
        let mut buf = [0u8; 10];
        let mut len = 0usize;
        if n == 0 {
            debug_byte(b'0');
            return;
        }
        while n > 0 {
            buf[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
        }
        for i in (0..len).rev() {
            debug_byte(buf[i]);
        }
    }
    // SAFETY: CLI from Ring 0 — always safe. debug_byte writes to port 0xE9.
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        debug_str(b"\n\x1B[1;31m*** KERNEL PANIC ***\x1B[0m ");
        if let Some(loc) = info.location() {
            debug_str(loc.file().as_bytes());
            debug_byte(b':');
            debug_u32(loc.line());
            debug_byte(b':');
            debug_u32(loc.column());
        } else {
            debug_str(b"<location inconnue>");
        }
        debug_byte(b'\n');
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

#[cfg(all(target_os = "none", not(kani)))]
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    // BUG-5 FIX: same fix — displays size/alignment on QEMU port 0xE9.
    #[inline(always)]
    unsafe fn debug_byte(b: u8) {
        core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
    }
    #[inline(always)]
    unsafe fn debug_usize(mut n: usize) {
        let mut buf = [0u8; 20];
        let mut len = 0usize;
        if n == 0 {
            debug_byte(b'0');
            return;
        }
        while n > 0 {
            buf[len] = b'0' + (n % 10) as u8;
            len += 1;
            n /= 10;
        }
        for i in (0..len).rev() {
            debug_byte(buf[i]);
        }
    }
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        for &b in b"\n*** ALLOC ERROR size=" {
            debug_byte(b);
        }
        debug_usize(layout.size());
        for &b in b" align=" {
            debug_byte(b);
        }
        debug_usize(layout.align());
        debug_byte(b'\n');
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}

// ── Main Kernel Entry Point (BSP) ─────────────────────────────────
//
// Note: kernel_main is defined in main.rs (the binary).
// The lib only exposes kernel_init(), arch_boot_init(), and halt_cpu().
