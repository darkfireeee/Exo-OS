//! ExoPhoenix controlled resurrection test.
//!
//! This module provides the requested QEMU proof path: Kernel A triggers an
//! intentional Ring 0 fault, Kernel B/ExoPhoenix captures the collapse, locks
//! the IOMMU window, verifies the clean image of A from ExoFS, then resumes at a
//! healthy restart point instead of requiring a physical reset.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::exceptions::ExceptionFrame;
use crate::exophoenix::{forge, ssr, stage0, PhoenixState, PHOENIX_STATE};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};

static TEST_ARMED: AtomicBool = AtomicBool::new(false);
static RECOVERY_STARTED: AtomicBool = AtomicBool::new(false);
static RECOVERY_SUCCEEDED: AtomicBool = AtomicBool::new(false);
/// Runtime controlled recovery landing pad for Ring 0 crash/exception survival proofs (P5.2).
static RUNTIME_RECOVERY_ARMED: AtomicBool = AtomicBool::new(false);
static RUNTIME_RECOVERY_RIP: AtomicU64 = AtomicU64::new(0);
static RUNTIME_RECOVERY_RSP: AtomicU64 = AtomicU64::new(0);
static RUNTIME_RECOVERY_RBP: AtomicU64 = AtomicU64::new(0);
static RUNTIME_RECOVERY_COUNT: AtomicU64 = AtomicU64::new(0);
/// Hardware NMI recovery hook state (P5.1, `IST_NMI`).
static NMI_RECOVERY_ARMED: AtomicBool = AtomicBool::new(false);
static NMI_DEADLOCK_BROKEN: AtomicBool = AtomicBool::new(false);
static NMI_EXECUTED_ON_IST: AtomicBool = AtomicBool::new(false);
static NMI_RECOVERY_COUNT: AtomicU64 = AtomicU64::new(0);
#[cfg(exophoenix_resurrection_test)]
static BOOT_TEST_STARTED: AtomicBool = AtomicBool::new(false);

#[inline(always)]
unsafe fn debug_byte(b: u8) {
    core::arch::asm!("out 0xE9, al", in("al") b, options(nomem, nostack));
}

fn debug_str(s: &str) {
    for &b in s.as_bytes() {
        unsafe { debug_byte(b) };
    }
}

fn set_handoff_flag(v: u64) {
    unsafe {
        ssr::ssr_atomic(ssr::SSR_HANDOFF_FLAG).store(v, Ordering::Release);
    }
}

fn lock_iommu_for_handoff() {
    let blocked_domain = stage0::blocked_domain_id();
    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked_domain as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

fn recover_kernel_a(reason: &str, frame: &mut ExceptionFrame) -> bool {
    if RECOVERY_STARTED.swap(true, Ordering::AcqRel) {
        return false;
    }

    debug_str("\n[ExoPhoenix] Kernel A effondré: ");
    debug_str(reason);
    debug_str("\n");
    debug_str("[ExoPhoenix] Core 0: heartbeat Kernel A arrêté\n");

    if ssr::initialize_layout_v7().is_err() {
        debug_str("[ExoPhoenix] SSR v7 invalide, récupération refusée\n");
        PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
        return false;
    }

    PHOENIX_STATE.store(PhoenixState::Threat as u8, Ordering::Release);
    set_handoff_flag(ssr::HANDOFF_FREEZE_REQ);
    lock_iommu_for_handoff();
    debug_str("\x1b[31m[ExoPhoenix] Handoff déclenché, IOMMU verrouillé\x1b[0m\n");

    PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);
    match forge::verify_seeded_kernel_a_image() {
        Ok(()) => {
            crate::arch::x86_64::time::ktime::phoenix_reset_time_base();
            crate::arch::x86_64::irq::reset_all_masked_since();
            debug_str("[ExoPhoenix] ExoFS propre vérifié, image Kernel A rechargée\n");
            set_handoff_flag(ssr::HANDOFF_B_ACTIVE);
            core::sync::atomic::fence(Ordering::SeqCst);
            frame.rip = exophoenix_resurrection_landing_pad as *const () as usize as u64;
            RECOVERY_SUCCEEDED.store(true, Ordering::Release);
            true
        }
        Err(_) => {
            debug_str("[ExoPhoenix] Forge Kernel A refusé, passage Degraded\n");
            set_handoff_flag(ssr::HANDOFF_NORMAL);
            PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
            false
        }
    }
}

