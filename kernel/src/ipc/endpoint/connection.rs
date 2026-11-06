// kernel/src/ipc/endpoint/connection.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONNECTION — Connection establishment + IPC handshake
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Connection protocol:
//   1. CLIENT: `connect(endpoint_name, token)` -> enqueue PendingConnection into backlog.
//   2. SERVER: `accept()` -> dequeue PendingConnection, creates bidirectional channel.
//   3. HANDSHAKE: version and capability exchange on created channel.
//   4. ESTABLISHED: both peers obtain their ChannelId to communicate.
//
// RULE: zero heap allocation — channels created from a static pool.
// ═══════════════════════════════════════════════════════════════════════════════

use super::descriptor::{EndpointDesc, PendingConnection};
use crate::ipc::core::constants::RPC_PROTOCOL_VERSION;
use crate::ipc::core::types::{alloc_channel_id, ChannelId, Cookie, EndpointId, IpcError};
use crate::scheduler::core::task::ThreadId;
use crate::security::capability::{CapTable, CapToken, Rights};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ConnectionState — established connection state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ConnectionState {
    /// Awaiting acceptance (in backlog).
    Pending = 0,
    /// Handshake in progress.
    Handshaking = 1,
    /// Connection established, ready for use.
    Established = 2,
    /// Closure initiated by one peer.
    Closing = 3,
    /// Connection closed.
    Closed = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// HandshakeMsg — message exchanged during handshake
// ─────────────────────────────────────────────────────────────────────────────

/// Handshake message — 32 bytes.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct HandshakeMsg {
    /// IPC Magic.
    pub magic: u32,
    /// Protocol version.
    pub version: u8,
    /// Type: 0 = SYN, 1 = SYN-ACK, 2 = ACK.
    pub kind: u8,
    pub _pad: u16,
    /// Cookie for correlation.
    pub cookie: u64,
    /// Target EndpointId.
    pub ep_id: u64,
    /// Initiator ThreadId.
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
// ActiveConnection — established connection
// ─────────────────────────────────────────────────────────────────────────────

/// Active connection between two threads.
#[repr(C, align(32))]
pub struct ActiveConnection {
    /// Channel identifier client -> server.
    pub c2s_channel: ChannelId,
    /// Channel identifier server -> client.
    pub s2c_channel: ChannelId,
    /// Client thread.
    pub client_tid: ThreadId,
    /// Server thread.
    pub server_tid: ThreadId,
    /// Endpoint to which connection is attached.
    pub endpoint_id: EndpointId,
    /// Connection state.
    pub state: AtomicU32,
    /// Correlation cookie.
    pub cookie: u64,
    /// Establishment timestamp.
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
// ConnectionManager — active connection pool
// ─────────────────────────────────────────────────────────────────────────────

/// Result of `connect()`.
pub struct ConnectResult {
    pub channel_id: ChannelId,
    pub cookie: Cookie,
}

/// Result of `accept()`.
pub struct AcceptResult {
    pub connection: PendingConnection,
    pub s2c_channel: ChannelId,
}

/// Performs connection request from client.
///
/// # Arguments
/// - `ep_id`  : target endpoint.
/// - `ep`     : endpoint descriptor.
/// - `client` : client thread.
/// - `table`  : client capability table.
/// - `token`  : client capability token.
///
/// # Return
/// `Ok(ConnectResult)` with allocated ChannelId to receive reply.
pub fn do_connect(
    _ep_id: EndpointId,
    ep: &EndpointDesc,
    client: ThreadId,
    table: &CapTable,
    token: CapToken,
) -> Result<ConnectResult, IpcError> {
    // Capability check: CONNECT required.
    crate::ipc::capability_bridge::check_endpoint_access(table, token, Rights::IPC_CONNECT)
        .map_err(|_| IpcError::PermissionDenied)?;

    if !ep.is_listening() {
        return Err(IpcError::ConnRefused);
    }

    let channel_id = alloc_channel_id();
    let cookie = Cookie::new(channel_id.get() ^ client.0);

    let pending = PendingConnection {
        requester: client,
        channel_id: channel_id.get(),
        cookie: cookie.get(),
        timestamp_ticks: 0, // updated by caller with current tick
    };

    ep.enqueue_connection(pending)?;
    Ok(ConnectResult { channel_id, cookie })
}

/// Accepts the next pending connection.
///
/// # Arguments
/// - `ep`     : endpoint descriptor.
/// - `server` : server thread (must own the endpoint).
///
/// # Return
/// `Ok(AcceptResult)` with PendingConnection and server->client channel.
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
