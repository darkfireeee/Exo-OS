// kernel/src/security/isolation/namespaces.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Isolation Namespaces — PID/Mount/Network/IPC isolation per namespace
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • NamespaceSet: set of namespaces for a process
//   • Kinds: PID, Mount, Network, IPC, User, UTS
//   • Each namespace has a monotonically increasing unique ID
//   • Namespaces integrate with capability/namespace.rs (NamespaceId)
//   • A process can only see resources within its namespace
//
// RULE NS-01: A process CANNOT access a parent namespace without CAP_NS_ADMIN.
// RULE NS-02: Namespace creation requires CAP_SYS_ADMIN.
// RULE NS-03: A namespace is destroyed when its refcount reaches 0.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Namespace identifiers
// ─────────────────────────────────────────────────────────────────────────────

/// Unique namespace identifier (global, increasing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NsId(pub u32);

impl NsId {
    /// Init namespace (PID 1) — id 0.
    pub const INIT: NsId = NsId(0);

    pub fn is_init(&self) -> bool {
        self.0 == 0
    }
}

// Global counter for NsId allocation
static NS_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

fn alloc_ns_id() -> NsId {
    NsId(NS_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ─────────────────────────────────────────────────────────────────────────────
// Namespace type
// ─────────────────────────────────────────────────────────────────────────────

/// Namespace types supported by Exo-OS.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsKind {
    Pid = 0,
    Mount = 1,
    Network = 2,
    Ipc = 3,
    User = 4,
    Uts = 5,
}

impl NsKind {
    pub const COUNT: usize = 6;
}

// ─────────────────────────────────────────────────────────────────────────────
// Namespace — descriptor of a single namespace
// ─────────────────────────────────────────────────────────────────────────────

/// Namespace descriptor.
pub struct Namespace {
    pub id: NsId,
    pub kind: NsKind,
    /// Parent namespace (None for init).
    pub parent_id: Option<NsId>,
    /// Reference counter — destroyed when 0.
    ref_count: AtomicU32,
    /// Number of processes in this namespace.
    process_count: AtomicU32,
    /// Flags (bitmask).
    flags: AtomicU64,
}

pub mod ns_flags {
    /// Namespace is undergoing destruction.
    pub const DYING: u64 = 1 << 0;
    /// Namespace is a clone of a parent.
    pub const CLONED: u64 = 1 << 1;
    /// Network namespace is isolated (no external access).
    pub const NET_ISOLATED: u64 = 1 << 2;
    /// PID namespace starts at 1 (container).
    pub const PID_CONTAINER: u64 = 1 << 3;
}

impl Namespace {
    pub fn new(kind: NsKind, parent_id: Option<NsId>) -> Self {
        Self {
            id: alloc_ns_id(),
            kind,
            parent_id,
            ref_count: AtomicU32::new(1),
            process_count: AtomicU32::new(0),
            flags: AtomicU64::new(0),
        }
    }

    pub fn new_init(kind: NsKind) -> Self {
        Self {
            id: NsId::INIT,
            kind,
            parent_id: None,
            ref_count: AtomicU32::new(1),
            process_count: AtomicU32::new(0),
            flags: AtomicU64::new(0),
        }
    }

    /// Acquires a reference (increments ref_count).
    pub fn acquire(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Releases a reference. Returns true if the namespace must be destroyed.
    pub fn release(&self) -> bool {
        let prev = self.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            self.flags.fetch_or(ns_flags::DYING, Ordering::Release);
            return true;
        }
        false
    }

    /// Enrolls a process in this namespace.
    pub fn enter(&self) {
        self.process_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Unenrolls a process from this namespace.
    pub fn leave(&self) {
        self.process_count.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn ref_count(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }

    pub fn process_count(&self) -> u32 {
        self.process_count.load(Ordering::Relaxed)
    }

    pub fn is_dying(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & ns_flags::DYING != 0
    }

    /// Checks if a namespace `other` is visible from `self`.
    /// A ns is visible from itself or from a parent namespace.
    pub fn can_see(&self, other: &Namespace) -> bool {
        // Same namespace = always visible
        if self.id == other.id {
            return true;
        }
        // The init namespace is visible by all
        if other.id.is_init() {
            return true;
        }
        // Check parent relationship
        if let Some(parent) = self.parent_id {
            if parent == other.id {
                return true;
            }
        }
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceSet — set of namespaces of a process
// ─────────────────────────────────────────────────────────────────────────────

/// Set of namespaces of a process (one namespace per type).
pub struct NamespaceSet {
    pub pid_ns: NsId,
    pub mnt_ns: NsId,
    pub net_ns: NsId,
    pub ipc_ns: NsId,
    pub user_ns: NsId,
    pub uts_ns: NsId,
}

impl NamespaceSet {
    /// Creates the namespace set for the init process.
    pub const fn init() -> Self {
        Self {
            pid_ns: NsId::INIT,
            mnt_ns: NsId::INIT,
            net_ns: NsId::INIT,
            ipc_ns: NsId::INIT,
            user_ns: NsId::INIT,
            uts_ns: NsId::INIT,
        }
    }

    /// Returns the NsId for a given kind.
    pub fn get(&self, kind: NsKind) -> NsId {
        match kind {
            NsKind::Pid => self.pid_ns,
            NsKind::Mount => self.mnt_ns,
            NsKind::Network => self.net_ns,
            NsKind::Ipc => self.ipc_ns,
            NsKind::User => self.user_ns,
            NsKind::Uts => self.uts_ns,
        }
    }

    /// Creates a clone of the namespace set with a new namespace for `kind`.
    pub fn clone_with_new(&self, kind: NsKind, new_id: NsId) -> Self {
        let mut ns = *self;
        match kind {
            NsKind::Pid => ns.pid_ns = new_id,
            NsKind::Mount => ns.mnt_ns = new_id,
            NsKind::Network => ns.net_ns = new_id,
            NsKind::Ipc => ns.ipc_ns = new_id,
            NsKind::User => ns.user_ns = new_id,
            NsKind::Uts => ns.uts_ns = new_id,
        }
        ns
    }

    /// Checks if the namespace of type `kind` is the init namespace.
    pub fn is_init_ns(&self, kind: NsKind) -> bool {
        self.get(kind) == NsId::INIT
    }
}

impl Copy for NamespaceSet {}
impl Clone for NamespaceSet {
    fn clone(&self) -> Self {
        *self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NsManager — global registry of active namespaces
// ─────────────────────────────────────────────────────────────────────────────

const MAX_NAMESPACES: usize = 256;

struct NsRegistry {
    entries: [Option<Namespace>; MAX_NAMESPACES],
    count: usize,
}

impl NsRegistry {
    const fn new() -> Self {
        // SAFETY: Option<Namespace> is valid all-zero for None (initialized via implicit MaybeUninit)
        const NONE_NS: Option<Namespace> = None;
        Self {
            entries: [NONE_NS; MAX_NAMESPACES],
            count: 0,
        }
    }

    fn find(&self, id: NsId) -> Option<&Namespace> {
        self.entries.iter().flatten().find(|ns| ns.id == id)
    }

    fn register(&mut self, ns: Namespace) -> Result<(), NsError> {
        if self.count >= MAX_NAMESPACES {
            return Err(NsError::RegistryFull);
        }
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                *slot = Some(ns);
                self.count += 1;
                return Ok(());
            }
        }
        Err(NsError::RegistryFull)
    }

    fn remove(&mut self, id: NsId) {
        for slot in self.entries.iter_mut() {
            if let Some(ref ns) = slot {
                if ns.id == id {
                    *slot = None;
                    self.count -= 1;
                    return;
                }
            }
        }
    }
}

static NS_REGISTRY: Mutex<NsRegistry> = Mutex::new(NsRegistry::new());

/// Creates a new namespace of the given type.
pub fn create_namespace(kind: NsKind, parent: Option<NsId>) -> Result<NsId, NsError> {
    let ns = Namespace::new(kind, parent);
    let id = ns.id;
    NS_REGISTRY.lock().register(ns)?;
    Ok(id)
}

/// Destroys a namespace if it is no longer referenced.
pub fn destroy_namespace(id: NsId) {
    let mut reg = NS_REGISTRY.lock();
    if let Some(ns) = reg.find(id) {
        if ns.release() {
            reg.remove(id);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NsError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum NsError {
    NotFound,
    RegistryFull,
    AccessDenied,
    AlreadyDying,
}
