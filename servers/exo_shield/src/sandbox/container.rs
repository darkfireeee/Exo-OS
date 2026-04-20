//! Container isolation for the exo_shield sandbox.
//!
//! Provides `ContainerProfile` (PID, fs-root, net-namespace, syscall-filter)
//! with a maximum of 8 containers and full lifecycle management
//! (create → start → stop → destroy).

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use super::fs_restriction::FsRestrictionConfig;
use super::net_isolation::NetIsolationConfig;
use super::syscall_filter::{SyscallBitmap, SyscallFilterProfile};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concurrent containers.
pub const MAX_CONTAINERS: usize = 8;

/// Maximum length of a filesystem root path.
pub const MAX_FS_ROOT_LEN: usize = 128;

/// Maximum length of a network namespace name.
pub const MAX_NET_NS_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Container ID
// ---------------------------------------------------------------------------

/// Opaque container identifier (wraps a u32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ContainerId(u32);

impl ContainerId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Sentinel value for an invalid / unassigned ID.
    pub const fn invalid() -> Self {
        Self(u32::MAX)
    }

    pub const fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }
}

// ---------------------------------------------------------------------------
// Container state
// ---------------------------------------------------------------------------

/// Lifecycle state of a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum ContainerState {
    /// Created but not yet running.
    Created = 0,
    /// Currently running.
    Running = 1,
    /// Paused (frozen).
    Paused = 2,
    /// Stopped (can be restarted).
    Stopped = 3,
    /// Destroyed (slot free for reuse).
    Destroyed = 4,
}

impl ContainerState {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ContainerState::Created),
            1 => Some(ContainerState::Running),
            2 => Some(ContainerState::Paused),
            3 => Some(ContainerState::Stopped),
            4 => Some(ContainerState::Destroyed),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ---------------------------------------------------------------------------
// Container profile
// ---------------------------------------------------------------------------

/// Full isolation profile for a single container.
#[derive(Debug)]
#[repr(C)]
pub struct ContainerProfile {
    /// Unique container ID.
    id: ContainerId,
    /// Primary PID (init process) inside the container.
    pid: u32,
    /// Filesystem root path (null-terminated, zero-padded).
    fs_root: [u8; MAX_FS_ROOT_LEN],
    /// Length of the fs_root path.
    fs_root_len: u16,
    /// Network namespace name (null-terminated).
    net_namespace: [u8; MAX_NET_NS_LEN],
    /// Length of the net namespace name.
    net_ns_len: u16,
    /// Syscall filter bitmap for this container.
    syscall_filter: SyscallBitmap,
    /// Filesystem restriction config.
    fs_config: FsRestrictionConfig,
    /// Network isolation config.
    net_config: NetIsolationConfig,
    /// Lifecycle state.
    state: AtomicU8,
    /// Violation count (total across all violations).
    violation_count: AtomicU32,
    /// Whether the slot is in use.
    active: bool,
}

impl ContainerProfile {
    /// Create a new container profile.
    pub fn new(
        id: ContainerId,
        pid: u32,
        fs_root: &[u8],
        net_namespace: &[u8],
        syscall_filter: SyscallBitmap,
    ) -> Option<Self> {
        if fs_root.len() >= MAX_FS_ROOT_LEN || net_namespace.len() >= MAX_NET_NS_LEN {
            return None;
        }
        let mut fs_buf = [0u8; MAX_FS_ROOT_LEN];
        let mut ns_buf = [0u8; MAX_NET_NS_LEN];
        let mut i = 0;
        while i < fs_root.len() {
            fs_buf[i] = fs_root[i];
            i += 1;
        }
        let mut j = 0;
        while j < net_namespace.len() {
            ns_buf[j] = net_namespace[j];
            j += 1;
        }
        Some(Self {
            id,
            pid,
            fs_root: fs_buf,
            fs_root_len: fs_root.len() as u16,
            net_namespace: ns_buf,
            net_ns_len: net_namespace.len() as u16,
            syscall_filter,
            fs_config: FsRestrictionConfig::new(
                super::fs_restriction::FsPolicy::WhitelistFirst,
                super::fs_restriction::AccessMode::new(super::fs_restriction::AccessMode::NONE),
            ),
            net_config: NetIsolationConfig::new_deny_all(),
            state: AtomicU8::new(ContainerState::Created.as_u8()),
            violation_count: AtomicU32::new(0),
            active: true,
        })
    }

    /// Create an empty (inactive) profile slot.
    pub const fn empty() -> Self {
        Self {
            id: ContainerId::invalid(),
            pid: 0,
            fs_root: [0u8; MAX_FS_ROOT_LEN],
            fs_root_len: 0,
            net_namespace: [0u8; MAX_NET_NS_LEN],
            net_ns_len: 0,
            syscall_filter: SyscallBitmap::deny_all(),
            fs_config: FsRestrictionConfig::new(
                super::fs_restriction::FsPolicy::WhitelistFirst,
                super::fs_restriction::AccessMode::new(super::fs_restriction::AccessMode::NONE),
            ),
            net_config: NetIsolationConfig::new_deny_all(),
            state: AtomicU8::new(ContainerState::Destroyed as u8),
            violation_count: AtomicU32::new(0),
            active: false,
        }
    }

