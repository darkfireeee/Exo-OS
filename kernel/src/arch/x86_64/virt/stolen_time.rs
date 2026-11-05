//! # arch/x86_64/virt/stolen_time.rs — Time stolen by the hypervisor
//!
//! "Stolen time" is CPU time consumed by the hypervisor or other
//! VMs while this VM was supposed to be executing.
//!
//! ## KVM Steal Time
//! KVM implements this via a per-page shared structure (MSR_KVM_STEAL_TIME).
//! The VM reads `steal` (u64, accumulated nanoseconds) to estimate hypervisor overhead.

use super::super::cpu::msr;
use crate::scheduler::smp::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU64, Ordering};

/// KVM Steal Time MSR
const MSR_KVM_STEAL_TIME: u32 = 0x4b564d03;
const KVM_STEAL_TIME_ENABLE: u64 = 1;

/// KVM Steal Time structure (shared between host and guest)
#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct KvmStealTime {
    pub steal: u64,   // stolen time in ns (accumulated)
    pub version: u32, // incremented by host before/after update (odd = update in progress)
    pub flags: u32,
    pub preempted: u8, // 1 if vCPU is preempted by host
    _pad: [u8; 47],
}

impl KvmStealTime {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        Self {
            steal: 0,
            version: 0,
            flags: 0,
            preempted: 0,
            _pad: [0u8; 47],
        }
    }
}

/// Size of KVM stolen-time table.
///
/// Aligned with the canonical scheduler value to avoid any silent
/// divergence between per-CPU tables.
const STOLEN_TIME_MAX_CPUS: usize = MAX_CPUS;
const _: () = assert!(
    STOLEN_TIME_MAX_CPUS >= MAX_CPUS,
    "STOLEN_TIME_MAX_CPUS doit rester >= MAX_CPUS canonique"
);

#[repr(align(64))]
struct StealTimeTable([KvmStealTime; STOLEN_TIME_MAX_CPUS]);
unsafe impl Sync for StealTimeTable {}

static STEAL_TIME_TABLE: StealTimeTable =
    StealTimeTable([const { KvmStealTime::zeroed() }; STOLEN_TIME_MAX_CPUS]);

static TOTAL_STOLEN_NS: AtomicU64 = AtomicU64::new(0);
static LAST_STEAL_NS: AtomicU64 = AtomicU64::new(0);

// ── Initialization ────────────────────────────────────────────────────────────

/// Registers the steal time page for CPU `cpu_id` with KVM
///
/// Called from `init_percpu_for_bsp/ap()` if KVM steal time is available.
pub fn init_steal_time_for_cpu(cpu_id: u32) {
    use super::detect::{hypervisor_type, kvm_has_steal_time, HypervisorType};
    if !(hypervisor_type() == HypervisorType::Kvm && kvm_has_steal_time()) {
        return;
    }
    if cpu_id as usize >= STOLEN_TIME_MAX_CPUS {
        return;
    }

    let phys_addr = &STEAL_TIME_TABLE.0[cpu_id as usize] as *const KvmStealTime as u64;
    // Address must be physical and 64-byte aligned (guaranteed by #[repr(align(64))])
    // SAFETY: MSR_KVM_STEAL_TIME write from Ring 0 in KVM mode
    unsafe {
        msr::write_msr(MSR_KVM_STEAL_TIME, phys_addr | KVM_STEAL_TIME_ENABLE);
    }
}

// ── Reading ───────────────────────────────────────────────────────────────────

/// Reads steal time value in nanoseconds for CPU `cpu_id`
///
/// Uses stable atomic read (even version field = coherent data).
pub fn read_steal_ns(cpu_id: u32) -> u64 {
    if cpu_id as usize >= STOLEN_TIME_MAX_CPUS {
        return 0;
    }
    let st = &STEAL_TIME_TABLE.0[cpu_id as usize];

    // Read even version (host completed update)
    loop {
        // SAFETY: volatile read of shared structure
        let ver_before = unsafe { core::ptr::read_volatile(&st.version) };
        if ver_before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        // SAFETY: same shared KVM structure, seqlock: even version guarantees coherent data.
        let steal = unsafe { core::ptr::read_volatile(&st.steal) };
        let ver_after = unsafe { core::ptr::read_volatile(&st.version) };
        if ver_before == ver_after {
            return steal;
        }
    }
}

/// Updates total stolen time counter (called from timer tick)
pub fn update_stolen_time(cpu_id: u32) {
    let current = read_steal_ns(cpu_id);
    let prev = LAST_STEAL_NS.swap(current, Ordering::AcqRel);
    if current > prev {
        TOTAL_STOLEN_NS.fetch_add(current - prev, Ordering::Relaxed);
    }
}

/// Returns total stolen time in nanoseconds (since boot)
pub fn stolen_time_ns() -> u64 {
    TOTAL_STOLEN_NS.load(Ordering::Relaxed)
}

/// Returns `true` if current vCPU is currently preempted by hypervisor
pub fn is_preempted(cpu_id: u32) -> bool {
    if cpu_id as usize >= STOLEN_TIME_MAX_CPUS {
        return false;
    }
    // SAFETY: volatile read of preempted field
    unsafe { core::ptr::read_volatile(&STEAL_TIME_TABLE.0[cpu_id as usize].preempted) != 0 }
}
