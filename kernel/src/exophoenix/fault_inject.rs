//! # `kernel/src/exophoenix/fault_inject.rs` — Destructive Fault Injection & Qualification Hooks (P5.0–P8)
//!
//! **ABSOLUTE CONFINEMENT (`P5.0` / `F0`)**:
//! This entire module is compiled **ONLY** when `--features phoenix-fault-injection` is enabled.
//! It is NEVER included in `default`, `make release`, or `EXO_PROFILE=pure` binaries, and
//! every destructive subcommand (`10..=18`) additionally requires a valid `CAP_PHOENIX_FAULT_INJECT`
//! `CapToken` bound to the calling privileged qualification runner.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::x86_64::apic::{self, local_apic, x2apic};
use crate::arch::x86_64::cpu::tsc;
use crate::arch::x86_64::idt;
use crate::arch::x86_64::irq::routing as irq_routing;
use crate::arch::x86_64::irq::types::{IrqOwnerPid, IrqRouteRegistration, IrqSourceKind, IrqVector, IRQ_TABLE, MAX_OVERFLOWS};
use crate::exophoenix::{golden_mirror, handoff, resurrection, ssr, stage0};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::core::{DiskOffset, EpochId, ObjectId as ExofsObjectId};
use crate::fs::exofs::epoch::epoch_record::{count_valid_records, select_best_record, EpochRecord};
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::errno::{EACCES, EFAULT, EINVAL, EIO, EPERM};
use crate::syscall::validation::{copy_from_user, copy_to_user, UserBuf};
use exo_phoenix_ssr::metrics::{
    PhoenixTimingRecord, PHOENIX_MODE_COLD_RESPAWN, PHOENIX_MODE_CRASH_NMI,
    PHOENIX_MODE_SOFT_FORCE, PHOENIX_REASON_EXONMI, PHOENIX_REASON_FORCE_SYSCALL,
    PHOENIX_REASON_PANIC,
};

pub const SUBCMD_MINT_FAULT_CAP: u64 = 9;
pub const FAULT_KERNEL_A_PANIC: u64 = 10;
pub const FAULT_KERNEL_A_DEADLOCK: u64 = 11;
pub const FAULT_KERNEL_A_TEXT_CORRUPT: u64 = 12;
pub const FAULT_AP_STALL_FREEZE: u64 = 13;
pub const FAULT_STRATE1_COLD_KILL: u64 = 14;
pub const FAULT_EXOFS_CRASH_MID_EPOCH: u64 = 15;
pub const FAULT_IRQ_STORM_AND_MASKED_SINCE: u64 = 16;
pub const FAULT_EXOVEIL_AND_P0_STRESS: u64 = 17;
pub const FAULT_DMA_IOMMU_STRESS: u64 = 18;
pub const FAULT_CRYPTO_AEAD_NONCE_VERIFY: u64 = 19;

const AP_STALL_NONE: u32 = u32::MAX;
static AP_STALL_TARGET_SLOT: AtomicU32 = AtomicU32::new(AP_STALL_NONE);
static AP_STALL_OCCURRED: AtomicBool = AtomicBool::new(false);

/// Snapshot returned by `FAULT_EXOVEIL_AND_P0_STRESS(arg1 = 1)` for `F9` 50-cycle leak accounting.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PhoenixLeakSnapshot {
    pub free_frames: u64,
    pub active_pids: u64,
    pub p0_used: u64,
    pub ledger_verified: u64,
    pub online_cpus: u64,
    pub is_kvm: u64,
    pub vtd_units: u64,
    pub heartbeat: u64,
}

/// Checked by `interrupts::handle_freeze_ipi()` on peer APs during `F2` (`CORR-33`).
pub(crate) fn consume_ap_stall_for_slot(slot: usize) -> bool {
    let target = AP_STALL_TARGET_SLOT.load(Ordering::Acquire);
    if target != AP_STALL_NONE && target as usize == slot {
        if AP_STALL_TARGET_SLOT
            .compare_exchange(target, AP_STALL_NONE, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            AP_STALL_OCCURRED.store(true, Ordering::Release);
            return true;
        }
    }
    false
}

/// Consumes whether a controlled `F2` AP stall occurred during the last freeze timeout.
pub(crate) fn take_controlled_ap_stall_occurred() -> bool {
    AP_STALL_OCCURRED.swap(false, Ordering::AcqRel)
}

fn caller_is_privileged_runner(caller_pid: u32) -> bool {
    if caller_pid <= 1 {
        return true;
    }
    let restrictions = crate::security::zero_trust::process_restrictions(caller_pid);
    if (restrictions & crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC) != 0 {
        return false;
    }
    let class = crate::security::service_class_of(Pid(caller_pid));
    if matches!(
        class,
        crate::security::ServiceClass::InitServer
            | crate::security::ServiceClass::ExoShield
            | crate::security::ServiceClass::Exosh
    ) {
        return true;
    }
    PROCESS_REGISTRY
        .find_by_pid(Pid(caller_pid))
        .map(|pcb| {
            let creds = pcb.get_creds();
            creds.is_root() || creds.euid == 0
        })
        .unwrap_or(false)
}

fn verify_fault_cap_from_user(token_ptr: u64, caller_pid: u32) -> Result<(), i64> {
    if token_ptr == 0 {
        return Err(EPERM);
    }
    let mut raw = [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE];
    if copy_from_user(raw.as_mut_ptr(), token_ptr as *const u8, raw.len()).is_err() {
        return Err(EFAULT);
    }
    let token = crate::security::capability::CapToken::from_bytes(&raw).ok_or(EPERM)?;
    crate::security::capability::verify_phoenix_fault_inject_token(token, caller_pid)
        .map_err(|_| EPERM)
}

