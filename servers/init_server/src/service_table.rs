use super::Service;

pub const SERVICE_COUNT: usize = 9;

pub struct ServiceMetadata {
    pub name: &'static str,
    #[allow(dead_code)]
    pub bin_path: &'static [u8],
    pub requires: &'static [&'static str],
    pub ready_timeout_ms: u64,
    pub critical: bool,
}

const NO_DEPS: &[&str] = &[];
const DEPS_MEMORY: &[&str] = &["ipc_router"];
const DEPS_VFS: &[&str] = &["ipc_router", "memory_server"];
const DEPS_CRYPTO: &[&str] = &["vfs_server"];
const DEPS_DEVICE: &[&str] = &["ipc_router", "memory_server"];
const DEPS_VIRTIO: &[&str] = &["device_server"];
const DEPS_NETWORK: &[&str] = &["device_server", "virtio_drivers"];
const DEPS_SCHEDULER: &[&str] = &["init_server"];
const DEPS_EXO_SHIELD: &[&str] = &[
    "ipc_router",
    "memory_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    "virtio_drivers",
    "network_server",
    "scheduler_server",
];

pub static IPC_ROUTER_BIN: &[u8] = b"/sbin/exo-ipc-router\0";
pub static MEMORY_SERVER_BIN: &[u8] = b"/sbin/exo-memory-server\0";
pub static VFS_SERVER_BIN: &[u8] = b"/sbin/exo-vfs-server\0";
pub static CRYPTO_SERVER_BIN: &[u8] = b"/sbin/exo-crypto-server\0";
pub static DEVICE_SERVER_BIN: &[u8] = b"/sbin/exo-device-server\0";
pub static VIRTIO_DRIVERS_BIN: &[u8] = b"/sbin/exo-virtio-drivers\0";
pub static NETWORK_SERVER_BIN: &[u8] = b"/sbin/exo-network-server\0";
pub static SCHEDULER_SERVER_BIN: &[u8] = b"/sbin/exo-scheduler-server\0";
pub static EXO_SHIELD_BIN: &[u8] = b"/sbin/exo-shield\0";

pub static CANONICAL_SERVICES: [ServiceMetadata; SERVICE_COUNT] = [
    ServiceMetadata {
        name: "ipc_router",
        bin_path: IPC_ROUTER_BIN,
        requires: NO_DEPS,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceMetadata {
        name: "memory_server",
        bin_path: MEMORY_SERVER_BIN,
        requires: DEPS_MEMORY,
        ready_timeout_ms: 750,
        critical: true,
    },
    ServiceMetadata {
        name: "vfs_server",
        bin_path: VFS_SERVER_BIN,
        requires: DEPS_VFS,
        ready_timeout_ms: 750,
        critical: true,
    },
    ServiceMetadata {
        name: "crypto_server",
        bin_path: CRYPTO_SERVER_BIN,
        requires: DEPS_CRYPTO,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceMetadata {
        name: "device_server",
        bin_path: DEVICE_SERVER_BIN,
        requires: DEPS_DEVICE,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceMetadata {
        name: "virtio_drivers",
        bin_path: VIRTIO_DRIVERS_BIN,
        requires: DEPS_VIRTIO,
        ready_timeout_ms: 750,
        critical: false,
    },
    ServiceMetadata {
        name: "network_server",
        bin_path: NETWORK_SERVER_BIN,
        requires: DEPS_NETWORK,
        ready_timeout_ms: 500,
        critical: false,
    },
    ServiceMetadata {
        name: "scheduler_server",
        bin_path: SCHEDULER_SERVER_BIN,
        requires: DEPS_SCHEDULER,
        ready_timeout_ms: 500,
        critical: false,
    },
    ServiceMetadata {
        name: "exo_shield",
        bin_path: EXO_SHIELD_BIN,
        requires: DEPS_EXO_SHIELD,
        ready_timeout_ms: 1_000,
        critical: false,
    },
];

#[inline]
pub fn metadata(name: &str) -> Option<&'static ServiceMetadata> {
    CANONICAL_SERVICES.iter().find(|service| service.name == name)
}

#[inline]
pub fn runtime_running_mask(services: &[Service]) -> u64 {
    let mut mask = 0u64;
    let mut idx = 0usize;
    while idx < services.len() {
        if services[idx].current_pid() != 0 {
            mask |= 1u64 << idx;
        }
        idx += 1;
    }
    mask
}

#[inline]
pub fn runtime_index_by_name(services: &[Service], raw_name: &[u8]) -> Option<usize> {
    let mut idx = 0usize;
    while idx < services.len() {
        if name_matches(services[idx].name, raw_name) {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

#[inline]
pub fn name_matches(expected: &str, raw_name: &[u8]) -> bool {
    expected.as_bytes() == raw_name
}
