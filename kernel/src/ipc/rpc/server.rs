// ipc/rpc/server.rs — Serveur RPC IPC pour Exo-OS
//
// Ce module implémente un serveur RPC IPC capable d'enregistrer jusqu'à
// MAX_RPC_METHODS méthodes, chacune associée à un handler fn pointer.
//
// Architecture :
//   - `MethodHandler` : fn pointer de type `RpcHandlerFn`
//   - `MethodEntry` : entrée de table (MethodId + handler + stats)
//   - `RpcServer` : table de méthodes + endpoint dédié + boucle de dispatch
//   - `RpcServerTable` : table statique de MAX_RPC_SERVERS serveurs
//
// Lifecycle :
//   1. `rpc_server_create(ep)` — crée un serveur sur un endpoint
//   2. `rpc_server_register(idx, method_id, handler)` — enregistre une méthode
//   3. `rpc_server_dispatch(idx, call_buf, reply_buf)` — traite un appel entrant
//
// RÈGLE SERVER-01 : pas de thread dédié ici — le kernel appelle dispatch()
//                   depuis son scheduler de messages IPC.
// RÈGLE SERVER-02 : les handler fn pointers sont statiques (pas de closure).

use core::mem::MaybeUninit;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::ipc::rpc::protocol::{
    MethodId, RpcCallFrame, RpcHeader, RpcReplyFrame, RpcStatus, MAX_RPC_PAYLOAD,
    METHOD_ID_INTROSPECT, METHOD_ID_PING,
};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Constantes
// ---------------------------------------------------------------------------

/// Nombre maximum de méthodes par serveur
pub const MAX_RPC_METHODS: usize = 64;

/// Nombre maximum de serveurs RPC dans la table globale
pub const MAX_RPC_SERVERS: usize = 32;

// ---------------------------------------------------------------------------
// Signature d'un handler de méthode RPC
// ---------------------------------------------------------------------------

/// Signature d'un handler RPC.
///
/// - `call` : trame d'appel déjà désérialisée
/// - `reply_payload` : buffer à remplir par le handler
/// - Retourne : taille du payload de réponse, ou Err(IpcError)
pub type RpcHandlerFn =
    fn(call: &RpcCallFrame, reply_payload: &mut [u8]) -> Result<usize, IpcError>;

// ---------------------------------------------------------------------------
// MethodEntry
// ---------------------------------------------------------------------------

/// Entrée dans la table des méthodes d'un serveur RPC.
#[repr(C, align(32))]
pub struct MethodEntry {
    pub method_id: AtomicU32,
    pub valid: AtomicBool,
    _pad: [u8; 3],
    /// Handler (fn pointer)
    pub handler: core::sync::atomic::AtomicUsize,
    /// Statistiques
    pub call_count: AtomicU64,
    pub error_count: AtomicU64,
}

// SAFETY: tous les champs sont atomiques ou fn pointers
unsafe impl Sync for MethodEntry {}
unsafe impl Send for MethodEntry {}

impl MethodEntry {
    pub const fn empty() -> Self {
        Self {
            method_id: AtomicU32::new(0),
            valid: AtomicBool::new(false),
            _pad: [0u8; 3],
            handler: core::sync::atomic::AtomicUsize::new(0),
            call_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
        }
    }

    pub fn is_match(&self, id: u32) -> bool {
        self.valid.load(Ordering::Relaxed) && self.method_id.load(Ordering::Relaxed) == id
    }

    pub fn handler(&self) -> Option<RpcHandlerFn> {
        let p = self.handler.load(Ordering::Relaxed);
        if p == 0 {
            return None;
        }
        // SAFETY: handler est une fn pointer statique enregistrée via register()
        Some(unsafe { core::mem::transmute(p) })
    }

    pub fn invoke(&self, call: &RpcCallFrame, reply_payload: &mut [u8]) -> Result<usize, IpcError> {
        if let Some(f) = self.handler() {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            match f(call, reply_payload) {
                Ok(n) => Ok(n),
                Err(e) => {
                    self.error_count.fetch_add(1, Ordering::Relaxed);
                    Err(e)
                }
            }
        } else {
            Err(IpcError::NotFound)
        }
    }
}

// ---------------------------------------------------------------------------
// RpcServer
// ---------------------------------------------------------------------------

/// Serveur RPC IPC.
///
/// Enregistre des méthodes et dispatche les appels entrants.
#[repr(C, align(64))]
pub struct RpcServer {
    pub id: u32,
    /// Endpoint associé à ce serveur
    pub endpoint: AtomicU64,
    /// Serveur actif
    pub active: AtomicBool,
    _pad0: [u8; 3],
    /// Table des méthodes
    methods: [MethodEntry; MAX_RPC_METHODS],
    method_count: AtomicU32,
    /// Statistiques globales
    pub total_calls: AtomicU64,
    pub total_errors: AtomicU64,
    pub total_not_found: AtomicU64,
}

