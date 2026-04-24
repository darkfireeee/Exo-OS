// ipc/rpc/protocol.rs — Protocole binaire RPC IPC pour Exo-OS
//
// Ce module définit le protocole binaire des appels RPC IPC :
//   - `RpcHeader` : en-tête de trame RPC (repr(C), 64 bytes)
//   - `RpcCallFrame` / `RpcReplyFrame` : trames request/response
//   - `RpcStatus` : codes de retour RPC
//   - `MethodId` : identifiant 32 bits d'une méthode (namespace + method)
//   - Constantes : RPC_MAGIC, RPC_VERSION, MAX_RPC_PAYLOAD
//
// Format sur le fil :
//   [RpcHeader 64 bytes] [payload N bytes]
//
// RÈGLE RPC-01 : magic = 0xEA04_5250 ("EA\x04RP")
// RÈGLE RPC-02 : version = 1 (incompatible si différent)
// RÈGLE RPC-03 : cookie u64 unique par appel (assigné par le client)

use core::mem::size_of;

use crate::ipc::core::types::{EndpointId, IpcError, ProcessId};

// ---------------------------------------------------------------------------
// Constantes du protocole
// ---------------------------------------------------------------------------

/// Magic number d'une trame RPC valide
pub const RPC_MAGIC: u32 = 0xEA04_5250;

/// Version courante du protocole RPC
pub const RPC_VERSION: u8 = 1;

/// Taille maximale du payload d'un appel ou d'une réponse RPC (octets)
pub const MAX_RPC_PAYLOAD: usize = 4096;

/// Identifiant de méthode réservé : invalide
pub const METHOD_ID_INVALID: u32 = 0;

/// Identifiant de méthode : ping (test de connectivité)
pub const METHOD_ID_PING: u32 = 1;

/// Identifiant de méthode : introspection (liste des méthodes)
pub const METHOD_ID_INTROSPECT: u32 = 2;

// ---------------------------------------------------------------------------
// MethodId
// ---------------------------------------------------------------------------

/// Identifiant de méthode RPC.
///
/// Encodage : bits 31-16 = namespace (service), bits 15-0 = method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct MethodId(pub u32);

impl MethodId {
    pub const fn new(namespace: u16, method: u16) -> Self {
        Self(((namespace as u32) << 16) | (method as u32))
    }

    pub fn namespace(&self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub fn method(&self) -> u16 {
        self.0 as u16
    }

    pub fn is_valid(&self) -> bool {
        self.0 != METHOD_ID_INVALID
    }
}

// ---------------------------------------------------------------------------
// RpcStatus — codes de retour
// ---------------------------------------------------------------------------

/// Codes de statut d'un appel RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RpcStatus {
    /// Succès
    Ok = 0,
    /// Méthode inconnue
    MethodNotFound = 1,
    /// Payload invalide ou trop grand
    InvalidPayload = 2,
    /// Timeout côté serveur
    ServerTimeout = 3,
    /// Serveur occupé
    ServerBusy = 4,
    /// Permission refusée
    Denied = 5,
    /// Erreur interne serveur
    InternalError = 6,
    /// Endpoint serveur invalide
    EndpointInvalid = 7,
    /// Version de protocole incompatible
    VersionMismatch = 8,
}

impl RpcStatus {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::MethodNotFound,
            2 => Self::InvalidPayload,
            3 => Self::ServerTimeout,
            4 => Self::ServerBusy,
            5 => Self::Denied,
            6 => Self::InternalError,
            7 => Self::EndpointInvalid,
            8 => Self::VersionMismatch,
            _ => Self::Ok,
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    pub fn to_ipc_error(self) -> IpcError {
        match self {
            Self::Ok => IpcError::Invalid, // ne devrait pas être appelé sur Ok
            Self::MethodNotFound => IpcError::NotFound,
            Self::InvalidPayload => IpcError::Invalid,
            Self::ServerTimeout => IpcError::Timeout,
            Self::ServerBusy => IpcError::WouldBlock,
            Self::Denied => IpcError::PermissionDenied,
            Self::InternalError => IpcError::Internal,
            Self::EndpointInvalid => IpcError::InvalidEndpoint,
            Self::VersionMismatch => IpcError::Invalid,
        }
    }
}

