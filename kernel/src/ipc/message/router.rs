// ipc/message/router.rs — Table de routage IPC multi-saut pour Exo-OS
//
// Ce module implémente un routeur de messages IPC : chaque EndpointId peut
// être associé à un next-hop (un autre EndpointId) dans une table statique.
// Le routeur permet le forwarding transparent des messages entre endpoints
// sans connaissance explicite du destinataire final.
//
// Architecture :
//   - `RoutingTable` : tableau statique de MAX_ROUTES entrées (EndpointId src → dst)
//   - `IpcRouter` : encapsule la table avec verrou par lecture/écriture spinlock
//   - `route()` : résout le next-hop en O(1) si table compacte, O(N) sinon
//   - `forward()` : route + retourne le descripteur de message modifié (dst remplacé)
//   - Support de routes multi-sauts (max MAX_HOPS avant déclarer loop)
//
// RÈGLE ROUTE-01 : table statique, pas d'allocation.
// RÈGLE ROUTE-02 : MAX_HOPS = 8 pour éviter les boucles infinies.
// RÈGLE ROUTE-03 : les routes circulaires sont détectées et retournent Err(Loop).

use core::sync::atomic::{AtomicU32, AtomicBool, AtomicU64, Ordering};
use core::num::NonZeroU64;

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::ipc::message::builder::IpcMessage;
use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Constantes
// ---------------------------------------------------------------------------

/// Nombre maximum de routes dans la table
pub const MAX_ROUTES: usize = 512;

/// Nombre maximum de sauts avant de détecter une boucle
pub const MAX_HOPS: usize = 8;

// ---------------------------------------------------------------------------
// Entrée de route
// ---------------------------------------------------------------------------

/// Une entrée dans la table de routage : src_id → next_hop + métadonnées.
#[repr(C, align(32))]
pub struct RouteEntry {
    /// Endpoint source (clé de recherche)
    pub src: AtomicU32,
    /// Next-hop (EndpointId destination)
    pub next_hop: AtomicU32,
    /// Coût de la route (pour sélection de chemin si plusieurs routes vers même src)
    pub cost: AtomicU32,
    /// L'entrée est valide
    pub valid: AtomicBool,
    /// Statistiques de forwarding
    pub fwd_count: AtomicU64,
    _pad: [u8; 3],
}

// SAFETY: tous les champs sont atomiques
unsafe impl Sync for RouteEntry {}
unsafe impl Send for RouteEntry {}

impl RouteEntry {
    pub const fn empty() -> Self {
        Self {
            src: AtomicU32::new(0),
            next_hop: AtomicU32::new(0),
            cost: AtomicU32::new(0),
            valid: AtomicBool::new(false),
            fwd_count: AtomicU64::new(0),
            _pad: [0u8; 3],
        }
    }

    pub fn is_match(&self, ep_id: u32) -> bool {
        self.valid.load(Ordering::Relaxed) && self.src.load(Ordering::Relaxed) == ep_id
    }
}

// ---------------------------------------------------------------------------
// RoutingTable — table statique
// ---------------------------------------------------------------------------

/// Table de routage IPC statique.
///
/// Recherche linéaire O(N) — N ≤ MAX_ROUTES = 512.
/// Pour un noyau avec peu d'endpoints, c'est optimal (cache-friendly).
struct RoutingTable {
    entries: [RouteEntry; MAX_ROUTES],
    count: AtomicU32,
}

// SAFETY: RouteEntry est Sync
unsafe impl Sync for RoutingTable {}

impl RoutingTable {
    const fn new() -> Self {
        const EMPTY: RouteEntry = RouteEntry::empty();
        Self {
            entries: [EMPTY; MAX_ROUTES],
            count: AtomicU32::new(0),
        }
    }