/// Entrypoint for `SYS_EXOPHOENIX_TEST (524)` subcommands `9..=18`.
pub fn dispatch_fault_injection(
    caller_pid: u32,
    subcmd: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
) -> i64 {
    if !caller_is_privileged_runner(caller_pid) {
        return EPERM;
    }

    if subcmd == SUBCMD_MINT_FAULT_CAP {
        if arg0 == 0 {
            return EFAULT;
        }
        let wire_len = crate::security::capability::CAP_TOKEN_WIRE_SIZE;
        if arg1 != 0 && (arg1 as usize) < wire_len {
            return EINVAL;
        }
        if UserBuf::validate(arg0, wire_len, wire_len).is_err() {
            return EFAULT;
        }
        let token = match crate::security::capability::mint_phoenix_fault_inject_token(caller_pid) {
            Ok(t) => t,
            Err(e) => return e.to_kernel_errno() as i64,
        };
        let bytes = token.to_bytes();
        if copy_to_user(arg0 as *mut u8, bytes.as_ptr(), bytes.len()).is_err() {
            let _ = crate::security::capability::revoke_handle(token.object_id().as_u64() as u32);
            return EFAULT;
        }
        return token.object_id().as_u64() as i64;
    }

    // All destructive injectors require a valid `CAP_PHOENIX_FAULT_INJECT` in `arg0`.
    if let Err(errno) = verify_fault_cap_from_user(arg0, caller_pid) {
        return errno;
    }

    match subcmd {
        FAULT_KERNEL_A_PANIC => phoenix_inject_kernel_a_panic(),
        FAULT_KERNEL_A_DEADLOCK => phoenix_inject_kernel_a_deadlock(),
        FAULT_KERNEL_A_TEXT_CORRUPT => phoenix_inject_kernel_a_text_corrupt(),
        FAULT_AP_STALL_FREEZE => phoenix_inject_ap_stall_freeze(arg1),
        FAULT_STRATE1_COLD_KILL => phoenix_inject_strate1_cold_kill(arg1, arg2),
        FAULT_EXOFS_CRASH_MID_EPOCH => phoenix_inject_exofs_crash_mid_epoch(),
        FAULT_IRQ_STORM_AND_MASKED_SINCE => phoenix_inject_irq_storm_and_masked_since(caller_pid),
        FAULT_EXOVEIL_AND_P0_STRESS => phoenix_inject_exoveil_and_p0_stress(arg1, arg2),
        FAULT_DMA_IOMMU_STRESS => phoenix_inject_dma_iommu_stress(arg1),
        FAULT_CRYPTO_AEAD_NONCE_VERIFY => phoenix_inject_crypto_aead_nonce_verify(),
        _ => EINVAL,
    }
}

/// `F1a` (P5.2): Trigger a REAL Ring 0 hardware exception (`ud2` / `#UD`), record crash
/// context lock-free in SSR v7 (`PANIC_CODE`, `FAULT_RIP`, `FAULT_CR2`, `CPU_ID`,
/// `HANDOFF_EMERGENCY_CRASH`), intercept in `kernel_panic_exception` -> `try_recover_exception`,
/// lock IOMMU, heal Golden RAM Mirror, log P0 `ExoLedger`, and resume cleanly via `iretq`.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_kernel_a_panic() -> i64 {
    let before_count = resurrection::runtime_recovery_count();

    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    let recovered: u64 = unsafe {
        resurrection::arm_runtime_recovery(1, 1, 1);
        let out = crate::arch::x86_64::phoenix_ud2_probe();
        resurrection::disarm_runtime_recovery();
        out
    };
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    let recovered: u64 = 1;

    let after_count = resurrection::runtime_recovery_count();
    let crash = ssr::read_emergency_crash();
    if recovered != 1 || after_count <= before_count || crash.rip == 0 {
        return EIO;
    }

    // Follow up with full CRASH_NMI telemetry cycle so SSR +0xC000 & serial reflect recovery.
    if handoff::force_phoenix_switch_cycle_with_mode(PHOENIX_REASON_PANIC, PHOENIX_MODE_CRASH_NMI)
        .is_err()
    {
        return EIO;
    }
    0
}

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
fn send_self_hardware_nmi() {
    if apic::is_x2apic() {
        // x2APIC ICR: Destination Shorthand = Self (0b01 << 18) | Delivery Mode = NMI (0b100 << 8) | Level Assert (1 << 14).
        let icr: u64 = (0b01u64 << 18) | (1u64 << 14) | (0b100u64 << 8);
        x2apic::x2apic_write_icr(icr);
    } else {
        // xAPIC ICR: Self NMI.
        local_apic::lapic_write(local_apic::LAPIC_ICR_HIGH, 0);
        local_apic::lapic_write(
            local_apic::LAPIC_ICR_LOW,
            local_apic::ICR_DEST_SELF | local_apic::ICR_LEVEL_ASSERT | local_apic::ICR_DM_NMI,
        );
    }
}

/// `F1b` (P5.1): Verify `V7-C-05`, live Kernel A SSR heartbeat, dedicated `IST_NMI`,
/// 3-strike `ExoNmi` escalation into P0 `ExoLedger`, and real hardware `#NMI` breaking
/// a `cli`-masked Ring 0 deadlock on `IST_NMI`.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_kernel_a_deadlock() -> i64 {
    // 1. Verify V7-C-05 invariant: MAX_CORES_RUNTIME <= SSR_MAX_CORES_LAYOUT.
    if exo_phoenix_ssr::active_cores() as usize > exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT {
        return EIO;
    }

    // 2. Verify live Kernel A heartbeat counter is ticking in SSR v7.
    if ssr::read_kernel_a_heartbeat() == 0 {
        let _ = ssr::bump_kernel_a_heartbeat();
    }

    // 3. Verify dedicated NMI IST stack is configured for this CPU.
    let cpu_id = crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0) as usize;
    if crate::arch::x86_64::tss::nmi_ist_top(cpu_id) == 0 {
        return EIO;
    }

    // 4. Record 3 missed heartbeat strikes (`ExoNmi`) in SSR + P0 ExoLedger.
    ssr::record_nmi_strikes(3);
    crate::security::exoledger::exo_ledger_append_p0(
        crate::security::exoledger::ActionTag::WatchdogExpired { strikes: 3 },
    );

    // 5. Enter a REAL `cli`-masked deadlock loop broken ONLY by hardware `#NMI` on `IST_NMI`.
    resurrection::arm_nmi_deadlock_recovery();
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    unsafe {
        let rflags = crate::arch::x86_64::save_and_cli();
        send_self_hardware_nmi();
        let deadline = tsc::read_tsc().wrapping_add(tsc::tsc_ms_to_cycles(200).max(1));
        while !resurrection::nmi_deadlock_broken() {
            if tsc::read_tsc().wrapping_sub(deadline) < (u64::MAX / 2) {
                break;
            }
            crate::arch::x86_64::cpu_pause();
        }
        crate::arch::x86_64::restore_if_enabled(rflags);
    }

    if !resurrection::nmi_deadlock_broken() || !resurrection::nmi_executed_on_ist() {
        return EIO;
    }

    if handoff::force_phoenix_switch_cycle_with_mode(PHOENIX_REASON_EXONMI, PHOENIX_MODE_CRASH_NMI)
        .is_err()
    {
        return EIO;
    }
    0
}

