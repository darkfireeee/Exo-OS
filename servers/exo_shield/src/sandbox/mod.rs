//! Sandbox module — filesystem restrictions, network isolation, syscall
//! filtering, and container lifecycle management for the exo_shield
//! security server.

pub mod container;
pub mod fs_restriction;
pub mod net_isolation;
pub mod syscall_filter;

// Re-export the primary public types for ergonomic downstream use.
pub use container::{ContainerId, ContainerProfile, ContainerState, ContainerManager};
pub use fs_restriction::{
    AccessMode, FsPolicy, PathEntry, PathMatcher, FsRestrictionConfig,
};
pub use net_isolation::{
    BandwidthLimit, HostEntry, NetIsolationConfig, Protocol, ProtocolFilter,
};
pub use syscall_filter::{
    SyscallBitmap, SyscallFilterProfile, SyscallViolation, SyscallFilterManager,
};