    // -- Accessors ----------------------------------------------------------

    pub fn id(&self) -> ContainerId { self.id }
    pub fn pid(&self) -> u32 { self.pid }
    pub fn state(&self) -> ContainerState {
        ContainerState::from_u8(self.state.load(Ordering::Acquire))
            .unwrap_or(ContainerState::Destroyed)
    }
    pub fn is_active(&self) -> bool { self.active }
    pub fn violation_count(&self) -> u32 {
        self.violation_count.load(Ordering::Relaxed)
    }

    pub fn fs_root(&self) -> &[u8] {
        &self.fs_root[..self.fs_root_len as usize]
    }

    pub fn net_namespace(&self) -> &[u8] {
        &self.net_namespace[..self.net_ns_len as usize]
    }

    pub fn syscall_filter(&self) -> &SyscallBitmap { &self.syscall_filter }
    pub fn syscall_filter_mut(&mut self) -> &mut SyscallBitmap { &mut self.syscall_filter }
    pub fn fs_config(&self) -> &FsRestrictionConfig { &self.fs_config }
    pub fn fs_config_mut(&mut self) -> &mut FsRestrictionConfig { &mut self.fs_config }
    pub fn net_config(&self) -> &NetIsolationConfig { &self.net_config }
    pub fn net_config_mut(&mut self) -> &mut NetIsolationConfig { &mut self.net_config }

    /// Check whether a syscall is allowed for this container.
    pub fn check_syscall(&self, nr: u8) -> bool {
        self.syscall_filter.is_allowed(nr)
    }

    /// Record a violation and increment the counter.
    pub fn record_violation(&self) {
        self.violation_count.fetch_add(1, Ordering::Relaxed);
    }

    // -- State transitions --------------------------------------------------

    /// Transition to Running (only from Created or Stopped).
    pub fn start(&self) -> bool {
        loop {
            let cur = self.state.load(Ordering::Acquire);
            let cur_state = ContainerState::from_u8(cur).unwrap_or(ContainerState::Destroyed);
            if cur_state != ContainerState::Created && cur_state != ContainerState::Stopped {
                return false;
            }
            if self.state.compare_exchange(
                cur,
                ContainerState::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                return true;
            }
        }
    }

    /// Transition to Stopped (only from Running or Paused).
    pub fn stop(&self) -> bool {
        loop {
            let cur = self.state.load(Ordering::Acquire);
            let cur_state = ContainerState::from_u8(cur).unwrap_or(ContainerState::Destroyed);
            if cur_state != ContainerState::Running && cur_state != ContainerState::Paused {
                return false;
            }
            if self.state.compare_exchange(
                cur,
                ContainerState::Stopped as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                return true;
            }
        }
    }

    /// Transition to Paused (only from Running).
    pub fn pause(&self) -> bool {
        loop {
            let cur = self.state.load(Ordering::Acquire);
            let cur_state = ContainerState::from_u8(cur).unwrap_or(ContainerState::Destroyed);
            if cur_state != ContainerState::Running {
                return false;
            }
            if self.state.compare_exchange(
                cur,
                ContainerState::Paused as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                return true;
            }
        }
    }