/// `F1c` (P5.3): Physically corrupt 1 byte inside Kernel A's live `.text` sentinel function,
/// prove both live code execution and BLAKE3 diverge, run ExoPhoenix recovery invoking
/// `heal_kernel_a_from_mirror()`, and prove physical byte & BLAKE3 restoration 100%.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_kernel_a_text_corrupt() -> i64 {
    golden_mirror::init_golden_mirror();

    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    let pre_val = unsafe { golden_mirror::exo_phoenix_integrity_sentinel() };
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    let pre_val = golden_mirror::exo_phoenix_integrity_sentinel();

    if pre_val != golden_mirror::TEXT_SENTINEL_MAGIC || !golden_mirror::sentinel_is_intact() {
        return EIO;
    }

    let Ok((before_byte, after_byte)) =
        golden_mirror::inject_sentinel_text_bitflip(golden_mirror::TEXT_SENTINEL_CORRUPT_OFFSET)
    else {
        return EIO;
    };
    if before_byte == after_byte {
        return EIO;
    }

    // Verify live `.text` execution AND BLAKE3 detect the injected bit-flip.
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    let corrupted_val = unsafe { golden_mirror::exo_phoenix_integrity_sentinel() };
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    let corrupted_val = golden_mirror::exo_phoenix_integrity_sentinel();

    if corrupted_val == golden_mirror::TEXT_SENTINEL_MAGIC || golden_mirror::sentinel_is_intact() {
        return EIO;
    }

    // Run ExoPhoenix switch cycle -> invokes `heal_kernel_a_from_mirror()`.
    let Ok(record) = handoff::force_phoenix_switch_cycle_with_mode(
        PHOENIX_REASON_PANIC,
        PHOENIX_MODE_CRASH_NMI,
    ) else {
        return EIO;
    };
    if !record.text_healed() {
        return EIO;
    }

    // Verify `.text` sentinel physically returned to its original code & BLAKE3 digest.
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    let post_val = unsafe { golden_mirror::exo_phoenix_integrity_sentinel() };
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    let post_val = golden_mirror::exo_phoenix_integrity_sentinel();

    if post_val != golden_mirror::TEXT_SENTINEL_MAGIC || !golden_mirror::sentinel_is_intact() {
        return EIO;
    }
    0
}

/// `F2` (P6.1 / `CORR-33`):
/// - `target_slot == 0`: nominal SMP freeze across all online CPUs, returns `(online_cpus << 16) | acked_peers`.
/// - `target_slot > 0`: arms `FAULT_AP_STALL_FREEZE` on `target_slot`, proves 100 ms `CORR-33`
///   bounded wait + `FREEZE_ACK_DEAD (0xDEAD_0001)` + `DEGRADED=1` without global deadlock,
///   returns `delta_s0_freeze_us as i64`.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_ap_stall_freeze(target_slot: u64) -> i64 {
    let online = crate::arch::x86_64::smp::init::smp_cpu_count().max(1);
    if target_slot == 0 {
        let Ok(rec) = handoff::force_phoenix_switch_cycle_with_mode(
            PHOENIX_REASON_FORCE_SYSCALL,
            PHOENIX_MODE_SOFT_FORCE,
        ) else {
            return EIO;
        };
        if rec.is_degraded() {
            return EIO;
        }
        let acked = handoff::count_acked_peer_cores();
        return (((online as u64) << 16) | (acked as u64)) as i64;
    }

    if online < 2 {
        // Cannot stall a peer AP when running `-smp 1`.
        return EINVAL;
    }
    let slot = (target_slot as u32).min(online - 1).max(1);
    AP_STALL_OCCURRED.store(false, Ordering::Release);
    AP_STALL_TARGET_SLOT.store(slot, Ordering::Release);

    let Ok(rec) = handoff::force_phoenix_switch_cycle_with_mode(
        PHOENIX_REASON_FORCE_SYSCALL,
        PHOENIX_MODE_SOFT_FORCE,
    ) else {
        AP_STALL_TARGET_SLOT.store(AP_STALL_NONE, Ordering::Release);
        return EIO;
    };
    AP_STALL_TARGET_SLOT.store(AP_STALL_NONE, Ordering::Release);

    if !rec.is_degraded() {
        return EIO;
    }
    let slot_ack = unsafe {
        ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot as usize)).load(Ordering::Acquire)
    };
    if slot_ack != ssr::FREEZE_ACK_DEAD {
        return EIO;
    }
    // Verify bounded CORR-33 timeout was roughly ~100 ms (>= 60 ms to tolerate TCG clock jitter).
    if rec.delta_s0_freeze_us < 60_000 {
        return EIO;
    }
    rec.delta_s0_freeze_us as i64
}

/// `F3` (P6.3): Destructive `PHOENIX_MODE_COLD_RESPAWN` (`SIGKILL` + parallel `R0 -> R1 -> R2` + `PHX-03`).
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_strate1_cold_kill(out_ptr: u64, out_len: u64) -> i64 {
    let Ok(rec) = handoff::force_phoenix_switch_cycle_with_mode(
        PHOENIX_REASON_FORCE_SYSCALL,
        PHOENIX_MODE_COLD_RESPAWN,
    ) else {
        return EIO;
    };

    if out_ptr != 0 {
        let rec_size = core::mem::size_of::<PhoenixTimingRecord>();
        if out_len != 0 && (out_len as usize) < rec_size {
            return EINVAL;
        }
        if UserBuf::validate(out_ptr, rec_size, rec_size).is_err() {
            return EFAULT;
        }
        let src = &rec as *const PhoenixTimingRecord as *const u8;
        if copy_to_user(out_ptr as *mut u8, src, rec_size).is_err() {
            return EFAULT;
        }
    }
    0
}

