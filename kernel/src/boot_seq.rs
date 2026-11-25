//! # boot_seq.rs — Layered Kernel Initialization & Bare-Metal Panic/OOM Handlers
//!
//! Extracted from `lib.rs` so `lib.rs` remains a pure module-wiring & re-export root.

use crate::arch::x86_64::debug_e9_byte;
#[cfg(all(target_os = "none", not(kani)))]
use crate::arch::x86_64::{disable_interrupts, halt_cpu, read_rip};

/// Layer initialization sequence (called from `kernel_main` in `main.rs`).
///
/// Follows DOC2 + DOC3 order:
/// 1. `arch`       (already done prior to this call)
/// 2. `memory/`    — EmergencyPool FIRST (RULE EMERGENCY-01)
/// 3. `scheduler/` — after memory
/// 4. `process/`   — after scheduler
/// 5. `security/`  — after process bootstrap (RULE SEC-BOOT-GAP: before IPC)
/// 6. `ipc/`       — after process + security
/// 7. `fs/`        — last
///
/// # Safety
/// Must be called exactly once, from the BSP, after `arch_boot_init`.
pub unsafe fn kernel_init(cpu_count: usize) {
    struct IrqStateGuard(u64);

    impl Drop for IrqStateGuard {
        fn drop(&mut self) {
            crate::arch::x86_64::irq_restore(self.0);
        }
    }

    // ── Phase 2a: EmergencyPool — ABSOLUTELY FIRST (RULE EMERGENCY-01) ─────────
    crate::memory::physical::frame::emergency_pool::init();
    debug_e9_byte(b'2');

    // ── Phase 2b: Heap allocator (SLUB + large) ────────────────────────────
    crate::memory::heap::allocator::hybrid::init();
    crate::memory::cow::init();
    let _ = crate::arch::x86_64::apic::local_apic::init_lapic_fixmap_post_memory();
    debug_e9_byte(b'3');
    crate::arch::x86_64::boot_display::stage_ok("MEMORY");

    // ── Phase 2c: Time subsystem (HPET + TSC calibration + ktime seqlock) ────────
    crate::arch::x86_64::time::time_init();
    debug_e9_byte(b'4');
    crate::arch::x86_64::boot_display::stage_ok("TIME");

    // ── Phase 2d: GI-03 drivers (IOMMU queues + kernel notifications) ────────
    crate::drivers::init();
    debug_e9_byte(b'D');
    crate::arch::x86_64::boot_display::stage_ok("DRIVERS");

    // ── Phase 2e: ExoSeal/ExoCage pre-scheduler (CORR-82 / S-03) ───────────
    unsafe {
        crate::security::exoseal::exoseal_boot_phase0();
    }
    debug_e9_byte(b'C');
    crate::arch::x86_64::boot_display::stage_ok("EXOSEAL0");

    // ── Phase 2f: cgroup root before runqueues (CORR-77) ─────────────
    crate::process::resource::cgroup::init();
    debug_e9_byte(b'c');

    // ── Phase 3: Scheduler ───────────────────────────────────────────
    let total_cpus = if crate::arch::x86_64::smp::init::smp_boot_complete() {
        cpu_count.max(1)
    } else {
        1
    };
    let sched_cpus = 1;
    crate::scheduler::init(&crate::scheduler::SchedInitParams {
        nr_cpus: sched_cpus,
        nr_nodes: 1,
    });
    if total_cpus > sched_cpus {
        crate::scheduler::core::runqueue::init_percpu(total_cpus);
    }
    debug_e9_byte(b'5');

    // ── Phase 3b: Bootstrap idle thread (BSP + APs already online) ─────────
    let _ = crate::scheduler::core::publish_current_boot_idle(
        0,
        crate::arch::x86_64::boot::early_init::boot_stack_top(),
    );
    crate::scheduler::core::bind_boot_idle_threads(total_cpus);
    debug_e9_byte(b'6');

    // ── Phase 3c: Address space cloner registration ───────────────
    {
        use crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER;
        use crate::process::lifecycle::fork::register_addr_space_cloner;
        register_addr_space_cloner(&KERNEL_AS_CLONER);
    }
    debug_e9_byte(b'F');
    crate::arch::x86_64::boot_display::stage_ok("SCHEDULER");

    // ── Phase 4: Process ────────────────────────────────────────────────────
    let process_irq_guard = IrqStateGuard(crate::arch::x86_64::irq_save());
    debug_e9_byte(b'a');
    crate::process::init(&crate::process::ProcessInitParams::default());
    drop(process_irq_guard);
    debug_e9_byte(b'P');
    crate::arch::x86_64::boot_display::stage_ok("PROCESS");

    // ── Phase 5: Security ──────────────────────────────────────────────────
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
    debug_e9_byte(b'7');

    // ERR-05 fix: Init SipHash seed of the futex table (anti-DoS hash collision).
    {
        let mut seed = [0u8; 16];
        if crate::security::crypto::rng_fill(&mut seed).is_ok() {
            crate::memory::utils::futex_table::init_futex_seed(seed);
        }
    }
    debug_e9_byte(b'8');
    crate::arch::x86_64::boot_display::stage_ok("SECURITY");

    crate::security::start_integrity_monitor();

    // ── Phase 5b: ExoPhoenix Stage0 (IOMMU blocking domain) ─────────────
    let _stage0_summary = crate::exophoenix::stage0::stage0_init_all_steps(true);
    debug_e9_byte(b'S');
    crate::arch::x86_64::boot_display::stage_ok("STAGE0");

    // ── Phase 6: IPC ────────────────────────────────────────────────────────
    crate::ipc::ring::spsc::init_spsc_rings();
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
    crate::ipc::ipc_init(shm_pool.start_address().as_u64(), 1);
    crate::ipc::ipc_install_scheduler_hooks(crate::scheduler::core::switch::block_current_thread);
    crate::ipc::ipc_install_vmm_hooks(
        crate::arch::x86_64::memory_iface::shm_vmm_map_page,
        crate::arch::x86_64::memory_iface::shm_vmm_unmap_page,
    );
    debug_e9_byte(b'9');
    crate::arch::x86_64::boot_display::stage_ok("IPC");

    // ── Phase 7: FS ─────────────────────────────────────────────────────────
    let exofs_ready = crate::fs::exofs::exofs_init(
        crate::fs::exofs::storage::virtio_adapter::default_global_disk_size_bytes(),
    )
    .is_ok();

    debug_e9_byte(b'R');
    if exofs_ready {
        let _ = crate::exophoenix::forge::seed_kernel_a_image_blob();
    }
    debug_e9_byte(b'T');

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
    debug_e9_byte(b'E');

    unsafe {
        crate::syscall::fs_bridge::fs_bridge_init();
        crate::syscall::net_bridge::net_bridge_preinit();
    }
    debug_e9_byte(b'@');
    crate::arch::x86_64::boot_display::stage_ok("FS");
}