    /// Ajoute ou remplace une route src → next_hop.
    fn add(&self, src: u32, next_hop: u32, cost: u32) -> Result<(), IpcError> {
        // Chercher une entrée existante pour src
        for i in 0..MAX_ROUTES {
            if self.entries[i].valid.load(Ordering::Relaxed)
                && self.entries[i].src.load(Ordering::Relaxed) == src
            {
                // Mise à jour
                self.entries[i].next_hop.store(next_hop, Ordering::Relaxed);
                self.entries[i].cost.store(cost, Ordering::Relaxed);
                return Ok(());
            }
        }

        // Chercher un slot libre
        for i in 0..MAX_ROUTES {
            if !self.entries[i].valid.load(Ordering::Relaxed) {
                self.entries[i].src.store(src, Ordering::Relaxed);
                self.entries[i].next_hop.store(next_hop, Ordering::Relaxed);
                self.entries[i].cost.store(cost, Ordering::Relaxed);
                self.entries[i].valid.store(true, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        }

        Err(IpcError::NullEndpoint)
    }

    /// Supprime la route pour `src`.
    fn remove(&self, src: u32) -> bool {
        for i in 0..MAX_ROUTES {
            if self.entries[i].is_match(src) {
                self.entries[i].valid.store(false, Ordering::Release);
                self.count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Résout le next-hop pour `src`. Retourne None si aucune route.
    fn lookup(&self, src: u32) -> Option<u32> {
        for i in 0..MAX_ROUTES {
            if self.entries[i].is_match(src) {
                self.entries[i].fwd_count.fetch_add(1, Ordering::Relaxed);
                return Some(self.entries[i].next_hop.load(Ordering::Relaxed));
            }
        }
        None
    }

    /// Résout avec détection de boucle (multi-sauts).
    /// Retourne le EndpointId final après MAX_HOPS sauts max.
    fn resolve_final(&self, start: u32) -> Result<u32, IpcError> {
        let mut current = start;
        let mut hops = 0usize;

        loop {
            match self.lookup(current) {
                None => return Ok(current),    // fin de chaîne
                Some(next) => {
                    if next == start || next == current {
                        return Err(IpcError::Loop);
                    }
                    current = next;
                    hops += 1;
                    if hops >= MAX_HOPS {
                        return Err(IpcError::Loop);
                    }
                }
            }
        }
    }

    fn route_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// IpcRouter — routeur principal
// ---------------------------------------------------------------------------

/// Routeur IPC principal.
///
/// Accès concurrent protégé par SpinLock.
pub struct IpcRouter {
    table: SpinLock<RoutingTable>,
    /// Nombre total de messages forwardés
    pub forwarded: AtomicU64,
    /// Nombre de routes non trouvées (miss)
    pub miss: AtomicU64,
    /// Nombre de boucles détectées
    pub loops_detected: AtomicU64,
}

// SAFETY: SpinLock<RoutingTable> est Sync
unsafe impl Sync for IpcRouter {}
unsafe impl Send for IpcRouter {}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            table: SpinLock::new(RoutingTable::new()),
            forwarded: AtomicU64::new(0),
            miss: AtomicU64::new(0),
            loops_detected: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Gestion des routes
    // -----------------------------------------------------------------------

    /// Ajoute une route src → next_hop avec un coût optionnel.
    pub fn add_route(&self, src: EndpointId, next_hop: EndpointId, cost: u32) -> Result<(), IpcError> {
        let guard = self.table.lock();
        guard.add(src.0.get() as u32, next_hop.0.get() as u32, cost)
    }

    /// Supprime la route pour src.
    pub fn remove_route(&self, src: EndpointId) -> bool {
        let guard = self.table.lock();
        guard.remove(src.0.get() as u32)
    }

    /// Lookup direct : next-hop immédiat pour src.
    pub fn next_hop(&self, src: EndpointId) -> Option<EndpointId> {
        let guard = self.table.lock();
        guard.lookup(src.0.get() as u32)
            .and_then(|v| NonZeroU64::new(v as u64))
            .map(EndpointId)
    }

    /// Résolution finale multi-sauts : retourne l'endpoint terminal.
    pub fn resolve(&self, src: EndpointId) -> Result<EndpointId, IpcError> {
        let guard = self.table.lock();
        match guard.resolve_final(src.0.get() as u32) {
            Ok(id) => Ok(NonZeroU64::new(id as u64).map(EndpointId).unwrap_or(EndpointId::INVALID)),
            Err(e) => {
                self.loops_detected.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Nombre de routes actives.
    pub fn route_count(&self) -> u32 {
        let guard = self.table.lock();
        guard.route_count()
    }

    // -----------------------------------------------------------------------
    // Forwarding de messages
    // -----------------------------------------------------------------------

    /// Route un message : résout la destination finale et met à jour msg.dst.
    ///
    /// Si aucune route n'est trouvée, msg.dst reste inchangé (livraison directe).
    pub fn route_message(&self, msg: &mut IpcMessage) -> Result<(), IpcError> {
        let guard = self.table.lock();
        let dst_raw = msg.dst.0.get() as u32;
        match guard.resolve_final(dst_raw) {
            Ok(final_dst) => {
                if final_dst != dst_raw {
                    msg.dst = NonZeroU64::new(final_dst as u64).map(EndpointId).unwrap_or(EndpointId::INVALID);
                    self.forwarded.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                }
                Ok(())
            }
            Err(IpcError::Loop) => {
                self.loops_detected.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::GenericError);
                Err(IpcError::Loop)
            }
            Err(e) => Err(e),
        }
    }

    /// Vérifie si un endpoint est atteignable (chemin existe).
    pub fn is_reachable(&self, dst: EndpointId) -> bool {
        // Un endpoint est toujours lui-même atteignable.
        // La résolution est réussie si pas de boucle.
        let guard = self.table.lock();
        guard.resolve_final(dst.0.get() as u32).is_ok()
    }

    /// Snapshot des statistiques du routeur.
    pub fn snapshot(&self) -> IpcRouterStats {
        IpcRouterStats {
            forwarded: self.forwarded.load(Ordering::Relaxed),
            miss: self.miss.load(Ordering::Relaxed),
            loops_detected: self.loops_detected.load(Ordering::Relaxed),
            route_count: self.route_count(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcRouterStats {
    pub forwarded: u64,
    pub miss: u64,
    pub loops_detected: u64,
    pub route_count: u32,
}

// ---------------------------------------------------------------------------
// Instance globale
// ---------------------------------------------------------------------------

/// Routeur IPC global.
///
/// Toutes les opérations de routage passent par cette instance.
pub static IPC_ROUTER: IpcRouter = IpcRouter::new();

// ---------------------------------------------------------------------------
// API publique
// ---------------------------------------------------------------------------

/// Ajoute une route dans le routeur global.
pub fn router_add(src: EndpointId, next_hop: EndpointId) -> Result<(), IpcError> {
    IPC_ROUTER.add_route(src, next_hop, 1)
}

/// Supprime une route du routeur global.
pub fn router_remove(src: EndpointId) -> bool {
    IPC_ROUTER.remove_route(src)
}

/// Résout le next-hop pour un endpoint.
pub fn router_lookup(src: EndpointId) -> Option<EndpointId> {
    IPC_ROUTER.next_hop(src)
}

/// Résout l'endpoint terminal (multi-sauts).
pub fn router_resolve(src: EndpointId) -> Result<EndpointId, IpcError> {
    IPC_ROUTER.resolve(src)
}

/// Route un message (modifie msg.dst si forwarding nécessaire).
pub fn router_dispatch(msg: &mut IpcMessage) -> Result<(), IpcError> {
    IPC_ROUTER.route_message(msg)
}

/// Statistiques du routeur global.
pub fn router_stats() -> IpcRouterStats {
    IPC_ROUTER.snapshot()
}