// SAFETY: MethodEntry est Sync
unsafe impl Sync for RpcServer {}
unsafe impl Send for RpcServer {}

impl RpcServer {
    pub const fn new(id: u32) -> Self {
        const EMPTY_ME: MethodEntry = MethodEntry::empty();
        Self {
            id,
            endpoint: AtomicU64::new(0),
            active: AtomicBool::new(false),
            _pad0: [0u8; 3],
            methods: [EMPTY_ME; MAX_RPC_METHODS],
            method_count: AtomicU32::new(0),
            total_calls: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_not_found: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    pub fn start(&self, ep: EndpointId) {
        self.endpoint.store(ep.0.get(), Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
        // Enregistrer les méthodes builtins
        let _ = self.register_builtin();
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn register_builtin(&self) -> Result<(), IpcError> {
        self.register(MethodId(METHOD_ID_PING), builtin_ping)?;
        self.register(MethodId(METHOD_ID_INTROSPECT), builtin_introspect)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Enregistrement des méthodes
    // -----------------------------------------------------------------------

    /// Enregistre un handler pour `method_id`.
    pub fn register(&self, method_id: MethodId, handler: RpcHandlerFn) -> Result<(), IpcError> {
        if !method_id.is_valid() {
            return Err(IpcError::Invalid);
        }

        // Mise à jour si déjà existant
        for i in 0..MAX_RPC_METHODS {
            if self.methods[i].is_match(method_id.0) {
                self.methods[i]
                    .handler
                    .store(handler as usize, Ordering::Relaxed);
                return Ok(());
            }
        }

        // Nouveau slot
        for i in 0..MAX_RPC_METHODS {
            if !self.methods[i].valid.load(Ordering::Relaxed) {
                if self.methods[i]
                    .valid
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    self.methods[i]
                        .method_id
                        .store(method_id.0, Ordering::Relaxed);
                    self.methods[i]
                        .handler
                        .store(handler as usize, Ordering::Release);
                    self.method_count.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
            }
        }

        Err(IpcError::Full)
    }

    /// Désenregistre une méthode.
    pub fn unregister(&self, method_id: MethodId) -> bool {
        for i in 0..MAX_RPC_METHODS {
            if self.methods[i].is_match(method_id.0) {
                self.methods[i].valid.store(false, Ordering::Release);
                self.method_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------------

    /// Dispatche un appel RPC depuis un buffer sérialisé.
    ///
    /// - `call_buf` : buffer contenant la trame RpcCallFrame sérialisée
    /// - `reply_buf` : buffer où écrire la trame RpcReplyFrame sérialisée
    ///
    /// Retourne le nombre d'octets écrits dans `reply_buf`.
    pub fn dispatch(&self, call_buf: &[u8], reply_buf: &mut [u8]) -> Result<usize, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        self.total_calls.fetch_add(1, Ordering::Relaxed);

        // Désérialiser l'appel
        let call = RpcCallFrame::deserialize(call_buf).map_err(|_| {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::RpcProtocolError);
            IpcError::Invalid
        })?;

        if !call.header.is_valid() {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::Invalid);
        }

        let method_id = MethodId(call.header.method_id);
        let mut payload_buf = [0u8; MAX_RPC_PAYLOAD];

        // Trouver et invoquer le handler
        let result = self.invoke_method(method_id, &call, &mut payload_buf);

        // Construire la réponse
        let (status, plen) = match result {
            Ok(n) => (RpcStatus::Ok, n),
            Err(IpcError::NotFound) => {
                self.total_not_found.fetch_add(1, Ordering::Relaxed);
                (RpcStatus::MethodNotFound, 0)
            }
            Err(IpcError::Timeout) => (RpcStatus::ServerTimeout, 0),
            Err(IpcError::PermissionDenied) => (RpcStatus::Denied, 0),
            Err(_) => {
                self.total_errors.fetch_add(1, Ordering::Relaxed);
                (RpcStatus::InternalError, 0)
            }
        };

        let mut reply = RpcReplyFrame::empty();
        reply.header = RpcHeader::new_reply(&call.header, status, plen as u32);
        if plen > 0 {
            reply.payload[..plen].copy_from_slice(&payload_buf[..plen]);
        }

        let written = reply.serialize(reply_buf)?;
        IPC_STATS.record(StatEvent::RpcReturn);
        Ok(written)
    }

    fn invoke_method(
        &self,
        method_id: MethodId,
        call: &RpcCallFrame,
        payload_buf: &mut [u8],
    ) -> Result<usize, IpcError> {
        for i in 0..MAX_RPC_METHODS {
            if self.methods[i].is_match(method_id.0) {
                return self.methods[i].invoke(call, payload_buf);
            }
        }
        Err(IpcError::NotFound)
    }

    pub fn snapshot(&self) -> RpcServerStats {
        let ep_raw = self.endpoint.load(Ordering::Relaxed);
        RpcServerStats {
            id: self.id,
            // SAFETY: ep_raw vient d'un store d'un EndpointId valide (non-nul).
            // 0 = serveur non encore démarré → INVALID.
            endpoint: if ep_raw == 0 {
                EndpointId::INVALID
            } else {
                EndpointId(unsafe { NonZeroU64::new_unchecked(ep_raw) })
            },
            method_count: self.method_count.load(Ordering::Relaxed),
            total_calls: self.total_calls.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            total_not_found: self.total_not_found.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RpcServerStats {
    pub id: u32,
    pub endpoint: EndpointId,
    pub method_count: u32,
    pub total_calls: u64,
    pub total_errors: u64,
    pub total_not_found: u64,
}

// ---------------------------------------------------------------------------
// Handlers builtins
// ---------------------------------------------------------------------------

fn builtin_ping(call: &RpcCallFrame, reply: &mut [u8]) -> Result<usize, IpcError> {
    // Echo du payload dans la réponse
    let plen = call.header.payload_len as usize;
    let copy_len = plen.min(reply.len());
    if copy_len > 0 {
        reply[..copy_len].copy_from_slice(&call.payload[..copy_len]);
    }
    Ok(copy_len)
}

fn builtin_introspect(_call: &RpcCallFrame, reply: &mut [u8]) -> Result<usize, IpcError> {
    // Retourne le nombre de méthodes en 4 octets LE
    if reply.len() < 4 {
        return Ok(0);
    }
    // Valeur symbolique — le serveur actuel est accessible via contexte global
    reply[..4].copy_from_slice(&(MAX_RPC_METHODS as u32).to_le_bytes());
    Ok(4)
}

// ---------------------------------------------------------------------------
// Table globale de serveurs RPC
// ---------------------------------------------------------------------------

struct RpcServerSlot {
    server: MaybeUninit<RpcServer>,
    occupied: AtomicBool,
}

impl RpcServerSlot {
    const fn empty() -> Self {
        Self {
            server: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct RpcServerTable {
    slots: [RpcServerSlot; MAX_RPC_SERVERS],
    count: AtomicU32,
}

unsafe impl Sync for RpcServerTable {}

impl RpcServerTable {
    const fn new() -> Self {
        const EMPTY: RpcServerSlot = RpcServerSlot::empty();
        Self {
            slots: [EMPTY; MAX_RPC_SERVERS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32) -> Option<usize> {
        for i in 0..MAX_RPC_SERVERS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel garantit l'exclusivité; server MaybeUninit<RpcServer> write-once.
                    unsafe {
                        (self.slots[i].server.as_ptr() as *mut RpcServer).write(RpcServer::new(id));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&RpcServer> {
        if idx >= MAX_RPC_SERVERS {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { &*self.slots[idx].server.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_RPC_SERVERS {
            return false;
        }
        if let Some(s) = self.get(idx) {
            s.stop();
        }
        if self.slots[idx]
            .occupied
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static RPC_SERVER_TABLE: RpcServerTable = RpcServerTable::new();

// ---------------------------------------------------------------------------
// API publique
// ---------------------------------------------------------------------------

/// Crée un serveur RPC et l'associe à un endpoint.
pub fn rpc_server_create(ep: EndpointId) -> Option<usize> {
    let idx = RPC_SERVER_TABLE.alloc(ep.0.get() as u32)?;
    RPC_SERVER_TABLE.get(idx)?.start(ep);
    Some(idx)
}

/// Enregistre une méthode sur le serveur.
pub fn rpc_server_register(
    idx: usize,
    method_id: MethodId,
    handler: RpcHandlerFn,
) -> Result<(), IpcError> {
    RPC_SERVER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .register(method_id, handler)
}

/// Dispatche un appel RPC entrant.
pub fn rpc_server_dispatch(
    idx: usize,
    call_buf: &[u8],
    reply_buf: &mut [u8],
) -> Result<usize, IpcError> {
    RPC_SERVER_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .dispatch(call_buf, reply_buf)
}

/// Arrête et détruit un serveur.
pub fn rpc_server_destroy(idx: usize) -> bool {
    RPC_SERVER_TABLE.free(idx)
}

/// Statistiques d'un serveur.
pub fn rpc_server_stats(idx: usize) -> Option<RpcServerStats> {
    RPC_SERVER_TABLE.get(idx).map(|s| s.snapshot())
}