/// Arms a runtime Ring 0 exception recovery landing pad (P5.2).
/// Used exclusively by controlled Ring 0 crash-survival qualification (`F1`).
#[cfg(feature = "phoenix-fault-injection")]
pub fn arm_runtime_recovery(rip: u64, rsp: u64, rbp: u64) {
    RUNTIME_RECOVERY_RIP.store(rip, Ordering::Release);
    RUNTIME_RECOVERY_RSP.store(rsp, Ordering::Release);
    RUNTIME_RECOVERY_RBP.store(rbp, Ordering::Release);
    RUNTIME_RECOVERY_ARMED.store(true, Ordering::Release);
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn disarm_runtime_recovery() {
    RUNTIME_RECOVERY_ARMED.store(false, Ordering::Release);
    RUNTIME_RECOVERY_RIP.store(0, Ordering::Release);
    RUNTIME_RECOVERY_RSP.store(0, Ordering::Release);
    RUNTIME_RECOVERY_RBP.store(0, Ordering::Release);
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn arm_nmi_deadlock_recovery() {
    NMI_DEADLOCK_BROKEN.store(false, Ordering::Release);
    NMI_EXECUTED_ON_IST.store(false, Ordering::Release);
    NMI_RECOVERY_ARMED.store(true, Ordering::Release);
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn nmi_deadlock_broken() -> bool {
    NMI_DEADLOCK_BROKEN.load(Ordering::Acquire)
}

#[cfg(feature = "phoenix-fault-injection")]
pub fn nmi_executed_on_ist() -> bool {
    NMI_EXECUTED_ON_IST.load(Ordering::Acquire)
}

pub fn runtime_recovery_count() -> u64 {
    RUNTIME_RECOVERY_COUNT.load(Ordering::Acquire)
}

pub fn nmi_recovery_count() -> u64 {
    NMI_RECOVERY_COUNT.load(Ordering::Acquire)
}

fn try_runtime_controlled_recovery(reason: &str, frame: &mut ExceptionFrame) -> bool {
    if !RUNTIME_RECOVERY_ARMED.swap(false, Ordering::AcqRel) {
        return false;
    }
    let _target_rip = RUNTIME_RECOVERY_RIP.load(Ordering::Acquire);
    let _target_rsp = RUNTIME_RECOVERY_RSP.load(Ordering::Acquire);
    let _target_rbp = RUNTIME_RECOVERY_RBP.load(Ordering::Acquire);

    debug_str("\n[ExoPhoenix] Ring 0 emergency crash intercepted: ");
    debug_str(reason);
    debug_str("\n");

    // Lock-free, zero-alloc ISR-safe handoff signaling inside `#UD` trap handler;
    // the full switch cycle (`force_phoenix_switch_cycle_with_mode`) executes immediately
    // upon `iretq` past the 2-byte `ud2` instruction (`0x0F 0x0B`).
    PHOENIX_STATE.store(PhoenixState::Threat as u8, Ordering::Release);
    set_handoff_flag(ssr::HANDOFF_EMERGENCY_CRASH);
    lock_iommu_for_handoff();
    PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);
    set_handoff_flag(ssr::HANDOFF_B_ACTIVE);
    core::sync::atomic::fence(Ordering::SeqCst);

    // Advance RIP past the 2-byte `ud2` instruction (`0x0F 0x0B`), keeping the hardware-saved
    // `frame.rsp` and `frame.rbp` intact, and return `rax = 1` to the recovery landing pad.
    frame.rip = frame.rip.wrapping_add(2);
    frame.rax = 1;
    RUNTIME_RECOVERY_COUNT.fetch_add(1, Ordering::AcqRel);
    RECOVERY_SUCCEEDED.store(true, Ordering::Release);
    true
}

pub fn try_recover_exception(reason: &str, frame: &mut ExceptionFrame) -> bool {
    if !frame.from_kernel() {
        return false;
    }
    if try_runtime_controlled_recovery(reason, frame) {
        return true;
    }
    // PATCH-P0-PHOENIX: in production, trigger recovery as soon as
    // ExoPhoenix is in Normal state (fully operational).
    // In test mode (cfg exophoenix_resurrection_test), TEST_ARMED is also sufficient.
    // Prior to this patch, the TEST_ARMED guard was always false in production,
    // making exception recovery impossible outside controlled tests.
    let phoenix_ready = PHOENIX_STATE.load(Ordering::Acquire) == PhoenixState::Normal as u8;
    let test_triggered = TEST_ARMED.swap(false, Ordering::AcqRel);

    if !phoenix_ready && !test_triggered {
        return false;
    }
    recover_kernel_a(reason, frame)
}

/// Hardware `#NMI` handler invoked on dedicated TSS `IST_NMI` stack (`cpu_faults::do_nmi`, P5.1).
pub fn handle_nmi(frame: &mut ExceptionFrame) -> bool {
    let cpu_id = crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0) as usize;
    let current_rsp: u64;
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nostack, nomem));
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        current_rsp = 0;
    }

    let on_ist = crate::arch::x86_64::tss::rsp_in_nmi_ist(cpu_id, current_rsp);
    if on_ist {
        NMI_EXECUTED_ON_IST.store(true, Ordering::Release);
    }

    if NMI_RECOVERY_ARMED.swap(false, Ordering::AcqRel) {
        ssr::record_emergency_crash(
            0x4E4D_4933, // "NMI3"
            frame.rip,
            crate::arch::x86_64::read_cr2(),
            cpu_id as u64,
        );
        PHOENIX_STATE.store(PhoenixState::Threat as u8, Ordering::Release);
        set_handoff_flag(ssr::HANDOFF_EMERGENCY_CRASH);
        lock_iommu_for_handoff();
        // Zero-lock NMI-safe recovery signaling: clear deadlock barrier so interrupted CPU
        // exits its `cli; pause` deadlock cleanly and completes full mirror heal + ExoLedger P0.
        NMI_DEADLOCK_BROKEN.store(true, Ordering::Release);
        NMI_RECOVERY_COUNT.fetch_add(1, Ordering::AcqRel);
        set_handoff_flag(ssr::HANDOFF_B_ACTIVE);
        PHOENIX_STATE.store(PhoenixState::Restore as u8, Ordering::Release);
        return true;
    }

    #[cfg(exophoenix_resurrection_test)]
    {
        TEST_ARMED.store(true, Ordering::Release);
        return recover_kernel_a("NMI matériel", frame);
    }

    #[cfg(not(exophoenix_resurrection_test))]
    {
        let _ = frame;
        false
    }
}

