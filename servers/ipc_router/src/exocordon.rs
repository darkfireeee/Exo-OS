use core::sync::atomic::{AtomicU64, Ordering};

pub type Pid = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcError {
    UnknownService,
    UnauthorizedPath,
    QuotaExhausted,
}

/// Identifiants de services Ring1.
///
/// FIX-EXOCORDON-01 : ajout des services Input(11), Tty(12), Fb(13), Exosh(14),
/// Ps2(15) absents du DAG initial. Sans ces ServiceId, tout IPC depuis/vers ces
/// services retournait systématiquement `IpcError::UnknownService`, bloquant le
/// pipeline d'affichage tty→fb, la chaîne d'entrée et l'interface shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceId {
    Init           = 1,
    IpcBroker      = 2,
    Memory         = 3,
    Vfs            = 4,
    Crypto         = 5,
    Device         = 6,
    Network        = 7,
    Scheduler      = 8,
    VirtioDrivers  = 9,
    ExoShield      = 10,
    // ── Ajouts FIX-EXOCORDON-01 ──────────────────────────────────────────────
    Input          = 11,  // input_server  — hub d'entrée PS/2 + USB HID
    Tty            = 12,  // tty_server    — discipline de ligne, pty, vt100
    Fb             = 13,  // fb_server     — framebuffer GOP Ring1
    Exosh          = 14,  // exosh         — shell interactif
    Ps2            = 15,  // ps2_driver    — driver clavier/souris PS/2
}

impl ServiceId {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1  => Some(Self::Init),
            2  => Some(Self::IpcBroker),
            3  => Some(Self::Memory),
            4  => Some(Self::Vfs),
            5  => Some(Self::Crypto),
            6  => Some(Self::Device),
            7  => Some(Self::Network),
            8  => Some(Self::Scheduler),
            9  => Some(Self::VirtioDrivers),
            10 => Some(Self::ExoShield),
            11 => Some(Self::Input),
            12 => Some(Self::Tty),
            13 => Some(Self::Fb),
            14 => Some(Self::Exosh),
            15 => Some(Self::Ps2),
            _  => None,
        }
    }
}

/// Résout un nom de service canonique vers son ServiceId.
///
/// FIX-EXOCORDON-03 : même table que le kernel
/// (`kernel/src/syscall/table.rs::service_class_for_endpoint_name`) pour que
/// la classification routeur/kernel reste identique.
pub fn service_id_for_name(name: &[u8]) -> Option<ServiceId> {
    match name {
        b"memory_server" => Some(ServiceId::Memory),
        b"vfs_server" => Some(ServiceId::Vfs),
        b"crypto_server" => Some(ServiceId::Crypto),
        b"device_server" => Some(ServiceId::Device),
        b"virtio_drivers" | b"e1000_driver" | b"virtio_net_driver" | b"loopback_driver" => {
            Some(ServiceId::VirtioDrivers)
        }
        b"ps2_driver" => Some(ServiceId::Ps2),
        b"network_server" => Some(ServiceId::Network),
        b"scheduler_server" => Some(ServiceId::Scheduler),
        b"input_server" => Some(ServiceId::Input),
        b"tty_server" => Some(ServiceId::Tty),
        b"fb_server" => Some(ServiceId::Fb),
        b"exo_shield" => Some(ServiceId::ExoShield),
        b"exosh" => Some(ServiceId::Exosh),
        _ => None,
    }
}

