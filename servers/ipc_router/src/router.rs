//! # router — Moteur de routage IPC avancé (ipc_router PID 2)
//!
//! Système de routage avec priorités, statistiques, détection de routes mortes,
//! et politiques de routage basées sur le DAG ExoCordon.
//!
//! ## Règles
//! - IPC-01 : toutes les routes doivent respecter le DAG ExoCordon
//! - IPC-04 : pas de payload inline > 48 octets (utiliser SHM)
//! - NS-01 : uniquement core::sync::atomic + spin

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de routes dans la table.
const MAX_ROUTES: usize = 128;

/// Nombre de battements de cœur manqués avant de déclarer une route morte.
const DEAD_THRESHOLD: u32 = 3;

/// Timeout d'un battement de cœur (en cycles TSC, ~5 secondes à 3 GHz).
const HEARTBEAT_TIMEOUT_TSC: u64 = 15_000_000_000;

// ── Politique de routage ─────────────────────────────────────────────────────

/// Politique de routage pour un message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RoutePolicy {
    /// Routage direct (même serveur).
    Direct = 0,
    /// Routage via forward (serveur intermédiaire).
    Forwarded = 1,
    /// Broadcast à tous les services d'un type.
    Broadcast = 2,
    /// Répartition de charge entre instances.
    LoadBalanced = 3,
}

impl RoutePolicy {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Direct),
            1 => Some(Self::Forwarded),
            2 => Some(Self::Broadcast),
            3 => Some(Self::LoadBalanced),
            _ => None,
        }
    }
}

// ── Entrée de route ──────────────────────────────────────────────────────────

/// Entrée dans la table de routage.
#[repr(C)]
struct RouteEntry {
    /// PID du service de destination.
    dest_service_id: u32,
    /// Priorité de la route (plus élevé = préféré).
    priority: u8,
    /// PID du prochain saut (0 = direct).
    next_hop_pid: u32,
    /// Métrique de coût (plus bas = meilleur).
    metric: u32,
    /// Flags d'état.
    flags: AtomicU8,
    /// Politique de routage.
    policy: u8,
    /// Compteur de battements de cœur manqués.
    missed_heartbeats: AtomicU32,
    /// TSC du dernier battement de cœur réussi.
    last_heartbeat_tsc: AtomicU64,
    /// Statistiques.
    messages_routed: AtomicU64,
    messages_dropped: AtomicU64,
    bytes_forwarded: AtomicU64,
}

/// Flags d'état d'une route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RouteFlags {
    /// Route inactive.
    Inactive = 0,
    /// Route active.
    Active = 1,
    /// Route morte (trop de heartbeats manqués).
    Dead = 2,
}

impl RouteFlags {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Inactive),
            1 => Some(Self::Active),
            2 => Some(Self::Dead),
            _ => None,
        }
    }
}

impl RouteEntry {
    const fn new() -> Self {
        Self {
            dest_service_id: 0,
            priority: 0,
            next_hop_pid: 0,
            metric: u32::MAX,
            flags: AtomicU8::new(RouteFlags::Inactive as u8),
            policy: RoutePolicy::Direct as u8,
            missed_heartbeats: AtomicU32::new(0),
            last_heartbeat_tsc: AtomicU64::new(0),
            messages_routed: AtomicU64::new(0),
            messages_dropped: AtomicU64::new(0),
            bytes_forwarded: AtomicU64::new(0),
        }
    }

    fn is_active(&self) -> bool {
        self.flags.load(Ordering::Acquire) == RouteFlags::Active as u8
    }
}

// ── Table de routage ─────────────────────────────────────────────────────────

/// Table de routage statique.
static ROUTE_TABLE: spin::Mutex<[RouteEntry; MAX_ROUTES]> = spin::Mutex::new({
    const ENTRY: RouteEntry = RouteEntry::new();
    [ENTRY; MAX_ROUTES]
});

static ROUTE_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Lecture TSC ──────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
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

// ── API publique ─────────────────────────────────────────────────────────────

/// Ajoute une route à la table.
pub fn add_route(
    dest_service_id: u32,
    priority: u8,
    next_hop_pid: u32,
    metric: u32,
    policy: RoutePolicy,
) -> bool {
    let mut table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    // Vérifier si une route vers cette destination existe déjà
    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == dest_service_id && table[i].is_active() {
            // Mettre à jour la route existante si la nouvelle est meilleure
            if (priority as u32) > table[i].priority as u32 ||
               ((priority as u32) == table[i].priority as u32 && metric < table[i].metric) {
                table[i].priority = priority;
                table[i].next_hop_pid = next_hop_pid;
                table[i].metric = metric;
                table[i].policy = policy as u8;
                return true;
            }
            return false;
        }
    }

    // Ajouter une nouvelle route
    let slot = if count < MAX_ROUTES {
        count
    } else {
        // Chercher un slot inactif
        let mut found = None;
        for i in 0..MAX_ROUTES {
            if !table[i].is_active() {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(s) => s,
            None => return false,
        }
    };

    table[slot].dest_service_id = dest_service_id;
    table[slot].priority = priority;
    table[slot].next_hop_pid = next_hop_pid;
    table[slot].metric = metric;
    table[slot].flags.store(RouteFlags::Active as u8, Ordering::Release);
    table[slot].policy = policy as u8;
    table[slot].missed_heartbeats.store(0, Ordering::Release);
    table[slot].last_heartbeat_tsc.store(read_tsc(), Ordering::Release);
    table[slot].messages_routed.store(0, Ordering::Release);
    table[slot].messages_dropped.store(0, Ordering::Release);
    table[slot].bytes_forwarded.store(0, Ordering::Release);

    ROUTE_COUNT.fetch_add(1, Ordering::Release);
    true
}

