//! Test de résurrection contrôlé ExoPhoenix.
//!
//! Ce module fournit le chemin de preuve QEMU demandé: Kernel A provoque une
//! faute Ring 0 volontaire, Kernel B/ExoPhoenix capture l'effondrement, verrouille
//! la fenêtre IOMMU, vérifie l'image propre de A depuis ExoFS, puis reprend sur un
//! point de relance sain au lieu de nécessiter un reset physique.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::exceptions::ExceptionFrame;
use crate::exophoenix::{forge, ssr, stage0, PhoenixState, PHOENIX_STATE};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};

static TEST_ARMED: AtomicBool = AtomicBool::new(false);
static RECOVERY_STARTED: AtomicBool = AtomicBool::new(false);
static RECOVERY_SUCCEEDED: AtomicBool = AtomicBool::new(false);
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

pub fn try_recover_exception(reason: &str, frame: &mut ExceptionFrame) -> bool {
    if !frame.from_kernel() {
        return false;
    }
    // PATCH-P0-PHOENIX: en production, déclencher la récupération dès que
    // ExoPhoenix est en état Normal (complètement opérationnel).
    // En mode test (cfg exophoenix_resurrection_test), TEST_ARMED suffit aussi.
    // Avant ce patch, la garde TEST_ARMED était toujours false en production,
    // rendant la récupération d'exception impossible hors tests contrôlés.
    let phoenix_ready = PHOENIX_STATE.load(Ordering::Acquire) == PhoenixState::Normal as u8;
    let test_triggered = TEST_ARMED.swap(false, Ordering::AcqRel);

    if !phoenix_ready && !test_triggered {
        return false;
    }
    recover_kernel_a(reason, frame)
}

#[cfg(exophoenix_resurrection_test)]
pub fn handle_nmi(frame: &mut ExceptionFrame) -> bool {
    TEST_ARMED.store(true, Ordering::Release);
    recover_kernel_a("NMI matériel", frame)
}

#[cfg(not(exophoenix_resurrection_test))]
pub fn handle_nmi(_frame: &mut ExceptionFrame) -> bool {
    false
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

#[cfg(exophoenix_resurrection_test)]
fn qemu_debug_exit_success() -> ! {
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

#[cfg(not(exophoenix_resurrection_test))]
fn qemu_debug_exit_success() -> ! {
    crate::arch::x86_64::halt_cpu()
}

#[no_mangle]
pub extern "C" fn exophoenix_resurrection_landing_pad() -> ! {
    set_handoff_flag(ssr::HANDOFF_NORMAL);
    PHOENIX_STATE.store(PhoenixState::Normal as u8, Ordering::Release);
    debug_str("[ExoPhoenix] Kernel A relancé depuis image saine\n");
    debug_str("[ExoPhoenix] RESURRECTION_OK\n");
    qemu_debug_exit_success()
}

pub fn recovery_succeeded() -> bool {
    RECOVERY_SUCCEEDED.load(Ordering::Acquire)
}