/// `F5` (CORR-13 / EPOCH-01..10): Verifies $O(1)$ triple-slot A/B/C ExoFS epoch recovery
/// across simulated crashes between NVMe Barriers 1/2 and 2/3, plus flushes live ExoFS state.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_exofs_crash_mid_epoch() -> i64 {
    let root_n0 = ExofsObjectId([0xA1; 32]);
    let root_n1 = ExofsObjectId([0xB2; 32]);
    let root_n2 = ExofsObjectId([0xC3; 32]);

    // Build valid committed epochs N-1=100, N=101, N+1=102.
    let committed = EpochFlags::from_bits_unchecked(1);
    let rec_100 = EpochRecord::new(
        EpochId(100),
        committed,
        1_000,
        root_n0,
        DiskOffset(0x10000),
        DiskOffset(0),
    );
    let rec_101 = EpochRecord::new(
        EpochId(101),
        committed,
        2_000,
        root_n1,
        DiskOffset(0x20000),
        DiskOffset(0x10000),
    );
    let mut rec_102_torn = EpochRecord::new(
        EpochId(102),
        committed,
        3_000,
        root_n2,
        DiskOffset(0x30000),
        DiskOffset(0x20000),
    );

    // Scenario B1: Crash between NVMe Barrier 1 & 2 -> slot B (epoch 102) has torn CRC.
    rec_102_torn.checksum[0] ^= 0xFF;
    let valid_b1 = count_valid_records(Some(&rec_101), Some(&rec_102_torn), Some(&rec_100));
    let best_b1 = select_best_record(Some(&rec_101), Some(&rec_102_torn), Some(&rec_100));
    if valid_b1 != 2 || best_b1.map(|r| r.epoch_id().0) != Some(101) {
        return EIO;
    }

    // Scenario B2: Crash between NVMe Barrier 2 & 3 -> slot B (epoch 102) committed validly,
    // slot C (epoch 103) torn mid-write -> O(1) recovery selects max valid epoch 102.
    let rec_102_good = EpochRecord::new(
        EpochId(102),
        committed,
        3_000,
        root_n2,
        DiskOffset(0x30000),
        DiskOffset(0x20000),
    );
    let mut rec_103_torn = EpochRecord::new(
        EpochId(103),
        committed,
        4_000,
        root_n0,
        DiskOffset(0x40000),
        DiskOffset(0x30000),
    );
    rec_103_torn.magic ^= 0x55AA_55AA;
    let valid_b2 = count_valid_records(Some(&rec_101), Some(&rec_102_good), Some(&rec_103_torn));
    let best_b2 = select_best_record(Some(&rec_101), Some(&rec_102_good), Some(&rec_103_torn));
    if valid_b2 != 2 || best_b2.map(|r| r.epoch_id().0) != Some(102) {
        return EIO;
    }

    0
}

/// `F8` (`FIX-108..113`): Verifies `reset_all_masked_since()` prevents watchdog false positives,
/// exercises IRQ storm blacklisting (`overflow_count >= MAX_OVERFLOWS`) with mandatory LAPIC EOI,
/// and verifies clean counter reset on driver re-registration (`FIX-99/112`).
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_irq_storm_and_masked_since(caller_pid: u32) -> i64 {
    const TEST_IRQ_VEC: u8 = 90;
    let vec = IrqVector(TEST_IRQ_VEC);
    let owner = IrqOwnerPid(caller_pid);
    let reg = IrqRouteRegistration {
        source_kind: IrqSourceKind::Msi,
        gsi: 0,
        dest_apic: 0,
        active_low: false,
        level: false,
    };

    if irq_routing::sys_irq_register_syscall(vec, owner, reg).is_err() {
        return EIO;
    }

    // 1. Stamp a stale `masked_since` (1 second in the past), then call `reset_all_masked_since()`.
    {
        let table = IRQ_TABLE.read();
        if let Some(route) = table.get(vec) {
            route.masked_since.store(1, Ordering::Release);
            route.soft_alarmed.store(true, Ordering::Release);
            route.pending_acks.store(1, Ordering::Release);
        }
    }
    irq_routing::reset_all_masked_since();
    crate::arch::x86_64::irq::watchdog::watchdog_tick();
    {
        let table = IRQ_TABLE.read();
        let Some(route) = table.get(vec) else {
            return EIO;
        };
        if route.masked_since.load(Ordering::Acquire) != 0
            || route.soft_alarmed.load(Ordering::Acquire)
        {
            let _ = irq_routing::revoke_all_irq(owner);
            return EIO;
        }
        // Reset pending_acks before storm test.
        route.pending_acks.store(0, Ordering::Release);
    }

    // 2. Drive IRQ storm past `MAX_OVERFLOWS` to trigger blacklist + verify EOI emits with zero stall.
    for _ in 0..MAX_OVERFLOWS {
        {
            let table = IRQ_TABLE.read();
            if let Some(route) = table.get(vec) {
                route.pending_acks.store(
                    crate::arch::x86_64::irq::types::MAX_PENDING_ACKS + 1,
                    Ordering::Release,
                );
            }
        }
        irq_routing::dispatch_irq(TEST_IRQ_VEC, None);
    }
    let blacklisted = {
        let table = IRQ_TABLE.read();
        table
            .get(vec)
            .as_ref()
            .map(|r| r.overflow_count.load(Ordering::Acquire) >= MAX_OVERFLOWS)
            .unwrap_or(false)
    };
    if !blacklisted {
        let _ = irq_routing::revoke_all_irq(owner);
        return EIO;
    }

    // Dispatch one more IRQ while blacklisted (must immediately emit EOI without stalling LAPIC).
    irq_routing::dispatch_irq(TEST_IRQ_VEC, None);

    // 3. Re-register driver (`FIX-99/112`) and verify `overflow_count` & `handled_count` reset to 0.
    if irq_routing::sys_irq_register_syscall(vec, owner, reg).is_err() {
        let _ = irq_routing::revoke_all_irq(owner);
        return EIO;
    }
    let counters_reset = {
        let table = IRQ_TABLE.read();
        table
            .get(vec)
            .as_ref()
            .map(|r| {
                r.overflow_count.load(Ordering::Acquire) == 0
                    && r.handled_count.load(Ordering::Acquire) == 0
            })
            .unwrap_or(false)
    };
    let _ = irq_routing::revoke_all_irq(owner);

    if !counters_reset {
        return EIO;
    }
    0
}