#[cfg(all(target_os = "none", not(kani)))]
#[inline]
fn debug_e9_str(s: &[u8]) {
    for &b in s {
        debug_e9_byte(b);
    }
}

#[cfg(all(target_os = "none", not(kani)))]
#[inline]
fn debug_e9_usize(mut n: usize) {
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if n == 0 {
        debug_e9_byte(b'0');
        return;
    }
    while n > 0 {
        buf[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
    }
    for i in (0..len).rev() {
        debug_e9_byte(buf[i]);
    }
}

#[cfg(all(target_os = "none", not(kani)))]
#[panic_handler]
fn kernel_panic(info: &core::panic::PanicInfo) -> ! {
    disable_interrupts();
    if crate::exophoenix::ssr::validate_layout_v7().is_ok() {
        let rip_here = read_rip();
        let cr2 = crate::arch::x86_64::read_cr2();
        let cpu = crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0) as u64;
        crate::exophoenix::ssr::record_emergency_crash(0x5041_4E49, rip_here, cr2, cpu);
    }
    debug_e9_str(b"\n\x1B[1;31m*** KERNEL PANIC ***\x1B[0m ");
    if let Some(loc) = info.location() {
        debug_e9_str(loc.file().as_bytes());
        debug_e9_byte(b':');
        debug_e9_usize(loc.line() as usize);
        debug_e9_byte(b':');
        debug_e9_usize(loc.column() as usize);
    } else {
        debug_e9_str(b"<location inconnue>");
    }
    debug_e9_byte(b'\n');
    halt_cpu()
}

#[cfg(all(target_os = "none", not(kani)))]
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    disable_interrupts();
    debug_e9_str(b"\n*** ALLOC ERROR size=");
    debug_e9_usize(layout.size());
    debug_e9_str(b" align=");
    debug_e9_usize(layout.align());
    debug_e9_byte(b'\n');
    halt_cpu()
}