#[repr(C)]
pub struct AuthEdge {
    pub src: ServiceId,
    pub dst: ServiceId,
    pub depth_max: u8,
    #[allow(dead_code)]
    pub quota_default: u64,
    pub quota_left: AtomicU64,
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

/// DAG ExoCordon — miroir EXACT de `kernel/src/security/ipc_policy.rs::POLICY`.
///
/// FIX-EXOCORDON-01 : le DAG original ne contenait que 5 arêtes, bloquant
/// silencieusement 46/51 chemins (pipeline affichage, chaîne d'entrée,
/// exosh↔services).
///
/// FIX-EXOCORDON-02 (exoos_ipc_incoherences.md §1, Security_Application_Audit
/// GAP-03) : alignement strict sur les 51 paires de la politique kernel.
/// Règle IPC-01 : le DAG est une SUR-COUCHE de la politique kernel, pas un
/// remplacement — il ne doit ni ouvrir un chemin que le kernel refuse en direct
/// (le routeur, étant IpcBroker, blanchirait le chemin), ni en fermer un que le
/// kernel autorise. Les 9 arêtes hors-politique (Init→Network/Input/Tty/Fb/Exosh,
/// ExoShield→Memory/Vfs/Device, Exosh→Vfs) ont été retirées et les 16 paires
/// kernel manquantes ajoutées. L'arête IpcBroker→ExoShield (audit violations)
/// est couverte par le wildcard src==IpcBroker de check_ipc(), identique au
/// comportement kernel (check_direct_ipc).
///
/// Conventions quota (QoS local routeur, sans équivalent kernel) :
///   10_000   — services critiques (init, sécurité)
///   50_000   — services d'infrastructure (fs, crypto, réseau)
///   100_000  — services d'affichage/entrée (tty, fb, input)
///   500_000+ — drivers et shell (haut débit)
static AUTHORIZED_GRAPH: [AuthEdge; 51] = [
    // ── Init ↔ services de base (requêtes + réponses) ────────────────────────
    AuthEdge::new(ServiceId::Init,      ServiceId::Memory,        4, 10_000),
    AuthEdge::new(ServiceId::Memory,    ServiceId::Init,          2, 50_000),
    AuthEdge::new(ServiceId::Init,      ServiceId::Vfs,           4, 10_000),
    AuthEdge::new(ServiceId::Vfs,       ServiceId::Init,          2, 50_000),
    AuthEdge::new(ServiceId::Init,      ServiceId::Crypto,        4, 10_000),
    AuthEdge::new(ServiceId::Crypto,    ServiceId::Init,          2, 50_000),
    AuthEdge::new(ServiceId::Init,      ServiceId::Device,        4, 10_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Init,          2, 50_000),
    AuthEdge::new(ServiceId::Init,      ServiceId::Scheduler,     4, 10_000),
    AuthEdge::new(ServiceId::Scheduler, ServiceId::Init,          2, 50_000),
    AuthEdge::new(ServiceId::Init,      ServiceId::ExoShield,     4, 10_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Init,          2, 10_000),

    // ── Infrastructure fs/crypto/réseau ──────────────────────────────────────
    AuthEdge::new(ServiceId::Vfs,       ServiceId::Crypto,        2, 50_000),
    AuthEdge::new(ServiceId::Crypto,    ServiceId::Vfs,           2, 50_000),
    AuthEdge::new(ServiceId::Vfs,       ServiceId::Network,       2, 50_000),
    AuthEdge::new(ServiceId::Network,   ServiceId::Vfs,           2, 100_000),

    // ── Device ↔ drivers et périphériques ────────────────────────────────────
    AuthEdge::new(ServiceId::Device,    ServiceId::VirtioDrivers, 1, 1_000_000),
    AuthEdge::new(ServiceId::VirtioDrivers, ServiceId::Device,    1, 1_000_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Input,         2, 100_000),
    AuthEdge::new(ServiceId::Input,     ServiceId::Device,        2, 100_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Ps2,           2, 100_000),
    AuthEdge::new(ServiceId::Ps2,       ServiceId::Device,        2, 100_000),

    // ── Chaîne d'entrée : Ps2 ↔ Input ↔ Tty ─────────────────────────────────
    AuthEdge::new(ServiceId::Ps2,       ServiceId::Input,         4, 500_000),
    AuthEdge::new(ServiceId::Input,     ServiceId::Ps2,           2, 100_000),
    AuthEdge::new(ServiceId::Input,     ServiceId::Tty,           4, 100_000),
    AuthEdge::new(ServiceId::Tty,       ServiceId::Input,         2, 100_000),

    // ── Pipeline d'affichage : Tty ↔ Fb, Device ↔ Fb ─────────────────────────
    AuthEdge::new(ServiceId::Tty,       ServiceId::Fb,            4, 500_000),
    AuthEdge::new(ServiceId::Fb,        ServiceId::Tty,           2, 100_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Fb,            2, 100_000),
    AuthEdge::new(ServiceId::Fb,        ServiceId::Device,        2, 100_000),

    // ── Tty ↔ Vfs (journal de session, périphériques fichiers) ──────────────
    AuthEdge::new(ServiceId::Tty,       ServiceId::Vfs,           2, 100_000),
    AuthEdge::new(ServiceId::Vfs,       ServiceId::Tty,           2, 100_000),

    // ── ExoShield ↔ services surveillés ──────────────────────────────────────
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Crypto,        2, 50_000),
    AuthEdge::new(ServiceId::Crypto,    ServiceId::ExoShield,     2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Input,         2, 100_000),
    AuthEdge::new(ServiceId::Input,     ServiceId::ExoShield,     2, 100_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Tty,           2, 100_000),
    AuthEdge::new(ServiceId::Tty,       ServiceId::ExoShield,     2, 100_000),

    // ── Shell exosh ↔ services autorisés ─────────────────────────────────────
    AuthEdge::new(ServiceId::Exosh,     ServiceId::IpcBroker,     4, 500_000),
    AuthEdge::new(ServiceId::Exosh,     ServiceId::Crypto,        2, 100_000),
    AuthEdge::new(ServiceId::Crypto,    ServiceId::Exosh,         2, 100_000),
    AuthEdge::new(ServiceId::Exosh,     ServiceId::Input,         2, 100_000),
    AuthEdge::new(ServiceId::Input,     ServiceId::Exosh,         2, 100_000),
    AuthEdge::new(ServiceId::Exosh,     ServiceId::Tty,           4, 500_000),
    AuthEdge::new(ServiceId::Tty,       ServiceId::Exosh,         4, 500_000),
    AuthEdge::new(ServiceId::Exosh,     ServiceId::ExoShield,     2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Exosh,         2, 50_000),

