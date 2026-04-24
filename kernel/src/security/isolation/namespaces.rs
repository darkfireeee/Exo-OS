// kernel/src/security/isolation/namespaces.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Namespaces d'isolation — Isolation PID/Mount/Network/IPC par namespace
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • NamespaceSet : ensemble de namespaces pour un processus
//   • Types : PID, Mount, Network, IPC, User, UTS
//   • Chaque namespace a un identifiant unique croissant
//   • Les namespaces s'intègrent avec capability/namespace.rs (NamespaceId)
//   • Un processus ne peut voir que les ressources dans son namespace
//
// RÈGLE NS-01 : Un processus ne peut PAS accéder à un namespace parent sans CAP_NS_ADMIN.
// RÈGLE NS-02 : La création d'un namespace requiert CAP_SYS_ADMIN.
// RÈGLE NS-03 : Un namespace est détruit quand son refcount atteint 0.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Identifiants de namespace
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un namespace (global, croissant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NsId(pub u32);

impl NsId {
    /// Namespace init (PID 1) — id 0.
    pub const INIT: NsId = NsId(0);

    pub fn is_init(&self) -> bool {
        self.0 == 0
    }
}

// Compteur global pour l'allocation de NsId
static NS_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

fn alloc_ns_id() -> NsId {
    NsId(NS_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ─────────────────────────────────────────────────────────────────────────────
// Type de namespace
// ─────────────────────────────────────────────────────────────────────────────

/// Types de namespaces supportés par Exo-OS.
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
// Namespace — descripteur d'un namespace unique
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur d'un namespace.
pub struct Namespace {
    pub id: NsId,
    pub kind: NsKind,
    /// Parent namespace (None pour init).
    pub parent_id: Option<NsId>,
    /// Compteur de références — détruit quand 0.
    ref_count: AtomicU32,
    /// Nombre de processus dans ce namespace.
    process_count: AtomicU32,
    /// Flags (bitmask).
    flags: AtomicU64,
}

pub mod ns_flags {
    /// Namespace est en cours de destruction.
    pub const DYING: u64 = 1 << 0;
    /// Namespace est un clone d'un parent.
    pub const CLONED: u64 = 1 << 1;
    /// Namespace réseau est isolé (no external access).
    pub const NET_ISOLATED: u64 = 1 << 2;
    /// Namespace PID commence à 1 (contenant).
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

    /// Acquiert une référence (incrémente ref_count).
    pub fn acquire(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Libère une référence. Retourne true si le namespace doit être détruit.
    pub fn release(&self) -> bool {
        let prev = self.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            self.flags.fetch_or(ns_flags::DYING, Ordering::Release);
            return true;
        }
        false
    }

    /// Enregistre un processus dans ce namespace.
    pub fn enter(&self) {
        self.process_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Désinscrit un processus de ce namespace.
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

    /// Vérifie si un namespace `other` est visible depuis `self`.
    /// Un ns est visible depuis soi-même ou depuis un namespace parent.
    pub fn can_see(&self, other: &Namespace) -> bool {
        // Même namespace = toujours visible
        if self.id == other.id {
            return true;
        }
        // Le namespace init est visible par tous
        if other.id.is_init() {
            return true;
        }
        // Vérifier la relation parent
        if let Some(parent) = self.parent_id {
            if parent == other.id {
                return true;
            }
        }
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceSet — ensemble de namespaces d'un processus
// ─────────────────────────────────────────────────────────────────────────────

/// Ensemble de namespaces d'un processus (un namespace par type).
pub struct NamespaceSet {
    pub pid_ns: NsId,
    pub mnt_ns: NsId,
    pub net_ns: NsId,
    pub ipc_ns: NsId,
    pub user_ns: NsId,
    pub uts_ns: NsId,
}

impl NamespaceSet {
    /// Crée le namespace set du processus init.
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

    /// Retourne le NsId pour un kind donné.
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

    /// Crée un clone du namespace set avec un nouveau namespace pour `kind`.
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

    /// Vérifie si le namespace de type `kind` est le namespace init.
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
// NsManager — registre global des namespaces actifs
// ─────────────────────────────────────────────────────────────────────────────

const MAX_NAMESPACES: usize = 256;

struct NsRegistry {
    entries: [Option<Namespace>; MAX_NAMESPACES],
    count: usize,
}

impl NsRegistry {
    const fn new() -> Self {
        // SAFETY: Option<Namespace> est valide à zéro pour None (initialisé via MaybeUninit implicite)
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

/// Crée un nouveau namespace du type donné.
pub fn create_namespace(kind: NsKind, parent: Option<NsId>) -> Result<NsId, NsError> {
    let ns = Namespace::new(kind, parent);
    let id = ns.id;
    NS_REGISTRY.lock().register(ns)?;
    Ok(id)
}

/// Détruit un namespace s'il n'est plus référencé.
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