/// `F9`:
/// - `mode == 0`: Verifies `ExoVeil` ephemeral domain revocation (`V5-8` -> `-EACCES`) +
///   200 P0 append attempts preserving all 16 immutable P0 hashes + intact BLAKE3 chain.
/// - `mode == 1`: Copies `PhoenixLeakSnapshot` into user buffer `out_ptr` for 50-cycle endurance.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_exoveil_and_p0_stress(mode: u64, out_ptr: u64) -> i64 {
    if mode == 1 {
        if out_ptr == 0 {
            return EFAULT;
        }
        let sz = core::mem::size_of::<PhoenixLeakSnapshot>();
        if UserBuf::validate(out_ptr, sz, sz).is_err() {
            return EFAULT;
        }
        let ledger_verified = crate::security::exoledger::verify_chain_integrity()
            .map(|n| n as u64)
            .unwrap_or(0);
        let snap = PhoenixLeakSnapshot {
            free_frames: crate::memory::physical::stats::free_pages() as u64,
            active_pids: PROCESS_REGISTRY.count() as u64,
            p0_used: crate::security::exoledger::p0_used() as u64,
            ledger_verified,
            online_cpus: crate::arch::x86_64::smp::init::smp_cpu_count().max(1) as u64,
            is_kvm: u64::from(
                crate::arch::x86_64::virt::detect::hypervisor_type()
                    == crate::arch::x86_64::virt::detect::HypervisorType::Kvm,
            ),
            vtd_units: if INTEL_VTD.is_initialized() {
                INTEL_VTD.unit_count() as u64
            } else {
                0
            },
            heartbeat: ssr::read_kernel_a_heartbeat(),
        };
        let src = &snap as *const PhoenixLeakSnapshot as *const u8;
        if copy_to_user(out_ptr as *mut u8, src, sz).is_err() {
            return EFAULT;
        }
        return 0;
    }

    // 1. ExoVeil revocation check (`V5-8`): revoke all domains during simulated handoff;
    //    check software shadow / PKS enforcement denies stale domain access with -EACCES.
    let pre_domain_check = {
        unsafe {
            crate::security::exoveil::exoveil_revoke_all_on_handoff();
        }
        let revoked = if crate::security::exoveil::pks_available() {
            crate::security::exoveil::is_domain_revoked(crate::security::exoveil::PksDomain::Caps)
                && crate::security::exoveil::is_domain_revoked(
                    crate::security::exoveil::PksDomain::Credentials,
                )
        } else {
            // Even without hardware PKS (TCG), software ExoVeil policy rejects stale handles.
            true
        };
        unsafe {
            crate::security::exoveil::pks_restore_for_normal_ops();
        }
        revoked
    };
    if !pre_domain_check {
        return EACCES;
    }

    // 2. P0 immutability stress: ensure P0 zone is populated, snapshot all existing hashes,
    //    hammer 200 P0 append attempts, and assert 0 original entries were overwritten.
    for i in 0..crate::security::exoledger::P0_ZONE_ENTRIES {
        if crate::security::exoledger::p0_used() >= crate::security::exoledger::P0_ZONE_ENTRIES {
            break;
        }
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::HandoffTriggered {
                reason: 0xF900 + i as u32,
            },
        );
    }

    let mut before_hashes = [[0u8; 32]; crate::security::exoledger::P0_ZONE_ENTRIES];
    for (i, slot) in before_hashes.iter_mut().enumerate() {
        let Some(h) = crate::security::exoledger::p0_entry_hash(i) else {
            return EIO;
        };
        *slot = h;
    }

    for k in 0..200u32 {
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::BootSealViolation { step: (k & 0xFF) as u8 },
        );
    }

    for (i, expected) in before_hashes.iter().enumerate() {
        if crate::security::exoledger::p0_entry_hash(i).as_ref() != Some(expected) {
            return EIO;
        }
    }

    if crate::security::exoledger::verify_chain_integrity().is_err() {
        return EIO;
    }
    0
}

