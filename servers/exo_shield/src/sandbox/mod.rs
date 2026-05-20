//! Sandbox module — filesystem restrictions, network isolation, syscall
//! filtering, and container lifecycle management for the exo_shield
//! security server.

pub mod container;
pub mod fs_restriction;
pub mod net_isolation;
pub mod syscall_filter;

// Re-export the primary public types for ergonomic downstream use.
pub use container::{
    container_manager_init, is_pid_quarantined, quarantine_allows_syscall, quarantine_pid,
    release_quarantine, ContainerId, ContainerManager, ContainerProfile, ContainerState,
};
pub use fs_restriction::{AccessMode, FsPolicy, FsRestrictionConfig, PathEntry, PathMatcher};
pub use net_isolation::{BandwidthLimit, HostEntry, NetIsolationConfig, Protocol, ProtocolFilter};
pub use syscall_filter::{
    SyscallBitmap, SyscallFilterManager, SyscallFilterProfile, SyscallViolation,
};

/// Initialize sandbox containment state.
pub fn sandbox_init() {
    container_manager_init();
}