// ---------------------------------------------------------------------------
// RpcHeader — en-tête de trame RPC (64 bytes, repr(C))
// ---------------------------------------------------------------------------

/// En-tête de trame RPC.
///
/// Présent en début de toute trame call ou reply.
#[derive(Debug, Clone, Copy)]
#[repr(C, align(8))]
pub struct RpcHeader {
    /// Magic number (RPC_MAGIC)
    pub magic: u32,
    /// Version du protocole (RPC_VERSION)
    pub version: u8,
    /// Type : 0 = Call, 1 = Reply
    pub frame_type: u8,
    /// Flags (réservé, 0)
    pub flags: u16,
    /// Identifiant de méthode (MethodId)
    pub method_id: u32,
    /// Cookie unique (client assigne, server répercute)
    pub cookie: u64,
    /// Endpoint appelant
    pub caller_ep: u32,
    /// Endpoint serveur
    pub server_ep: u32,
    /// PID du processus appelant
    pub caller_pid: u32,
    /// Statut de la réponse (valide seulement pour frame_type=Reply)
    pub status: u32,
    /// Taille du payload en octets
    pub payload_len: u32,
    /// Timeout souhaité par le client (nanosecondes, 0=défaut)
    pub timeout_ns: u64,
    _pad: [u8; 8],
    // Total : 4+1+1+2+4+8+4+4+4+4+4+8+8 = 56... + 8 pad = 64 bytes ✓
}

impl RpcHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new_call(
        method_id: MethodId,
        cookie: u64,
        caller_ep: EndpointId,
        server_ep: EndpointId,
        caller_pid: ProcessId,
        payload_len: u32,
        timeout_ns: u64,
    ) -> Self {
        Self {
            magic: RPC_MAGIC,
            version: RPC_VERSION,
            frame_type: 0,
            flags: 0,
            method_id: method_id.0,
            cookie,
            caller_ep: caller_ep.0.get() as u32,
            server_ep: server_ep.0.get() as u32,
            caller_pid: caller_pid.0,
            status: 0,
            payload_len,
            timeout_ns,
            _pad: [0u8; 8],
        }
    }

    pub fn new_reply(call: &RpcHeader, status: RpcStatus, payload_len: u32) -> Self {
        Self {
            magic: RPC_MAGIC,
            version: RPC_VERSION,
            frame_type: 1,
            flags: 0,
            method_id: call.method_id,
            cookie: call.cookie,
            caller_ep: call.caller_ep,
            server_ep: call.server_ep,
            caller_pid: call.caller_pid,
            status: status as u32,
            payload_len,
            timeout_ns: 0,
            _pad: [0u8; 8],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == RPC_MAGIC && self.version == RPC_VERSION
    }

    pub fn is_call(&self) -> bool {
        self.frame_type == 0
    }

    pub fn is_reply(&self) -> bool {
        self.frame_type == 1
    }

    pub fn reply_status(&self) -> RpcStatus {
        RpcStatus::from_u32(self.status)
    }

    /// Sérialise l'en-tête dans `buf[offset..]`.
    /// Retourne offset + RpcHeader::SIZE.
    pub fn serialize_into(&self, buf: &mut [u8], offset: usize) -> Result<usize, IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        // SAFETY: repr(C), pas de pointeurs
        unsafe {
            let src = self as *const Self as *const u8;
            buf[offset..end].copy_from_slice(core::slice::from_raw_parts(src, Self::SIZE));
        }
        Ok(end)
    }

    /// Désérialise un RpcHeader depuis `buf[offset..]`.
    pub fn deserialize_from(buf: &[u8], offset: usize) -> Result<(Self, usize), IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut h = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: repr(C), taille exacte
        unsafe {
            let dst = h.as_mut_ptr() as *mut u8;
            dst.copy_from_nonoverlapping(buf[offset..end].as_ptr(), Self::SIZE);
            Ok((h.assume_init(), end))
        }
    }
}

// ---------------------------------------------------------------------------
// RpcCallFrame / RpcReplyFrame — trames complètes avec payload inline
// ---------------------------------------------------------------------------

/// Trame d'appel RPC avec payload inline (stack-allocated).
#[repr(C, align(64))]
pub struct RpcCallFrame {
    pub header: RpcHeader,
    pub payload: [u8; MAX_RPC_PAYLOAD],
}

