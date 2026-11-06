// ipc/rpc/protocol.rs — Binary IPC RPC protocol for Exo-OS
//
// This module defines the binary protocol for IPC RPC calls:
//   - `RpcHeader`: RPC frame header (repr(C), 64 bytes)
//   - `RpcCallFrame` / `RpcReplyFrame`: request/response frames
//   - `RpcStatus`: RPC return codes
//   - `MethodId`: 32-bit method identifier (namespace + method)
//   - Constants: RPC_MAGIC, RPC_VERSION, MAX_RPC_PAYLOAD
//
// Wire format:
//   [RpcHeader 64 bytes] [payload N bytes]
//
// RULE RPC-01: magic = 0xEA04_5250 ("EA\x04RP")
// RULE RPC-02: version = 1 (incompatible if different)
// RULE RPC-03: unique u64 cookie per call (assigned by client)

use core::mem::size_of;

use crate::ipc::core::types::{EndpointId, IpcError, ProcessId};

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Magic number of a valid RPC frame
pub const RPC_MAGIC: u32 = 0xEA04_5250;

/// Current RPC protocol version
pub const RPC_VERSION: u8 = 1;

/// Maximum payload size of an RPC call or reply (bytes)
pub const MAX_RPC_PAYLOAD: usize = 4096;

/// Reserved method identifier: invalid
pub const METHOD_ID_INVALID: u32 = 0;

/// Method identifier: ping (connectivity test)
pub const METHOD_ID_PING: u32 = 1;

/// Method identifier: introspection (method listing)
pub const METHOD_ID_INTROSPECT: u32 = 2;

// ---------------------------------------------------------------------------
// MethodId
// ---------------------------------------------------------------------------

/// RPC method identifier.
///
/// Encoding: bits 31-16 = namespace (service), bits 15-0 = method.
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
// RpcStatus — return codes
// ---------------------------------------------------------------------------

/// Status codes of an RPC call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RpcStatus {
    /// Success
    Ok = 0,
    /// Unknown method
    MethodNotFound = 1,
    /// Invalid or too large payload
    InvalidPayload = 2,
    /// Server-side timeout
    ServerTimeout = 3,
    /// Server busy
    ServerBusy = 4,
    /// Permission denied
    Denied = 5,
    /// Server internal error
    InternalError = 6,
    /// Invalid server endpoint
    EndpointInvalid = 7,
    /// Incompatible protocol version
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

    /// Indicates whether error is transient (retry useful) or fatal (immediate failure).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ServerTimeout | Self::ServerBusy | Self::InternalError
        )
    }

    pub fn to_ipc_error(self) -> IpcError {
        match self {
            Self::Ok => IpcError::Invalid, // should not be called on Ok
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
// RpcHeader — RPC frame header (64 bytes, repr(C))
// ---------------------------------------------------------------------------

/// RPC frame header.
///
/// Present at the start of any call or reply frame.
#[derive(Debug, Clone, Copy)]
#[repr(C, align(8))]
pub struct RpcHeader {
    /// Magic number (RPC_MAGIC)
    pub magic: u32,
    /// Protocol version (RPC_VERSION)
    pub version: u8,
    /// Type: 0 = Call, 1 = Reply
    pub frame_type: u8,
    /// Flags (reserved, 0)
    pub flags: u16,
    /// Method identifier (MethodId)
    pub method_id: u32,
    /// Unique cookie (client assigns, server echoes)
    pub cookie: u64,
    /// Calling endpoint
    pub caller_ep: u32,
    /// Server endpoint
    pub server_ep: u32,
    /// Calling process PID
    pub caller_pid: u32,
    /// Reply status (valid only for frame_type=Reply)
    pub status: u32,
    /// Payload size in bytes
    pub payload_len: u32,
    /// Client-desired timeout (nanoseconds, 0=default)
    pub timeout_ns: u64,
    _pad: [u8; 8],
    // Total: 4+1+1+2+4+8+4+4+4+4+4+8+8 = 56... + 8 pad = 64 bytes ✓
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

    /// Serializes header into `buf[offset..]`.
    /// Returns offset + RpcHeader::SIZE.
    pub fn serialize_into(&self, buf: &mut [u8], offset: usize) -> Result<usize, IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        // SAFETY: repr(C), no pointers
        unsafe {
            let src = self as *const Self as *const u8;
            buf[offset..end].copy_from_slice(core::slice::from_raw_parts(src, Self::SIZE));
        }
        Ok(end)
    }

    /// Deserializes an RpcHeader from `buf[offset..]`.
    pub fn deserialize_from(buf: &[u8], offset: usize) -> Result<(Self, usize), IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut h = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: repr(C), exact size
        unsafe {
            let dst = h.as_mut_ptr() as *mut u8;
            dst.copy_from_nonoverlapping(buf[offset..end].as_ptr(), Self::SIZE);
            Ok((h.assume_init(), end))
        }
    }
}

// ---------------------------------------------------------------------------
// RpcCallFrame / RpcReplyFrame — complete frames with inline payload
// ---------------------------------------------------------------------------

/// RPC call frame with inline payload (stack-allocated).
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

    /// Serializes complete frame into `buf`. Returns number of written bytes.
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

    /// Deserializes from a buffer.
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

/// RPC reply frame with inline payload.
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

/// Computes total size of an RPC frame (header + payload).
pub fn rpc_frame_size(payload_len: usize) -> usize {
    RpcHeader::SIZE + payload_len
}

/// Verifies that a buffer contains a valid RPC frame (magic + version).
pub fn rpc_frame_valid(buf: &[u8]) -> bool {
    if buf.len() < RpcHeader::SIZE {
        return false;
    }
    match RpcHeader::deserialize_from(buf, 0) {
        Ok((h, _)) => h.is_valid(),
        Err(_) => false,
    }
}