    // ── Réseau ↔ device/drivers ──────────────────────────────────────────────
    AuthEdge::new(ServiceId::Network,   ServiceId::Device,        2, 100_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Network,       2, 100_000),
    AuthEdge::new(ServiceId::Network,   ServiceId::VirtioDrivers, 1, 1_000_000),
    AuthEdge::new(ServiceId::VirtioDrivers, ServiceId::Network,   1, 1_000_000),
];

const _: () = assert!(
    ServiceId::ExoShield as u8 == 10,
    "ExoShield doit rester à l'identifiant 10 (invariant Strata vague 5)"
);

// FIX-EXOCORDON-02 : miroir strict — même cardinalité que la politique kernel.
// kernel/src/security/ipc_policy.rs vérifie `POLICY.len() == 51` ; toute
// évolution de la politique kernel doit être répercutée ici (et inversement).
const _: () = assert!(
    AUTHORIZED_GRAPH.len() == 51,
    "DAG ExoCordon doit rester le miroir exact des 51 paires de ipc_policy.rs"
);

static LAST_REFILL_TSC: AtomicU64 = AtomicU64::new(0);
const REFILL_INTERVAL_TSC: u64 = 3_000_000_000;

/// Table dynamique PID→ServiceId (FIX-EXOCORDON-03).
///
/// Les PIDs Ring1 sont assignés dynamiquement par init_server selon l'ordre
/// réel des vagues Strata — la table statique ci-dessous ne peut donc pas
/// classifier correctement un expéditeur runtime. Comme le kernel
/// (SERVICE_REGISTRY peuplé à SYS_IPC_CREATE), le routeur enregistre le couple
/// pid→service quand un service s'annonce via IPC_MSG_REGISTER (après
/// validation contre le registre kernel, cf. FIX-REGISTRY-SYNC).
///
/// Encodage lock-free : slot AtomicU64 = pid (bits 0..32) | service_id (bits 32..40).
/// 0 = slot libre.
const MAX_DYNAMIC_SERVICES: usize = 32;
static DYNAMIC_PIDS: [AtomicU64; MAX_DYNAMIC_SERVICES] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; MAX_DYNAMIC_SERVICES]
};

/// Associe un PID runtime à un ServiceId. Retourne false si la table est pleine
/// ou si le PID/classe est réservé (Init=1 et IpcBroker=2 restent statiques).
pub fn register_pid(pid: Pid, id: ServiceId) -> bool {
    if pid <= 2 || id == ServiceId::Init || id == ServiceId::IpcBroker {
        return false;
    }
    let packed = (pid as u64) | ((id as u8 as u64) << 32);
    for slot in DYNAMIC_PIDS.iter() {
        let cur = slot.load(Ordering::Acquire);
        if cur != 0 && cur as u32 == pid {
            slot.store(packed, Ordering::Release);
            return true;
        }
    }
    for slot in DYNAMIC_PIDS.iter() {
        if slot
            .compare_exchange(0, packed, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
    false
}

fn dynamic_service_id_of(pid: Pid) -> Option<ServiceId> {
    for slot in DYNAMIC_PIDS.iter() {
        let cur = slot.load(Ordering::Acquire);
        if cur != 0 && cur as u32 == pid {
            return ServiceId::from_u8((cur >> 32) as u8);
        }
    }
    None
}

/// Résout un PID (ou endpoint fixe) vers un ServiceId.
///
/// FIX-EXOCORDON-01 : ajout des mappings 11–15 (Input, Tty, Fb, Exosh, Ps2).
/// FIX-EXOCORDON-03 : la table dynamique (peuplée par IPC_MSG_REGISTER) prime
/// sur la convention statique ; ajout de l'endpoint fixe 20 (FB_SERVER_ENDPOINT)
/// pour la résolution des destinations adressées par endpoint.
pub fn service_id_of(raw: Pid) -> Option<ServiceId> {
    if let Some(id) = dynamic_service_id_of(raw) {
        return Some(id);
    }
    match raw {
        1  => Some(ServiceId::Init),
        2  => Some(ServiceId::IpcBroker),
        3  => Some(ServiceId::Memory),
        4  => Some(ServiceId::Vfs),
        5  => Some(ServiceId::Crypto),
        6  => Some(ServiceId::Device),
        7  => Some(ServiceId::Network),
        8  => Some(ServiceId::Scheduler),
        9  => Some(ServiceId::VirtioDrivers),
        10 => Some(ServiceId::ExoShield),
        11 => Some(ServiceId::Input),
        12 => Some(ServiceId::Tty),
        13 => Some(ServiceId::Fb),
        14 => Some(ServiceId::Exosh),
        15 => Some(ServiceId::Ps2),
        20 => Some(ServiceId::Fb), // FB_SERVER_ENDPOINT (syscall_abi)
        _  => None,
    }
}

pub fn find_edge(src: ServiceId, dst: ServiceId) -> Option<&'static AuthEdge> {
    AUTHORIZED_GRAPH
        .iter()
        .find(|edge| edge.src == src && edge.dst == dst && edge.depth_max != 0)
}

#[inline(always)]
pub fn read_tsc() -> u64 {
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