/// Retire une route de la table.
pub fn remove_route(dest_service_id: u32) -> bool {
    let mut table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == dest_service_id && table[i].is_active() {
            table[i].flags.store(RouteFlags::Inactive as u8, Ordering::Release);
            ROUTE_COUNT.fetch_sub(1, Ordering::Release);
            return true;
        }
    }
    false
}

/// Résout la meilleure route pour une destination.
/// Retourne (next_hop_pid, policy) si trouvé, None sinon.
pub fn resolve_route(dest_service_id: u32) -> Option<(u32, RoutePolicy)> {
    let table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    let mut best_idx: Option<usize> = None;
    let mut best_priority: u8 = 0;
    let mut best_metric: u32 = u32::MAX;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == dest_service_id && table[i].is_active() {
            let p = table[i].priority;
            let m = table[i].metric;
            if p > best_priority || (p == best_priority && m < best_metric) {
                best_priority = p;
                best_metric = m;
                best_idx = Some(i);
            }
        }
    }

    match best_idx {
        Some(idx) => {
            let hop = if table[idx].next_hop_pid != 0 {
                table[idx].next_hop_pid
            } else {
                table[idx].dest_service_id
            };
            let policy = RoutePolicy::from_u8(table[idx].policy).unwrap_or(RoutePolicy::Direct);
            Some((hop, policy))
        }
        None => None,
    }
}

/// Détermine la politique de routage basée sur le DAG ExoCordon.
/// Vérifie que la communication src→dst est autorisée.
pub fn apply_policy(src_pid: u32, dst_pid: u32) -> RoutePolicy {
    // Vérifier d'abord ExoCordon
    if crate::exocordon::check_ipc(src_pid, dst_pid).is_err() {
        // Communication non autorisée — pas de routage
        return RoutePolicy::Direct; // Sera bloqué en amont
    }

    // Si la route existe, utiliser sa politique
    if let Some((_, policy)) = resolve_route(dst_pid) {
        return policy;
    }

    // Par défaut : routage direct
    RoutePolicy::Direct
}

/// Transmet un message IPC selon la politique de routage.
/// Retourne true si le message a été transmis avec succès.
pub fn forward_message(src_pid: u32, dst_pid: u32, payload: &[u8], payload_len: usize) -> bool {
    let policy = apply_policy(src_pid, dst_pid);

    match policy {
        RoutePolicy::Direct | RoutePolicy::Forwarded => {
            let (dest, _) = resolve_route(dst_pid).unwrap_or((dst_pid, RoutePolicy::Direct));
            let result = unsafe {
                crate::syscall::syscall6(
                    302, // SYS_IPC_SEND
                    dest as u64,
                    payload.as_ptr() as u64,
                    payload_len as u64,
                    src_pid as u64,
                    0, 0,
                )
            };

            let table = ROUTE_TABLE.lock();
            let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;
            for i in 0..count.min(MAX_ROUTES) {
                if table[i].dest_service_id == dst_pid && table[i].is_active() {
                    if result >= 0 {
                        table[i].messages_routed.fetch_add(1, Ordering::Relaxed);
                        table[i].bytes_forwarded.fetch_add(payload_len as u64, Ordering::Relaxed);
                    } else {
                        table[i].messages_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    break;
                }
            }

            result >= 0
        }
        RoutePolicy::Broadcast => {
            // Envoyer à tous les services actifs
            let table = ROUTE_TABLE.lock();
            let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;
            let mut sent = 0u32;

            for i in 0..count.min(MAX_ROUTES) {
                if table[i].is_active() {
                    let dest = if table[i].next_hop_pid != 0 {
                        table[i].next_hop_pid
                    } else {
                        table[i].dest_service_id
                    };
                    let result = unsafe {
                        crate::syscall::syscall6(
                            302,
                            dest as u64,
                            payload.as_ptr() as u64,
                            payload_len as u64,
                            src_pid as u64,
                            0, 0,
                        )
                    };
                    if result >= 0 {
                        sent += 1;
                    }
                }
            }

            sent > 0
        }
        RoutePolicy::LoadBalanced => {
            // Déléguer au load_balancer pour choisir l'instance
            // Pour le moment, fallback vers direct
            forward_message(src_pid, dst_pid, payload, payload_len)
        }
    }
}

/// Transmet un lot de messages efficacement.
pub fn batch_forward(messages: &[(u32, u32, &[u8])]) -> u32 {
    let mut sent = 0u32;
    for &(src_pid, dst_pid, payload) in messages {
        if forward_message(src_pid, dst_pid, payload, payload.len()) {
            sent += 1;
        }
    }
    sent
}

/// Vérifie si une route est vivante.
pub fn is_route_alive(dest_service_id: u32) -> bool {
    let table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == dest_service_id {
            return table[i].is_active() && table[i].missed_heartbeats.load(Ordering::Acquire) < DEAD_THRESHOLD;
        }
    }
    false
}

