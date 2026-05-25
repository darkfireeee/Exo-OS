use super::Service;

pub const SERVICE_COUNT: usize = 15;

pub struct ServiceMetadata {
    pub name: &'static str,
    #[allow(dead_code)]
    pub bin_path: &'static [u8],
    pub requires: &'static [&'static str],
    pub requires_optional: &'static [&'static str],
    pub ready_timeout_ms: u64,
    pub critical: bool,
}

const NO_DEPS: &[&str] = &[];
const DEPS_MEMORY: &[&str] = &["ipc_router"];
const DEPS_VFS: &[&str] = &["ipc_router", "memory_server"];
const DEPS_CRYPTO: &[&str] = &["ipc_router", "vfs_server"];
const DEPS_DEVICE: &[&str] = &["ipc_router", "memory_server"];
const DEPS_VIRTIO: &[&str] = &["ipc_router", "device_server"];
const DEPS_NET_DRIVER: &[&str] = &["ipc_router", "device_server"];
const DEPS_LOOPBACK: &[&str] = &["ipc_router"];
const DEPS_NETWORK: &[&str] = &["ipc_router", "vfs_server", "device_server"];
const OPT_DEPS_NETWORK: &[&str] = &["e1000_driver", "virtio_net_driver", "loopback_driver"];
const DEPS_SCHEDULER: &[&str] = &["ipc_router", "memory_server"];
const DEPS_INPUT: &[&str] = &["ipc_router", "device_server"];
const DEPS_TTY: &[&str] = &["ipc_router", "input_server", "vfs_server"];
const DEPS_EXOSH: &[&str] = &["ipc_router", "tty_server", "vfs_server", "exo_shield"];
const DEPS_EXO_SHIELD: &[&str] = &[
    "ipc_router",
    "memory_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    "input_server",
    "tty_server",
];
const OPT_DEPS_EXO_SHIELD: &[&str] = &["virtio_drivers", "network_server", "scheduler_server"];

pub static IPC_ROUTER_BIN: &[u8] = b"/sbin/exo-ipc-router\0";
pub static MEMORY_SERVER_BIN: &[u8] = b"/sbin/exo-memory-server\0";
pub static VFS_SERVER_BIN: &[u8] = b"/sbin/exo-vfs-server\0";
pub static CRYPTO_SERVER_BIN: &[u8] = b"/sbin/exo-crypto-server\0";
pub static DEVICE_SERVER_BIN: &[u8] = b"/sbin/exo-device-server\0";
pub static VIRTIO_DRIVERS_BIN: &[u8] = b"/sbin/exo-virtio-drivers\0";
pub static E1000_DRIVER_BIN: &[u8] = b"/sbin/exo-e1000-driver\0";
pub static VIRTIO_NET_DRIVER_BIN: &[u8] = b"/sbin/exo-virtio-net-driver\0";
pub static LOOPBACK_DRIVER_BIN: &[u8] = b"/sbin/exo-loopback-driver\0";
pub static NETWORK_SERVER_BIN: &[u8] = b"/sbin/exo-network-server\0";
pub static SCHEDULER_SERVER_BIN: &[u8] = b"/sbin/exo-scheduler-server\0";
pub static INPUT_SERVER_BIN: &[u8] = b"/sbin/exo-input-server\0";
pub static TTY_SERVER_BIN: &[u8] = b"/sbin/exo-tty-server\0";
pub static EXOSH_BIN: &[u8] = b"/bin/exosh\0";
pub static EXO_SHIELD_BIN: &[u8] = b"/sbin/exo-shield\0";

pub static CANONICAL_SERVICES: [ServiceMetadata; SERVICE_COUNT] = [
    ServiceMetadata {
        name: "ipc_router",
        bin_path: IPC_ROUTER_BIN,
        requires: NO_DEPS,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: true,
    },
    ServiceMetadata {
        name: "memory_server",
        bin_path: MEMORY_SERVER_BIN,
        requires: DEPS_MEMORY,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: true,
    },
    ServiceMetadata {
        name: "vfs_server",
        bin_path: VFS_SERVER_BIN,
        requires: DEPS_VFS,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 8_000,
        critical: true,
    },
    ServiceMetadata {
        name: "crypto_server",
        bin_path: CRYPTO_SERVER_BIN,
        requires: DEPS_CRYPTO,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 30_000,
        critical: true,
    },
    ServiceMetadata {
        name: "device_server",
        bin_path: DEVICE_SERVER_BIN,
        requires: DEPS_DEVICE,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: true,
    },
    ServiceMetadata {
        name: "virtio_drivers",
        bin_path: VIRTIO_DRIVERS_BIN,
        requires: DEPS_VIRTIO,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: false,
    },
    ServiceMetadata {
        name: "e1000_driver",
        bin_path: E1000_DRIVER_BIN,
        requires: DEPS_NET_DRIVER,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 8_000,
        critical: false,
    },
    ServiceMetadata {
        name: "virtio_net_driver",
        bin_path: VIRTIO_NET_DRIVER_BIN,
        requires: DEPS_NET_DRIVER,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 8_000,
        critical: false,
    },
    ServiceMetadata {
        name: "loopback_driver",
        bin_path: LOOPBACK_DRIVER_BIN,
        requires: DEPS_LOOPBACK,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: false,
    },
    ServiceMetadata {
        name: "network_server",
        bin_path: NETWORK_SERVER_BIN,
        requires: DEPS_NETWORK,
        requires_optional: OPT_DEPS_NETWORK,
        ready_timeout_ms: 10_000,
        critical: false,
    },
    ServiceMetadata {
        name: "scheduler_server",
        bin_path: SCHEDULER_SERVER_BIN,
        requires: DEPS_SCHEDULER,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 3_000,
        critical: false,
    },
    ServiceMetadata {
        name: "input_server",
        bin_path: INPUT_SERVER_BIN,
        requires: DEPS_INPUT,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: true,
    },
    ServiceMetadata {
        name: "tty_server",
        bin_path: TTY_SERVER_BIN,
        requires: DEPS_TTY,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: true,
    },
    ServiceMetadata {
        name: "exo_shield",
        bin_path: EXO_SHIELD_BIN,
        requires: DEPS_EXO_SHIELD,
        requires_optional: OPT_DEPS_EXO_SHIELD,
        ready_timeout_ms: 30_000,
        critical: true,
    },
    ServiceMetadata {
        name: "exosh",
        bin_path: EXOSH_BIN,
        requires: DEPS_EXOSH,
        requires_optional: NO_DEPS,
        ready_timeout_ms: 5_000,
        critical: false,
    },
];

#[inline]
pub fn metadata(name: &str) -> Option<&'static ServiceMetadata> {
    CANONICAL_SERVICES
        .iter()
        .find(|service| service.name == name)
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
