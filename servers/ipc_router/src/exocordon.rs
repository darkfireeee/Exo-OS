use core::sync::atomic::{AtomicU64, Ordering};

pub type Pid = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcError {
    UnknownService,
    UnauthorizedPath,
    QuotaExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum ServiceId {
    Init = 1,
    IpcBroker = 2,
    Memory = 3,
    Vfs = 4,
    Crypto = 5,
    Device = 6,
    Network = 7,
    Scheduler = 8,
    VirtioDrivers = 9,
    ExoShield = 10,
}

pub struct AuthEdge {
    src: ServiceId,
    dst: ServiceId,
    depth_max: u8,
    #[allow(dead_code)]
    quota_default: u64,
    quota_left: AtomicU64,
}

impl AuthEdge {
    const fn new(src: ServiceId, dst: ServiceId, depth_max: u8, quota_default: u64) -> Self {
        Self {
            src,
            dst,
            depth_max,
            quota_default,
            quota_left: AtomicU64::new(quota_default),
        }
    }

    fn consume_quota(&self) -> Result<(), IpcError> {
        let mut current = self.quota_left.load(Ordering::Acquire);
        loop {
            if current == 0 {
                return Err(IpcError::QuotaExhausted);
            }
            match self.quota_left.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(next) => current = next,
            }
        }
    }
}

static AUTHORIZED_GRAPH: [AuthEdge; 5] = [
    AuthEdge::new(ServiceId::Init, ServiceId::Memory, 4, 10_000),
    AuthEdge::new(ServiceId::Init, ServiceId::Vfs, 4, 10_000),
    AuthEdge::new(ServiceId::Vfs, ServiceId::Crypto, 2, 50_000),
    AuthEdge::new(ServiceId::Network, ServiceId::Vfs, 2, 100_000),
    AuthEdge::new(ServiceId::Device, ServiceId::VirtioDrivers, 1, 1_000_000),
];

const _: () = assert!(
    ServiceId::ExoShield as u8 == 10,
    "ExoShield doit rester le dernier service Ring1 actuel"
);

static LAST_REFILL_TSC: AtomicU64 = AtomicU64::new(0);
const REFILL_INTERVAL_TSC: u64 = 3_000_000_000;

fn service_id_of(raw: Pid) -> Option<ServiceId> {
    match raw {
        1 => Some(ServiceId::Init),
        2 => Some(ServiceId::IpcBroker),
        3 => Some(ServiceId::Memory),
        4 => Some(ServiceId::Vfs),
        5 => Some(ServiceId::Crypto),
        6 => Some(ServiceId::Device),
        7 => Some(ServiceId::Network),
        8 => Some(ServiceId::Scheduler),
        9 => Some(ServiceId::VirtioDrivers),
        10 => Some(ServiceId::ExoShield),
        _ => None,
    }
}

fn find_edge(src: ServiceId, dst: ServiceId) -> Option<&'static AuthEdge> {
    AUTHORIZED_GRAPH
        .iter()
        .find(|edge| edge.src == src && edge.dst == dst && edge.depth_max != 0)
}

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: lecture TSC locale, sans effet de bord mémoire.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

fn maybe_refill_quotas() {
    let now = read_tsc();
    let last = LAST_REFILL_TSC.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < REFILL_INTERVAL_TSC {
        return;
    }
    if LAST_REFILL_TSC
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    for edge in AUTHORIZED_GRAPH.iter() {
        let current = edge.quota_left.load(Ordering::Acquire);
        let refill = (edge.quota_default / 10).max(1);
        let new_val = current.saturating_add(refill).min(edge.quota_default);
        edge.quota_left.store(new_val, Ordering::Release);
    }
}

pub fn check_ipc(src: Pid, dst: Pid) -> Result<(), IpcError> {
    maybe_refill_quotas();
    let src = service_id_of(src).ok_or(IpcError::UnknownService)?;
    let dst = service_id_of(dst).ok_or(IpcError::UnknownService)?;
    if src == ServiceId::IpcBroker {
        return Ok(());
    }
    let edge = find_edge(src, dst).ok_or(IpcError::UnauthorizedPath)?;
    edge.consume_quota()
}

#[cfg(test)]
pub fn reset_quotas() {
    LAST_REFILL_TSC.store(0, Ordering::Release);
    for edge in AUTHORIZED_GRAPH.iter() {
        edge.quota_left.store(edge.quota_default, Ordering::Release);
    }
}

#[cfg(test)]
pub fn remaining_quota(src: Pid, dst: Pid) -> Option<u64> {
    let src = service_id_of(src)?;
    let dst = service_id_of(dst)?;
    find_edge(src, dst).map(|edge| edge.quota_left.load(Ordering::Acquire))
}