    /// Transition to Running from Paused.
    pub fn resume(&self) -> bool {
        loop {
            let cur = self.state.load(Ordering::Acquire);
            let cur_state = ContainerState::from_u8(cur).unwrap_or(ContainerState::Destroyed);
            if cur_state != ContainerState::Paused {
                return false;
            }
            if self.state.compare_exchange(
                cur,
                ContainerState::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                return true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Container manager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of up to `MAX_CONTAINERS` containers.
pub struct ContainerManager {
    containers: [ContainerProfile; MAX_CONTAINERS],
    /// Monotonically increasing ID counter.
    next_id: AtomicU32,
    /// Generation counter.
    generation: AtomicU32,
}

impl ContainerManager {
    /// Create a new manager with no containers.
    pub const fn new() -> Self {
        Self {
            containers: [ContainerProfile::empty(); MAX_CONTAINERS],
            next_id: AtomicU32::new(1),
            generation: AtomicU32::new(0),
        }
    }

    /// Create a new container and place it in the `Created` state.
    /// Returns the container ID on success, or `ContainerId::invalid()`.
    pub fn create(
        &mut self,
        pid: u32,
        fs_root: &[u8],
        net_namespace: &[u8],
        syscall_filter: SyscallBitmap,
    ) -> ContainerId {
        // Find a free slot.
        let slot = {
            let mut found: Option<usize> = None;
            for (i, c) in self.containers.iter().enumerate() {
                if !c.is_active() {
                    found = Some(i);
                    break;
                }
            }
            found
        };
        let slot = match slot {
            Some(s) => s,
            None => return ContainerId::invalid(),
        };

        let id = ContainerId::new(self.next_id.fetch_add(1, Ordering::Relaxed));
        let profile = match ContainerProfile::new(id, pid, fs_root, net_namespace, syscall_filter) {
            Some(p) => p,
            None => return ContainerId::invalid(),
        };
        self.containers[slot] = profile;
        self.generation.fetch_add(1, Ordering::Release);
        id
    }

    /// Start a container (Created/Stopped → Running).
    pub fn start(&self, id: ContainerId) -> bool {
        if let Some(c) = self.find_by_id(id) {
            c.start()
        } else {
            false
        }
    }

    /// Stop a container (Running/Paused → Stopped).
    pub fn stop(&self, id: ContainerId) -> bool {
        if let Some(c) = self.find_by_id(id) {
            c.stop()
        } else {
            false
        }
    }

    /// Pause a running container.
    pub fn pause(&self, id: ContainerId) -> bool {
        if let Some(c) = self.find_by_id(id) {
            c.pause()
        } else {
            false
        }
    }

    /// Resume a paused container.
    pub fn resume(&self, id: ContainerId) -> bool {
        if let Some(c) = self.find_by_id(id) {
            c.resume()
        } else {
            false
        }
    }

    /// Destroy a container (any state → Destroyed, slot freed).
    /// The container must be stopped first.
    pub fn destroy(&mut self, id: ContainerId) -> bool {
        for c in &mut self.containers {
            if c.is_active() && c.id() == id {
                let state = c.state();
                if state == ContainerState::Running || state == ContainerState::Paused {
                    return false; // must stop first
                }
                *c = ContainerProfile::empty();
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Get an immutable reference to a container by ID.
    pub fn get(&self, id: ContainerId) -> Option<&ContainerProfile> {
        self.find_by_id(id)
    }

    /// Get a mutable reference to a container by ID.
    pub fn get_mut(&mut self, id: ContainerId) -> Option<&mut ContainerProfile> {
        for c in &mut self.containers {
            if c.is_active() && c.id() == id {
                return Some(c);
            }
        }
        None
    }

    /// Count active containers.
    pub fn active_count(&self) -> u32 {
        let mut count = 0u32;
        for c in &self.containers {
            if c.is_active() {
                count += 1;
            }
        }
        count
    }

    /// Generation counter.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    // -- Internal -----------------------------------------------------------

    fn find_by_id(&self, id: ContainerId) -> Option<&ContainerProfile> {
        for c in &self.containers {
            if c.is_active() && c.id() == id {
                return Some(c);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_lifecycle() {
        let mut mgr = ContainerManager::new();
        let bm = SyscallBitmap::deny_all();
        let id = mgr.create(1, b"/jail/app1", b"ns1", bm);
        assert!(id.is_valid());

        let c = mgr.get(id).unwrap();
        assert_eq!(c.state(), ContainerState::Created);
        assert_eq!(c.pid(), 1);

        assert!(mgr.start(id));
        assert_eq!(mgr.get(id).unwrap().state(), ContainerState::Running);

        assert!(mgr.pause(id));
        assert_eq!(mgr.get(id).unwrap().state(), ContainerState::Paused);

        assert!(mgr.resume(id));
        assert_eq!(mgr.get(id).unwrap().state(), ContainerState::Running);

        assert!(mgr.stop(id));
        assert_eq!(mgr.get(id).unwrap().state(), ContainerState::Stopped);

        // Cannot destroy while running — but it's stopped, so:
        assert!(mgr.destroy(id));
        assert!(mgr.get(id).is_none());
    }

    #[test]
    fn container_max_limit() {
        let mut mgr = ContainerManager::new();
        let bm = SyscallBitmap::deny_all();
        let mut ids = [ContainerId::invalid(); MAX_CONTAINERS + 2];
        for i in 0..MAX_CONTAINERS {
            ids[i] = mgr.create(i as u32, b"/jail", b"ns", bm);
            assert!(ids[i].is_valid());
        }
        // Next one should fail
        let overflow = mgr.create(99, b"/jail", b"ns", bm);
        assert!(!overflow.is_valid());
    }

    #[test]
    fn container_cannot_destroy_running() {
        let mut mgr = ContainerManager::new();
        let bm = SyscallBitmap::deny_all();
        let id = mgr.create(1, b"/jail", b"ns", bm);
        mgr.start(id).unwrap();
        assert!(!mgr.destroy(id)); // running — must stop first
        mgr.stop(id).unwrap();
        assert!(mgr.destroy(id));
    }

    #[test]
    fn container_syscall_check() {
        let mut mgr = ContainerManager::new();
        let mut bm = SyscallBitmap::deny_all();
        bm.allow(0);
        bm.allow(1);
        let id = mgr.create(1, b"/jail", b"ns", bm);
        let c = mgr.get(id).unwrap();
        assert!(c.check_syscall(0));
        assert!(!c.check_syscall(2));
    }
}