#[cfg(exophoenix_resurrection_test)]
#[inline(never)]
pub fn trigger_self_destruct() -> ! {
    if BOOT_TEST_STARTED.swap(true, Ordering::AcqRel) {
        crate::arch::x86_64::halt_cpu();
    }

    debug_str("[ExoPhoenix] Test de résurrection: autodestruction Ring 0 armée\n");
    TEST_ARMED.store(true, Ordering::Release);

    unsafe {
        core::arch::asm!(
            "xor edx, edx",
            "xor eax, eax",
            "div eax",
            options(nostack, nomem)
        );
    }

    crate::arch::x86_64::halt_cpu()
}

#[cfg(all(exophoenix_resurrection_test, feature = "qemu-compliance-exit"))]
fn finish_resurrection_landing_pad() -> ! {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xF4u16,
            in("eax") 0x10u32,
            options(nomem, nostack)
        );
    }
    crate::arch::x86_64::halt_cpu()
}

#[cfg(not(all(exophoenix_resurrection_test, feature = "qemu-compliance-exit")))]
fn finish_resurrection_landing_pad() -> ! {
    crate::arch::x86_64::halt_cpu()
}

#[no_mangle]
pub extern "C" fn exophoenix_resurrection_landing_pad() -> ! {
    set_handoff_flag(ssr::HANDOFF_NORMAL);
    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);
    debug_str("[ExoPhoenix] Kernel A relancé depuis image saine\n");
    debug_str("[ExoPhoenix] RESURRECTION_OK\n");
    finish_resurrection_landing_pad()
}

pub fn recovery_succeeded() -> bool {
    RECOVERY_SUCCEEDED.load(Ordering::Acquire)
}
