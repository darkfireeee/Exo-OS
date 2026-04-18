//! Dépendances de démarrage Ring1 pour `init_server`.
//!
//! Source canonique :
//! `ExoOS_Corrections_06_Servers_Arborescence.md`
//! + `GI-04-05-06_ExoFS_Phoenix_Servers.md`

pub struct ServiceDependency {
    pub name: &'static str,
    pub requires: &'static [&'static str],
    pub ready_timeout_ms: u64,
    #[allow(dead_code)]
    pub critical: bool,
}

const NO_DEPS: &[&str] = &[];
const DEPS_MEMORY: &[&str] = &["ipc_router"];
const DEPS_VFS: &[&str] = &["ipc_router", "memory_server"];
const DEPS_CRYPTO: &[&str] = &["vfs_server"];
const DEPS_DEVICE: &[&str] = &["ipc_router", "memory_server"];
const DEPS_NETWORK: &[&str] = &["device_server", "virtio_drivers"];
const DEPS_SCHEDULER: &[&str] = &["init_server"];
const DEPS_VIRTIO: &[&str] = &["device_server"];
const DEPS_EXO_SHIELD: &[&str] = &[
    "ipc_router",
    "memory_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    "network_server",
    "scheduler_server",
    "virtio_drivers",
];

pub static CANONICAL_SERVICES: [ServiceDependency; 9] = [
    ServiceDependency {
        name: "ipc_router",
        requires: NO_DEPS,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceDependency {
        name: "memory_server",
        requires: DEPS_MEMORY,
        ready_timeout_ms: 750,
        critical: true,
    },
    ServiceDependency {
        name: "vfs_server",
        requires: DEPS_VFS,
        ready_timeout_ms: 750,
        critical: true,
    },
    ServiceDependency {
        name: "crypto_server",
        requires: DEPS_CRYPTO,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceDependency {
        name: "device_server",
        requires: DEPS_DEVICE,
        ready_timeout_ms: 500,
        critical: true,
    },
    ServiceDependency {
        name: "network_server",
        requires: DEPS_NETWORK,
        ready_timeout_ms: 500,
        critical: false,
    },
    ServiceDependency {
        name: "scheduler_server",
        requires: DEPS_SCHEDULER,
        ready_timeout_ms: 500,
        critical: false,
    },
    ServiceDependency {
        name: "virtio_drivers",
        requires: DEPS_VIRTIO,
        ready_timeout_ms: 500,
        critical: false,
    },
    ServiceDependency {
        name: "exo_shield",
        requires: DEPS_EXO_SHIELD,
        ready_timeout_ms: 1_000,
        critical: false,
    },
];

#[inline]
pub fn metadata(name: &str) -> Option<&'static ServiceDependency> {
    CANONICAL_SERVICES.iter().find(|service| service.name == name)
}

#[inline]
pub fn ready_timeout_ms(name: &str) -> u64 {
    metadata(name)
        .map(|service| service.ready_timeout_ms)
        .unwrap_or(250)
}

#[inline]
pub fn dependencies_satisfied<F>(name: &str, mut has_service: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    let Some(service) = metadata(name) else {
        return false;
    };

    service.requires.iter().copied().all(|dep| has_service(dep))
}

#[allow(dead_code)]
#[inline]
pub fn is_critical(name: &str) -> bool {
    metadata(name).map(|service| service.critical).unwrap_or(false)
}
