// kernel/src/ipc/endpoint/connection.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONNECTION — Établissement de connexion + handshake IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le protocole de connexion est :
//   1. CLIENT : `connect(endpoint_name, token)` → enqueue PendingConnection dans backlog.
//   2. SERVEUR : `accept()` → dequeue PendingConnection, crée un canal bidirectionnel.
//   3. HANDSHAKE : échange de version et de capabilities sur le canal créé.
//   4. ÉTABLI : les deux parties ont leur ChannelId pour communiquer.
//
// RÈGLE : zéro allocation heap — canaux créés depuis un pool statique.
// ═══════════════════════════════════════════════════════════════════════════════

use super::descriptor::{EndpointDesc, PendingConnection};
use crate::ipc::core::constants::RPC_PROTOCOL_VERSION;
use crate::ipc::core::types::{alloc_channel_id, ChannelId, Cookie, EndpointId, IpcError};
use crate::scheduler::core::task::ThreadId;
use crate::security::access_control::{check_access, ObjectKind};
use crate::security::capability::{CapTable, CapToken, Rights};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ConnectionState — état d'une connexion établie
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ConnectionState {
    /// En attente d'acceptation (dans le backlog).
    Pending = 0,
    /// Handshake en cours.
    Handshaking = 1,
    /// Connexion établie, prête à l'emploi.
    Established = 2,
    /// Fermeture initiée par l'un des pairs.
    Closing = 3,
    /// Connexion fermée.
    Closed = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// HandshakeMsg — message échangé lors du handshake
// ─────────────────────────────────────────────────────────────────────────────

/// Message de handshake — 32 bytes.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct HandshakeMsg {
    /// Magic IPC.
    pub magic: u32,
    /// Version du protocole.
    pub version: u8,
    /// Type : 0 = SYN, 1 = SYN-ACK, 2 = ACK.
    pub kind: u8,
    pub _pad: u16,
    /// Cookie pour corrélation.
    pub cookie: u64,
    /// EndpointId cible.
    pub ep_id: u64,
    /// ThreadId de l'initiateur.
    pub tid: u32,
    pub _pad2: u32,
}

impl HandshakeMsg {
    pub const MAGIC: u32 = 0x4950_4348; // "IPCH"
    pub const KIND_SYN: u8 = 0;
    pub const KIND_SYNACK: u8 = 1;
    pub const KIND_ACK: u8 = 2;

    pub fn syn(cookie: Cookie, ep_id: EndpointId, tid: ThreadId) -> Self {
        Self {
            magic: Self::MAGIC,
            version: RPC_PROTOCOL_VERSION,
            kind: Self::KIND_SYN,
            _pad: 0,
            cookie: cookie.get(),
            ep_id: ep_id.get(),
            tid: tid.0 as u32,
            _pad2: 0,
        }
    }

    pub fn syn_ack(cookie: Cookie, ep_id: EndpointId, server_tid: ThreadId) -> Self {
        Self {
            magic: Self::MAGIC,
            version: RPC_PROTOCOL_VERSION,
            kind: Self::KIND_SYNACK,
            _pad: 0,
            cookie: cookie.get(),
            ep_id: ep_id.get(),
            tid: server_tid.0 as u32,
            _pad2: 0,
        }
    }

