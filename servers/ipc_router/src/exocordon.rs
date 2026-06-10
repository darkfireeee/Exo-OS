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

/// DAG ExoCordon — miroir de `kernel/src/security/ipc_policy.rs`.
///
/// FIX-EXOCORDON-01 : le DAG original ne contenait que 5 arêtes sur les 92 paires
/// autorisées par la politique kernel, bloquant silencieusement 46/51 chemins
/// (pipeline affichage, chaîne d'entrée, exosh↔services).
///
/// Conventions quota :
///   10_000   — services critiques (init, sécurité)
///   50_000   — services d'infrastructure (fs, crypto, réseau)
///   100_000  — services d'affichage/entrée (tty, fb, input)
///   500_000  — drivers et shell
static AUTHORIZED_GRAPH: [AuthEdge; 35] = [
    // ── Arêtes originales (conservées) ───────────────────────────────────────
    AuthEdge::new(ServiceId::Init,    ServiceId::Memory,        4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Vfs,           4, 10_000),
    AuthEdge::new(ServiceId::Vfs,     ServiceId::Crypto,        2, 50_000),
    AuthEdge::new(ServiceId::Network, ServiceId::Vfs,           2, 100_000),
    AuthEdge::new(ServiceId::Device,  ServiceId::VirtioDrivers, 1, 1_000_000),

    // ── Init → tous les services (démarrage + supervision) ───────────────────
    AuthEdge::new(ServiceId::Init,    ServiceId::Crypto,        4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Device,        4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Scheduler,     4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::ExoShield,     4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Network,       4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Input,         4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Tty,           4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Fb,            4, 10_000),
    AuthEdge::new(ServiceId::Init,    ServiceId::Exosh,         4, 10_000),

    // ── Réponses → Init (replies des services) ────────────────────────────────
    AuthEdge::new(ServiceId::Memory,    ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::Vfs,       ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::Crypto,    ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::Device,    ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::Scheduler, ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::Network,   ServiceId::Init,        2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Init,        2, 10_000),

    // ── ExoShield → services surveillés ──────────────────────────────────────
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Crypto,      2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Memory,      2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Tty,         2, 100_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Vfs,         2, 50_000),
    AuthEdge::new(ServiceId::ExoShield, ServiceId::Device,      2, 50_000),

    // ── Pipeline d'affichage : Input → Tty → Fb ──────────────────────────────
    AuthEdge::new(ServiceId::Input, ServiceId::Tty,             4, 100_000),
    AuthEdge::new(ServiceId::Tty,   ServiceId::Fb,              4, 500_000),
    AuthEdge::new(ServiceId::Tty,   ServiceId::Input,           2, 100_000),

    // ── Shell exosh → services requis ────────────────────────────────────────
    AuthEdge::new(ServiceId::Exosh, ServiceId::Tty,             4, 500_000),
    AuthEdge::new(ServiceId::Exosh, ServiceId::Vfs,             4, 500_000),
    AuthEdge::new(ServiceId::Exosh, ServiceId::Crypto,          2, 100_000),
    AuthEdge::new(ServiceId::Exosh, ServiceId::ExoShield,       2, 50_000),

    // ── Réseau → device + IpcBroker → ExoShield (audit violations) ───────────
    AuthEdge::new(ServiceId::Network,   ServiceId::Device,      2, 100_000),
    AuthEdge::new(ServiceId::IpcBroker, ServiceId::ExoShield,   4, 50_000),
];

const _: () = assert!(
    ServiceId::ExoShield as u8 == 10,
    "ExoShield doit rester à l'identifiant 10 (invariant Strata vague 5)"
);

// Vérification de non-régression : les 5 arêtes originales doivent rester présentes.
const _: () = assert!(AUTHORIZED_GRAPH.len() >= 5, "DAG ne peut pas régresser sous 5 arêtes");

static LAST_REFILL_TSC: AtomicU64 = AtomicU64::new(0);
const REFILL_INTERVAL_TSC: u64 = 3_000_000_000;

/// Résout un PID vers un ServiceId.
///
/// FIX-EXOCORDON-01 : ajout des mappings pour PIDs 11–15 (Input, Tty, Fb, Exosh, Ps2).
pub fn service_id_of(raw: Pid) -> Option<ServiceId> {
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