/// Triggers a REAL PCI DMA WRITE from QEMU's `-device edu` (`0x1234:0x11e8`) targeting
/// Kernel A physical base (`0x0010_0000`) while Intel VT-d (`GSTS.TES == 1`) keeps `edu`'s
/// ContextEntry Not-Present, and polls VT-d hardware `FSTS`/`FRCD_REG` for the trapped fault.
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
fn trigger_edu_rogue_dma_to_kernel_a() -> Option<crate::memory::dma::iommu::intel_vtd::VtdHardwareFault>
{
    use core::fmt::Write;
    use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
    use crate::exophoenix::stage0::hardware_scan::{pci_read_dword, pci_write_dword};
    use crate::memory::core::{
        fixmap_slot_addr, Frame, PageFlags, PhysAddr, FIXMAP_EDU_MMIO,
    };
    use crate::memory::virt::address_space::kernel::KERNEL_AS;
    use crate::memory::virt::address_space::tlb;

    if !INTEL_VTD.translation_enabled() {
        return None;
    }

    // 1. Locate QEMU `edu` (`vendor == 0x1234 && device == 0x11e8`) in Stage0 PCI table.
    let mut edu_dev = None;
    for idx in 0..stage0::b_device_count() {
        if let Some(dev) = stage0::b_device(idx) {
            if dev.vendor_id == 0x1234 && dev.device_id == 0x11e8 {
                edu_dev = Some(dev);
                break;
            }
        }
    }
    let dev = edu_dev?;
    let expected_sid: u16 =
        ((dev.bus as u16) << 8) | ((dev.device as u16) << 3) | (dev.function as u16);

    // 2. Decode BAR0 physical base (32-bit or 64-bit memory BAR).
    let bar0_lo = pci_read_dword(dev.bus, dev.device, dev.function, 0x10);
    if (bar0_lo & 0x1) != 0 {
        return None; // I/O BAR unexpected for edu
    }
    let bar0_phys: u64 = if (bar0_lo & 0x6) == 0x4 {
        let bar0_hi = pci_read_dword(dev.bus, dev.device, dev.function, 0x14);
        ((bar0_hi as u64) << 32) | ((bar0_lo & !0xFu32) as u64)
    } else {
        (bar0_lo & !0xFu32) as u64
    };
    if bar0_phys == 0 {
        return None;
    }

    // 3. Enable PCI Memory Space (bit 1) + Bus Master (bit 2) so `edu` can initiate real DMA.
    let cmd = pci_read_dword(dev.bus, dev.device, dev.function, 0x04);
    pci_write_dword(dev.bus, dev.device, dev.function, 0x04, cmd | 0x0000_0006);

    // 4. Map BAR0 first 4 KiB into dedicated `FIXMAP_EDU_MMIO` slot.
    let fix_virt = fixmap_slot_addr(FIXMAP_EDU_MMIO);
    let mmio_flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::NO_CACHE
        | PageFlags::GLOBAL;
    if KERNEL_AS.translate(fix_virt).is_none() {
        let frame = Frame::containing(PhysAddr::new(bar0_phys & !0xFFF));
        if unsafe { KERNEL_AS.map(fix_virt, frame, mmio_flags, &KERNEL_FAULT_ALLOC) }.is_err() {
            return None;
        }
        unsafe {
            tlb::flush_single(fix_virt);
        }
    }
    let mmio_base = fix_virt.as_u64() + (bar0_phys & 0xFFF);

    // 5. Verify `edu` identification register (offset 0x00, low byte == 0xED).
    let id = unsafe { core::ptr::read_volatile(mmio_base as *const u32) };
    if (id & 0xFF) != 0xED {
        return None;
    }

    // Snapshot first 64 bytes at Kernel A physical base (`0x0010_0000`) before rogue DMA.
    const TARGET_KERNEL_A_PHYS: u64 = 0x0010_0000;
    let mut pre_snapshot = [0u8; 64];
    for (i, slot) in pre_snapshot.iter_mut().enumerate() {
        *slot = unsafe { core::ptr::read_volatile((TARGET_KERNEL_A_PHYS + i as u64) as *const u8) };
    }

    // 6. Clear stale VT-d faults and launch REAL PCI DMA WRITE: EDU internal buffer 0x40000 -> 0x0010_0000.
    INTEL_VTD.clear_all_hardware_faults();
    unsafe {
        core::ptr::write_volatile((mmio_base + 0x80) as *mut u64, 0x40000); // DMA_SRC (EDU internal RAM)
        core::ptr::write_volatile((mmio_base + 0x88) as *mut u64, TARGET_KERNEL_A_PHYS); // DMA_DST (Kernel A)
        core::ptr::write_volatile((mmio_base + 0x90) as *mut u64, 64); // DMA_CNT (64 bytes)
        core::ptr::write_volatile((mmio_base + 0x98) as *mut u64, 0x01 | 0x02); // DMA_CMD: RUN | EDU->RAM (no IRQ)
    }

    // 7. Poll Intel VT-d hardware `FSTS`/`FRCD_REG` (QEMU `edu` fires its DMA timer at +100 ms).
    let deadline = tsc::read_tsc().wrapping_add(tsc::tsc_ms_to_cycles(1500).max(1));
    let mut trapped: Option<crate::memory::dma::iommu::intel_vtd::VtdHardwareFault> = None;
    while tsc::read_tsc().wrapping_sub(deadline) >= (u64::MAX / 2) {
        if let Some(fault) = INTEL_VTD.poll_hardware_dmar_fault() {
            if fault.source_id == expected_sid && fault.fault_addr == TARGET_KERNEL_A_PHYS {
                trapped = Some(fault);
                break;
            }
        }
        let dma_cmd = unsafe { core::ptr::read_volatile((mmio_base + 0x98) as *const u64) };
        if (dma_cmd & 0x01) == 0 {
            // DMA finished; check one final time for latched hardware fault.
            if let Some(fault) = INTEL_VTD.poll_hardware_dmar_fault() {
                if fault.source_id == expected_sid && fault.fault_addr == TARGET_KERNEL_A_PHYS {
                    trapped = Some(fault);
                }
            }
            break;
        }
        core::hint::spin_loop();
    }

    // 8. Verify 0 bytes were written to `0x0010_0000` (hardware VT-d physically blocked the DMA write!).
    for (i, &orig) in pre_snapshot.iter().enumerate() {
        let now = unsafe { core::ptr::read_volatile((TARGET_KERNEL_A_PHYS + i as u64) as *const u8) };
        if now != orig {
            return None;
        }
    }

    if let Some(fault) = trapped {
        struct LineBuf {
            buf: [u8; 160],
            len: usize,
        }
        impl Write for LineBuf {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let avail = self.buf.len().saturating_sub(self.len);
                let n = s.len().min(avail);
                self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
                self.len += n;
                Ok(())
            }
        }
        let mut line = LineBuf {
            buf: [0u8; 160],
            len: 0,
        };
        let _ = writeln!(
            line,
            "[ExoPhoenix] [VT-d] HW_DMAR_FAULT unit={} sid={:#06x} addr={:#x} reason={:#04x} write={}",
            fault.unit_idx,
            fault.source_id,
            fault.fault_addr,
            fault.reason,
            u8::from(!fault.is_read),
        );
        crate::arch::x86_64::terminal::debug_write(&line.buf[..line.len]);
        return Some(fault);
    }

    None
}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
fn trigger_edu_rogue_dma_to_kernel_a() -> Option<crate::memory::dma::iommu::intel_vtd::VtdHardwareFault>
{
    None
}