    pub fn ack(cookie: Cookie) -> Self {
        Self {
            magic: Self::MAGIC,
            version: RPC_PROTOCOL_VERSION,
            kind: Self::KIND_ACK,
            _pad: 0,
            cookie: cookie.get(),
            ep_id: 0,
            tid: 0,
            _pad2: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == Self::MAGIC && self.version == RPC_PROTOCOL_VERSION
    }
}

const _: () = assert!(
    core::mem::size_of::<HandshakeMsg>() == 32,
    "HandshakeMsg = 32B"
);

// ─────────────────────────────────────────────────────────────────────────────
// ActiveConnection — connexion établie
// ─────────────────────────────────────────────────────────────────────────────

/// Connexion active entre deux threads.
#[repr(C, align(32))]
pub struct ActiveConnection {
    /// Identifiant du canal client → serveur.
    pub c2s_channel: ChannelId,
    /// Identifiant du canal serveur → client.
    pub s2c_channel: ChannelId,
    /// Thread client.
    pub client_tid: ThreadId,
    /// Thread serveur.
    pub server_tid: ThreadId,
    /// Endpoint auquel la connexion est rattachée.
    pub endpoint_id: EndpointId,
    /// État de la connexion.
    pub state: AtomicU32,
    /// Cookie de corrélation.
    pub cookie: u64,
    /// Timestamp d'établissement.
    pub established_tick: AtomicU64,
}

impl ActiveConnection {
    pub fn new(
        c2s: ChannelId,
        s2c: ChannelId,
        client: ThreadId,
        server: ThreadId,
        ep_id: EndpointId,
        cookie: Cookie,
    ) -> Self {
        Self {
            c2s_channel: c2s,
            s2c_channel: s2c,
            client_tid: client,
            server_tid: server,
            endpoint_id: ep_id,
            state: AtomicU32::new(ConnectionState::Handshaking as u32),
            cookie: cookie.get(),
            established_tick: AtomicU64::new(0),
        }
    }

    pub fn mark_established(&self, tick: u64) {
        self.state
            .store(ConnectionState::Established as u32, Ordering::Release);
        self.established_tick.store(tick, Ordering::Relaxed);
    }

    pub fn is_established(&self) -> bool {
        self.state.load(Ordering::Acquire) == ConnectionState::Established as u32
    }

    pub fn close(&self) {
        self.state
            .store(ConnectionState::Closed as u32, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ConnectionManager — pool de connexions actives
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un `connect()`.
pub struct ConnectResult {
    pub channel_id: ChannelId,
    pub cookie: Cookie,
}

/// Résultat d'un `accept()`.
pub struct AcceptResult {
    pub connection: PendingConnection,
    pub s2c_channel: ChannelId,
}

/// Effectue la demande de connexion depuis le client.
///
/// # Arguments
/// - `ep_id`  : endpoint cible.
/// - `ep`     : descripteur de l'endpoint.
/// - `client` : thread client.
/// - `table`  : table de capabilities du client.
/// - `token`  : token capability du client.
///
/// # Retour
/// `Ok(ConnectResult)` avec le ChannelId alloué pour recevoir la réponse.
pub fn do_connect(
    _ep_id: EndpointId,
    ep: &EndpointDesc,
    client: ThreadId,
    table: &CapTable,
    token: CapToken,
) -> Result<ConnectResult, IpcError> {
    // Vérification capability : CONNECT requis.
    check_access(
        table,
        token,
        ObjectKind::IpcEndpoint,
        Rights::IPC_CONNECT,
        "ipc/endpoint",
    )
    .map_err(|_| IpcError::PermissionDenied)?;

    if !ep.is_listening() {
        return Err(IpcError::ConnRefused);
    }

    let channel_id = alloc_channel_id();
    let cookie = Cookie::new(channel_id.get() ^ (client.0 as u64));

    let pending = PendingConnection {
        requester: client,
        channel_id: channel_id.get(),
        cookie: cookie.get(),
        timestamp_ticks: 0, // mis à jour par l'appelant avec le tick courant
    };

    ep.enqueue_connection(pending)?;
    Ok(ConnectResult { channel_id, cookie })
}

/// Accepte la prochaine connexion en attente.
///
/// # Arguments
/// - `ep`     : descripteur de l'endpoint.
/// - `server` : thread serveur (doit être propriétaire de l'endpoint).
///
/// # Retour
/// `Ok(AcceptResult)` avec le PendingConnection et le canal serveur→client.
pub fn do_accept(ep: &EndpointDesc, server: ThreadId) -> Result<AcceptResult, IpcError> {
    if !ep.is_owner(server) {
        return Err(IpcError::PermissionDenied);
    }
    let conn = ep.dequeue_connection().ok_or(IpcError::WouldBlock)?;
    let s2c_channel = alloc_channel_id();
    Ok(AcceptResult {
        connection: conn,
        s2c_channel,
    })
}
