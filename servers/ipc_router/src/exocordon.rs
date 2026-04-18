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
    Memory = 2,
    Vfs = 3,
    Crypto = 4,
    Device = 5,
    Network = 6,
    Scheduler = 7,
    VirtioBlock = 8,
    VirtioNet = 9,
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

static AUTHORIZED_GRAPH: [AuthEdge; 6] = [
    AuthEdge::new(ServiceId::Init, ServiceId::Memory, 4, 10_000),
    AuthEdge::new(ServiceId::Init, ServiceId::Vfs, 4, 10_000),
    AuthEdge::new(ServiceId::Vfs, ServiceId::Crypto, 2, 50_000),
    AuthEdge::new(ServiceId::Network, ServiceId::Vfs, 2, 100_000),
    AuthEdge::new(ServiceId::Device, ServiceId::VirtioBlock, 1, 1_000_000),
    AuthEdge::new(ServiceId::Device, ServiceId::VirtioNet, 1, 1_000_000),
];

fn service_id_of(raw: Pid) -> Option<ServiceId> {
    match raw {
        1 => Some(ServiceId::Init),
        3 => Some(ServiceId::Vfs),
        4 => Some(ServiceId::Crypto),
        5 => Some(ServiceId::Memory),
        6 => Some(ServiceId::Device),
        7 => Some(ServiceId::Network),
        8 => Some(ServiceId::Scheduler),
        9 => Some(ServiceId::VirtioBlock),
        10 => Some(ServiceId::VirtioNet),
        11 => Some(ServiceId::ExoShield),
        _ => None,
    }
}

fn find_edge(src: ServiceId, dst: ServiceId) -> Option<&'static AuthEdge> {
    AUTHORIZED_GRAPH
        .iter()
        .find(|edge| edge.src == src && edge.dst == dst && edge.depth_max != 0)
}

pub fn check_ipc(src: Pid, dst: Pid) -> Result<(), IpcError> {
    let src = service_id_of(src).ok_or(IpcError::UnknownService)?;
    let dst = service_id_of(dst).ok_or(IpcError::UnknownService)?;
    let edge = find_edge(src, dst).ok_or(IpcError::UnauthorizedPath)?;
    edge.consume_quota()
}

#[cfg(test)]
pub fn reset_quotas() {
    for edge in AUTHORIZED_GRAPH {
        edge.quota_left.store(edge.quota_default, Ordering::Release);
    }
}

#[cfg(test)]
pub fn remaining_quota(src: Pid, dst: Pid) -> Option<u64> {
    let src = service_id_of(src)?;
    let dst = service_id_of(dst)?;
    find_edge(src, dst).map(|edge| edge.quota_left.load(Ordering::Acquire))
}