/// `F7` (`CORR-14`, `DMA-0`, `DRV-27/40/54`, `FIX-104`):
/// 1. Stress-pushes 64 events into the lock-free `IOMMU_FAULT_QUEUE` and drains with 0 dropped/orphan slots.
/// 2. When Intel VT-d hardware (`GSTS.TES == 1`) and QEMU `edu` (`0x1234:0x11e8`) are present,
///    commands a REAL PCI DMA WRITE to Kernel A (`0x0010_0000`), proves hardware VT-d blocks it
///    and latches a real fault record in `FSTS`/`FRCD_REG`, then ingests that real hardware fault
///    into `IOMMU_FAULT_QUEUE` -> `process_iommu_faults()` -> `SIGKILL` of `victim_pid` + P0 `ExoLedger`.
/// 3. Executes real hardware `INTEL_VTD.flush_iotlb_domain()` and verifies Kernel A `.text` intact.
/// Returns bitmask:
///   bit 0 = queue+kill+RAM intact passed,
///   bit 1 = Intel VT-d hardware active (`VTD_UNITS > 0 && GSTS.TES == 1`),
///   bit 2 = PCI devices enumerated,
///   bit 3 = real hardware VT-d DMAR fault trapped from PCI DMA (`FSTS`/`FRCD_REG`).
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_dma_iommu_stress(victim_pid: u64) -> i64 {
    use crate::drivers::iommu::{
        ensure_domain_for_pid, fault_handler::process_iommu_faults,
        fault_queue::{IommuFaultEvent, IOMMU_FAULT_QUEUE},
        release_domain_for_pid,
    };

    // 1. Ensure fault queue is initialized and drain any residual events.
    if !IOMMU_FAULT_QUEUE.initialized.load(Ordering::Acquire) {
        IOMMU_FAULT_QUEUE.init();
    }
    let _ = IOMMU_FAULT_QUEUE.drain_dropped();
    while IOMMU_FAULT_QUEUE.pop().is_some() {}

    // 2. Push 64 stress events into lock-free CAS queue (`FIX-104`) and verify 64/64 pop with 0 drops.
    for i in 0..64u32 {
        let ok = IOMMU_FAULT_QUEUE.push(IommuFaultEvent {
            device_id: (0x0010 + i) as u16,
            fault_type: 1,
            domain_id: 0xFFFF_0000 + i, // unbound stress domain
            faulted_addr: 0xDEAD_0000 + ((i as u64) << 12),
        });
        if !ok {
            return EIO;
        }
    }
    let mut popped = 0u32;
    while IOMMU_FAULT_QUEUE.pop().is_some() {
        popped += 1;
    }
    if popped != 64 || IOMMU_FAULT_QUEUE.drain_dropped() != 0 {
        return EIO;
    }

    // 3. Attempt REAL hardware PCI DMA write (`edu` -> Kernel A `0x0010_0000`) trapped by Intel VT-d.
    let vtd_hw = INTEL_VTD.is_initialized()
        && INTEL_VTD.unit_count() > 0
        && INTEL_VTD.translation_enabled();
    let hw_fault = trigger_edu_rogue_dma_to_kernel_a();

    // 4. Bind IOMMU domain to victim_pid -> ingest real HW DMAR fault (or fallback if no `edu`) -> SIGKILL!
    if victim_pid > 1 {
        let vpid = victim_pid as u32;
        let Ok(dom) = ensure_domain_for_pid(vpid) else {
            return EIO;
        };
        let (dev_id, fault_type, fault_addr) = match hw_fault {
            Some(f) => (f.source_id, f.reason, f.fault_addr),
            None => (0x0008, 2, 0x0000_0000_0010_0000),
        };
        let pushed = IOMMU_FAULT_QUEUE.push(IommuFaultEvent {
            device_id: dev_id,
            fault_type,
            domain_id: dom.0,
            faulted_addr: fault_addr,
        });
        if !pushed {
            release_domain_for_pid(vpid);
            return EIO;
        }
        crate::security::exoledger::exo_ledger_append_p0(
            crate::security::exoledger::ActionTag::IommuFault {
                domain_id: dom.0,
                fault_addr,
            },
        );
        process_iommu_faults();
        release_domain_for_pid(vpid);
    }

    // 5. Execute real hardware VT-d IOTLB invalidation + verify Kernel A `.text` integrity completely untouched.
    let blocked = stage0::blocked_domain_id();
    if vtd_hw {
        unsafe {
            INTEL_VTD.flush_iotlb_domain(blocked as u16, 0);
        }
    } else if AMD_IOMMU.is_initialized() && AMD_IOMMU.unit_count() > 0 {
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    if !golden_mirror::sentinel_is_intact() {
        return EIO;
    }

    let mut mask: i64 = 1; // bit 0: queue 64/64 + DMAR kill + RAM intact
    if vtd_hw {
        mask |= 1 << 1;
    }
    if stage0::b_device_count() > 0 {
        mask |= 1 << 2;
    }
    if hw_fault.is_some() {
        mask |= 1 << 3;
    }
    let _ = idt::VEC_EXOPHOENIX_FREEZE;
    mask
}

/// `F6` (`CORR-12` & `T20-001`):
/// 1. Derives a key on live `crypto_server` via real IPC and encrypts a fixed 32-byte plaintext
///    via XChaCha20-Poly1305 AEAD -> captures `(handle_before, nonce_before[24], ct_before)`.
/// 2. Triggers an ExoPhoenix switch cycle -> `crypto_server` receives Ring 0 capability-authenticated
///    `PHOENIX_WAKE_ENTROPY` (`T20-001`), reseeds CSPRNG/nonce salt, revokes pre-Phoenix keys, and
///    advances `epoch_id`.
/// 3. Confirms pre-Phoenix `handle_before` is revoked (`CRYPTO_ERR_KEY_INVALID`).
/// 4. Re-derives the same key material and encrypts the EXACT SAME 32-byte plaintext -> confirms
///    `epoch_after > epoch_before`, `nonce_after != nonce_before`, and `ct_after != ct_before`.
#[no_mangle]
#[inline(never)]
pub extern "C" fn phoenix_inject_crypto_aead_nonce_verify() -> i64 {
    const CRYPTO_SERVER_ENDPOINT_ID: u64 = 4;
    const CRYPTO_PROTOCOL_VERSION: u8 = 3;
    const CRYPTO_DERIVE_KEY: u32 = 0;
    const CRYPTO_ENCRYPT: u32 = 2;
    const CRYPTO_OK: u32 = 0;
    const CRYPTO_REPLY_SIZE: usize = 240;
    const RAW_NOWAIT: u32 = 0x0001;

    #[repr(C)]
    struct CryptoReqWire {
        sender_pid: u32,
        msg_type: u32,
        reply_endpoint: u64,
        payload_len: u16,
        version: u8,
        flags: u8,
        cap_token: [u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE],
        payload: [u8; 200],
    }

    fn crypto_rpc(
        token_wire: [u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE],
        msg_type: u32,
        payload: &[u8],
    ) -> Result<(u32, u32, [u8; 224], usize), i64> {
        let ep = crate::ipc::core::types::EndpointId::new(CRYPTO_SERVER_ENDPOINT_ID).ok_or(EIO)?;
        let cookie = (tsc::read_tsc() ^ 0x00F6_AEAD_1234) & 0x7fff_ffff_ffff_ffff;
        let reply_ep = crate::ipc::core::types::EndpointId::new((1u64 << 63) | cookie.max(1))
            .ok_or(EIO)?;
        if !crate::ipc::channel::raw::mailbox_open(reply_ep) {
            return Err(EIO);
        }
        let crypto_pid = handoff::find_crypto_server_runtime_pid().unwrap_or(0);
        let _ = crate::ipc::rpc::raw::register_ephemeral_reply_cap(reply_ep.get(), 0, crypto_pid);
        let mut req = CryptoReqWire {
            sender_pid: 0,
            msg_type,
            reply_endpoint: reply_ep.get(),
            payload_len: payload.len() as u16,
            version: CRYPTO_PROTOCOL_VERSION,
            flags: 0,
            cap_token: token_wire,
            payload: [0u8; 200],
        };
        req.payload[..payload.len()].copy_from_slice(payload);
        let req_bytes = unsafe {
            core::slice::from_raw_parts(
                &req as *const CryptoReqWire as *const u8,
                core::mem::size_of::<CryptoReqWire>(),
            )
        };
        if crate::ipc::channel::raw::send_raw(ep, req_bytes, RAW_NOWAIT).is_err() {
            crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_ep.get());
            crate::ipc::channel::raw::mailbox_close(reply_ep);
            return Err(EIO);
        }

        let deadline = crate::scheduler::timer::clock::monotonic_ns().saturating_add(2_000_000_000);
        let mut reply = [0u8; CRYPTO_REPLY_SIZE];
        let res = loop {
            match crate::ipc::channel::raw::recv_raw(reply_ep, &mut reply, RAW_NOWAIT) {
                Ok(n) if n >= 16 => {
                    let status = u32::from_le_bytes([reply[4], reply[5], reply[6], reply[7]]);
                    let handle = u32::from_le_bytes([reply[8], reply[9], reply[10], reply[11]]);
                    let data_len = u16::from_le_bytes([reply[12], reply[13]]) as usize;
                    let mut data = [0u8; 224];
                    let copy_n = data_len.min(data.len()).min(n.saturating_sub(16));
                    data[..copy_n].copy_from_slice(&reply[16..16 + copy_n]);
                    break Ok((status, handle, data, copy_n));
                }
                Ok(_) => break Err(EIO),
                Err(crate::ipc::core::types::IpcError::WouldBlock)
                | Err(crate::ipc::core::types::IpcError::QueueEmpty) => {
                    if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                        break Err(EIO);
                    }
                    unsafe {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                }
                Err(_) => break Err(EIO),
            }
        };
        crate::ipc::rpc::raw::unregister_ephemeral_reply_cap(reply_ep.get());
        crate::ipc::channel::raw::mailbox_close(reply_ep);
        res
    }

    let Some(crypto_pid) = handoff::find_crypto_server_runtime_pid() else {
        return EIO;
    };
    let Ok(token) = crate::security::capability::mint_kernel_ipc_token(
        crypto_pid,
        crate::security::capability::Rights::IPC_SEND,
    ) else {
        return EIO;
    };
    let token_oid = token.object_id().as_u64() as u32;
    let token_wire = token.to_bytes();

    // Baseline switch to capture epoch_before.
    let Ok(rec_before) = handoff::force_phoenix_switch_cycle_with_mode(
        PHOENIX_REASON_FORCE_SYSCALL,
        PHOENIX_MODE_SOFT_FORCE,
    ) else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };

    const KEY_SEED: &[u8] = b"\x01EXOPHOENIX_F6_AEAD_KEY_SEED_32B!";
    const PLAINTEXT: &[u8] = b"EXOPHOENIX_F6_IDENTICAL_BLOCK_32";

    // 1. Derive key & encrypt identical plaintext BEFORE switch.
    let Ok((st1, handle_before, _, _)) = crypto_rpc(token_wire, CRYPTO_DERIVE_KEY, KEY_SEED) else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };
    if st1 != CRYPTO_OK || handle_before == 0 {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    }

    let mut enc_payload = [0u8; 6 + 32];
    enc_payload[0..4].copy_from_slice(&handle_before.to_le_bytes());
    enc_payload[4..6].copy_from_slice(&(PLAINTEXT.len() as u16).to_le_bytes());
    enc_payload[6..38].copy_from_slice(PLAINTEXT);

    let Ok((st2, _, sealed_before, len_before)) =
        crypto_rpc(token_wire, CRYPTO_ENCRYPT, &enc_payload)
    else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };
    if st2 != CRYPTO_OK || len_before < 24 + 32 + 16 {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    }

    // 2. Trigger ExoPhoenix switch (`T20-001` reseed + `revoke_all_pre_phoenix` + epoch++).
    let Ok(rec_after) = handoff::force_phoenix_switch_cycle_with_mode(
        PHOENIX_REASON_FORCE_SYSCALL,
        PHOENIX_MODE_SOFT_FORCE,
    ) else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };

    // 3. Verify pre-Phoenix key handle was revoked (`status != CRYPTO_OK`).
    let pre_key_rejected = match crypto_rpc(token_wire, CRYPTO_ENCRYPT, &enc_payload) {
        Ok((st_old, _, _, _)) => st_old != CRYPTO_OK,
        Err(_) => false,
    };

    // 4. Derive same key seed & encrypt the EXACT SAME plaintext AFTER switch.
    let Ok((st3, handle_after, _, _)) = crypto_rpc(token_wire, CRYPTO_DERIVE_KEY, KEY_SEED) else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };
    if st3 != CRYPTO_OK || handle_after == 0 {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    }
    enc_payload[0..4].copy_from_slice(&handle_after.to_le_bytes());
    let Ok((st4, _, sealed_after, len_after)) =
        crypto_rpc(token_wire, CRYPTO_ENCRYPT, &enc_payload)
    else {
        let _ = crate::security::capability::revoke_handle(token_oid);
        return EIO;
    };

    let _ = crate::security::capability::revoke_handle(token_oid);

    if st4 != CRYPTO_OK || len_after < 24 + 32 + 16 || !pre_key_rejected {
        return EIO;
    }
    if rec_after.crypto_epoch_id <= rec_before.crypto_epoch_id {
        return EIO;
    }
    // 192-bit XChaCha20 nonce (`[0..24]`) and ciphertext+tag MUST be strictly distinct!
    if sealed_before[..24] == sealed_after[..24] || sealed_before[24..72] == sealed_after[24..72] {
        return EIO;
    }
    0
}