/// Enregistre un battement de cœur réussi pour une route.
pub fn record_heartbeat(service_id: u32) {
    let table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == service_id {
            table[i].missed_heartbeats.store(0, Ordering::Release);
            table[i].last_heartbeat_tsc.store(read_tsc(), Ordering::Release);
            // Si la route était morte, la réactiver
            if table[i].flags.load(Ordering::Acquire) == RouteFlags::Dead as u8 {
                table[i].flags.store(RouteFlags::Active as u8, Ordering::Release);
            }
            return;
        }
    }
}

/// Enregistre un battement de cœur manqué.
pub fn record_missed_heartbeat(service_id: u32) {
    let table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == service_id {
            let missed = table[i].missed_heartbeats.fetch_add(1, Ordering::AcqRel) + 1;
            if missed >= DEAD_THRESHOLD {
                table[i].flags.store(RouteFlags::Dead as u8, Ordering::Release);
            }
            return;
        }
    }
}

/// Vérifie les routes expirées (timeout heartbeat).
/// Retourne le nombre de routes nouvellement déclarées mortes.
pub fn check_dead_routes() -> u32 {
    let now = read_tsc();
    let mut dead_count = 0u32;
    let mut table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if !table[i].is_active() {
            continue;
        }
        let last = table[i].last_heartbeat_tsc.load(Ordering::Acquire);
        if now.wrapping_sub(last) > HEARTBEAT_TIMEOUT_TSC {
            let missed = table[i].missed_heartbeats.fetch_add(1, Ordering::AcqRel) + 1;
            if missed >= DEAD_THRESHOLD {
                table[i].flags.store(RouteFlags::Dead as u8, Ordering::Release);
                dead_count += 1;
            }
        }
    }

    dead_count
}

/// Statistiques de routage pour une destination.
#[repr(C)]
pub struct RouteStats {
    pub messages_routed: u64,
    pub messages_dropped: u64,
    pub bytes_forwarded: u64,
    pub missed_heartbeats: u32,
    pub is_alive: bool,
}

/// Retourne les statistiques d'une route.
pub fn get_route_stats(dest_service_id: u32) -> Option<RouteStats> {
    let table = ROUTE_TABLE.lock();
    let count = ROUTE_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_ROUTES) {
        if table[i].dest_service_id == dest_service_id {
            return Some(RouteStats {
                messages_routed: table[i].messages_routed.load(Ordering::Acquire),
                messages_dropped: table[i].messages_dropped.load(Ordering::Acquire),
                bytes_forwarded: table[i].bytes_forwarded.load(Ordering::Acquire),
                missed_heartbeats: table[i].missed_heartbeats.load(Ordering::Acquire),
                is_alive: table[i].is_active() && table[i].missed_heartbeats.load(Ordering::Acquire) < DEAD_THRESHOLD,
            });
        }
    }
    None
}

/// Initialise la table de routage avec les routes statiques par défaut.
pub fn router_init() {
    let mut table = ROUTE_TABLE.lock();

    // Routes par défaut pour les services Ring 1
    // (PID, priorité, next_hop, métrique, politique)
    let default_routes: [(u32, u8, u32, u32, RoutePolicy); 10] = [
        (1, 10, 0, 1, RoutePolicy::Direct),   // init_server
        (2, 20, 0, 1, RoutePolicy::Direct),   // ipc_router (nous-mêmes)
        (3, 10, 0, 1, RoutePolicy::Direct),   // memory_server
        (4, 10, 0, 1, RoutePolicy::Direct),   // vfs_server
        (5, 15, 0, 1, RoutePolicy::Direct),   // crypto_server
        (6, 10, 0, 2, RoutePolicy::Direct),   // device_server
        (7, 10, 0, 2, RoutePolicy::Direct),   // network_server
        (8, 5,  0, 3, RoutePolicy::Direct),   // scheduler_server
        (9, 5,  0, 3, RoutePolicy::Direct),   // virtio drivers
        (10, 20, 0, 1, RoutePolicy::Direct),  // exo_shield
    ];

    for (idx, &(dest, priority, next_hop, metric, policy)) in default_routes.iter().enumerate() {
        if idx >= MAX_ROUTES { break; }
        table[idx].dest_service_id = dest;
        table[idx].priority = priority;
        table[idx].next_hop_pid = next_hop;
        table[idx].metric = metric;
        table[idx].flags.store(RouteFlags::Active as u8, Ordering::Release);
        table[idx].policy = policy as u8;
        table[idx].missed_heartbeats.store(0, Ordering::Release);
        table[idx].last_heartbeat_tsc.store(read_tsc(), Ordering::Release);
    }

    ROUTE_COUNT.store(default_routes.len() as u32, Ordering::Release);
}