impl RpcCallFrame {
    pub const fn empty() -> Self {
        Self {
            header: RpcHeader {
                magic: 0,
                version: 0,
                frame_type: 0,
                flags: 0,
                method_id: 0,
                cookie: 0,
                caller_ep: 0,
                server_ep: 0,
                caller_pid: 0,
                status: 0,
                payload_len: 0,
                timeout_ns: 0,
                _pad: [0u8; 8],
            },
            payload: [0u8; MAX_RPC_PAYLOAD],
        }
    }

    pub fn payload_data(&self) -> &[u8] {
        &self.payload[..self.header.payload_len as usize]
    }

    /// Sérialise la trame complète dans `buf`. Retourne le nombre d'octets écrits.
    pub fn serialize(&self, buf: &mut [u8]) -> Result<usize, IpcError> {
        let plen = self.header.payload_len as usize;
        let total = RpcHeader::SIZE + plen;
        if buf.len() < total {
            return Err(IpcError::Invalid);
        }
        let off = self.header.serialize_into(buf, 0)?;
        if plen > 0 {
            buf[off..off + plen].copy_from_slice(&self.payload[..plen]);
        }
        Ok(off + plen)
    }

    /// Désérialise depuis un buffer.
    pub fn deserialize(buf: &[u8]) -> Result<Self, IpcError> {
        let (header, off) = RpcHeader::deserialize_from(buf, 0)?;
        if !header.is_valid() || !header.is_call() {
            return Err(IpcError::Invalid);
        }
        let plen = header.payload_len as usize;
        if plen > MAX_RPC_PAYLOAD {
            return Err(IpcError::Invalid);
        }
        if off + plen > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut frame = Self::empty();
        frame.header = header;
        if plen > 0 {
            frame.payload[..plen].copy_from_slice(&buf[off..off + plen]);
        }
        Ok(frame)
    }
}

/// Trame de réponse RPC avec payload inline.
#[repr(C, align(64))]
pub struct RpcReplyFrame {
    pub header: RpcHeader,
    pub payload: [u8; MAX_RPC_PAYLOAD],
}

impl RpcReplyFrame {
    pub const fn empty() -> Self {
        Self {
            header: RpcHeader {
                magic: 0,
                version: 0,
                frame_type: 0,
                flags: 0,
                method_id: 0,
                cookie: 0,
                caller_ep: 0,
                server_ep: 0,
                caller_pid: 0,
                status: 0,
                payload_len: 0,
                timeout_ns: 0,
                _pad: [0u8; 8],
            },
            payload: [0u8; MAX_RPC_PAYLOAD],
        }
    }

    pub fn payload_data(&self) -> &[u8] {
        &self.payload[..self.header.payload_len as usize]
    }

    pub fn serialize(&self, buf: &mut [u8]) -> Result<usize, IpcError> {
        let plen = self.header.payload_len as usize;
        let total = RpcHeader::SIZE + plen;
        if buf.len() < total {
            return Err(IpcError::Invalid);
        }
        let off = self.header.serialize_into(buf, 0)?;
        if plen > 0 {
            buf[off..off + plen].copy_from_slice(&self.payload[..plen]);
        }
        Ok(off + plen)
    }

    pub fn deserialize(buf: &[u8]) -> Result<Self, IpcError> {
        let (header, off) = RpcHeader::deserialize_from(buf, 0)?;
        if !header.is_valid() || !header.is_reply() {
            return Err(IpcError::Invalid);
        }
        let plen = header.payload_len as usize;
        if plen > MAX_RPC_PAYLOAD {
            return Err(IpcError::Invalid);
        }
        if off + plen > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut frame = Self::empty();
        frame.header = header;
        if plen > 0 {
            frame.payload[..plen].copy_from_slice(&buf[off..off + plen]);
        }
        Ok(frame)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Calcule la taille totale d'une trame RPC (header + payload).
pub fn rpc_frame_size(payload_len: usize) -> usize {
    RpcHeader::SIZE + payload_len
}

/// Vérifie qu'un buffer contient une trame RPC valide (magic + version).
pub fn rpc_frame_valid(buf: &[u8]) -> bool {
    if buf.len() < RpcHeader::SIZE {
        return false;
    }
    match RpcHeader::deserialize_from(buf, 0) {
        Ok((h, _)) => h.is_valid(),
        Err(_) => false,
    }
}
