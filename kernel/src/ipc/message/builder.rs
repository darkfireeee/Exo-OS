// ipc/message/builder.rs — Fluent IPC message builder for Exo-OS
//
// This module implements a builder pattern to assemble IPC messages
// safely and ergonomically. An `IpcMessageBuilder` accumulates metadata
// (flags, source/destination endpoint, type, priority) and inline payload
// in a stack-allocated buffer, then produces an `IpcMessage` ready to be
// transmitted.
//
// RULE MSG-01: no heap allocation. Payload is inline (max MAX_MSG_INLINE).
// RULE MSG-02: the builder is stack-allocated and never boxed.
// RULE MSG-03: build() produces a complete IpcMessage or returns an error.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError, MessageFlags, MessageType, ProcessId};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum inline payload size in an IpcMessage (bytes).
///
/// Must remain bounded by the SPSC slot (`ipc::core::MAX_MSG_SIZE`) to prevent
/// any silent truncation when a built message subsequently passes through the
/// fast path ring.
pub const MAX_MSG_INLINE: usize = crate::ipc::core::constants::MAX_MSG_SIZE;

const _: () = assert!(
    MAX_MSG_INLINE <= crate::ipc::core::constants::MAX_MSG_SIZE,
    "IpcMessage inline depasse la capacite du ring SPSC"
);

/// Maximum capacity of descriptors attached to a message
pub const MAX_MSG_DESCRIPTORS: usize = 8;

// ---------------------------------------------------------------------------
// IpcDescriptor — transferable handle
// ---------------------------------------------------------------------------

/// Descriptor transmitted with a message (file descriptor, capability, SHM handle, etc.)
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct IpcDescriptor {
    pub kind: u32,
    pub handle: u32,
    pub flags: u32,
    pub _pad: u32,
}

impl IpcDescriptor {
    pub const fn new(kind: u32, handle: u32, flags: u32) -> Self {
        Self {
            kind,
            handle,
            flags,
            _pad: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// IpcMessage — complete IPC message
// ---------------------------------------------------------------------------

/// Complete IPC message: header + inline payload + descriptors.
///
/// Packed struct to facilitate passing over ring buffers.
/// Total: MAX_MSG_INLINE + 192 bytes max.
#[repr(C, align(64))]
pub struct IpcMessage {
    // --- Header (64 bytes) ---
    /// Sequence number (assigned by builder or endpoint)
    pub seq: u64,
    /// Source endpoint
    pub src: EndpointId,
    /// Destination endpoint
    pub dst: EndpointId,
    /// Message type
    pub msg_type: MessageType,
    /// Message flags
    pub flags: MessageFlags,
    /// Priority (0=low ... 255=real-time)
    pub priority: u8,
    /// Number of attached descriptors
    pub desc_count: u8,
    /// Actual payload size (bytes)
    pub payload_len: u16,
    /// Sending process
    pub sender_pid: ProcessId,
    /// Correlation identification cookie (for RPC reply-matching)
    pub cookie: u64,
    _pad: [u8; 14],

    // --- Descriptors (8 × 16 bytes = 128 bytes) ---
    pub descriptors: [IpcDescriptor; MAX_MSG_DESCRIPTORS],

    // --- Inline payload (bounded by SPSC ring) ---
    payload: [u8; MAX_MSG_INLINE],
}

impl IpcMessage {
    /// New empty message (all fields zeroed / sentinels).
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            src: EndpointId::INVALID,
            dst: EndpointId::INVALID,
            msg_type: MessageType::Data,
            flags: MessageFlags::NONE,
            priority: 0,
            desc_count: 0,
            payload_len: 0,
            sender_pid: ProcessId(0),
            cookie: 0,
            _pad: [0u8; 14],
            descriptors: [IpcDescriptor {
                kind: 0,
                handle: 0,
                flags: 0,
                _pad: 0,
            }; MAX_MSG_DESCRIPTORS],
            payload: [0u8; MAX_MSG_INLINE],
        }
    }

    /// Read access to payload
    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }

    /// Write access to payload (dangerous: do not exceed payload_len)
    pub fn payload_mut(&mut self) -> &mut [u8] {
        let len = self.payload_len as usize;
        &mut self.payload[..len]
    }

    /// Copies payload to `out`. Returns number of copied bytes.
    pub fn copy_payload_to(&self, out: &mut [u8]) -> usize {
        let len = (self.payload_len as usize).min(out.len());
        out[..len].copy_from_slice(&self.payload[..len]);
        len
    }
}

// ---------------------------------------------------------------------------
// Global sequence counter
// ---------------------------------------------------------------------------

static MSG_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_seq() -> u64 {
    MSG_SEQ.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// IpcMessageBuilder — fluent API
// ---------------------------------------------------------------------------

/// Stack-allocated builder for IpcMessage.
///
/// # Example
/// ```rust
/// let msg = IpcMessageBuilder::new()
///     .src(my_ep)
///     .dst(target_ep)
///     .msg_type(MessageType::Data)
///     .priority(128)
///     .payload(data)?
///     .build()?;
/// ```
pub struct IpcMessageBuilder {
    src: EndpointId,
    dst: EndpointId,
    msg_type: MessageType,
    flags: MessageFlags,
    priority: u8,
    sender_pid: ProcessId,
    cookie: u64,
    descriptors: [IpcDescriptor; MAX_MSG_DESCRIPTORS],
    desc_count: usize,
    payload_buf: [u8; MAX_MSG_INLINE],
    payload_len: usize,
    error: Option<IpcError>,
}

impl IpcMessageBuilder {
    /// Creates a new builder with healthy defaults.
    pub fn new() -> Self {
        Self {
            src: EndpointId::INVALID,
            dst: EndpointId::INVALID,
            msg_type: MessageType::Data,
            flags: MessageFlags::NONE,
            priority: 64,
            sender_pid: ProcessId(0),
            cookie: 0,
            descriptors: [IpcDescriptor {
                kind: 0,
                handle: 0,
                flags: 0,
                _pad: 0,
            }; MAX_MSG_DESCRIPTORS],
            desc_count: 0,
            payload_buf: [0u8; MAX_MSG_INLINE],
            payload_len: 0,
            error: None,
        }
    }

    // -----------------------------------------------------------------------
    // Configuration methods (chainable)
    // -----------------------------------------------------------------------

    /// Source endpoint
    pub fn src(mut self, ep: EndpointId) -> Self {
        self.src = ep;
        self
    }

    /// Destination endpoint
    pub fn dst(mut self, ep: EndpointId) -> Self {
        self.dst = ep;
        self
    }

    /// Message type
    pub fn msg_type(mut self, t: MessageType) -> Self {
        self.msg_type = t;
        self
    }

    /// Message flags
    pub fn flags(mut self, f: MessageFlags) -> Self {
        self.flags = f;
        self
    }

    /// Priority (0–255)
    pub fn priority(mut self, p: u8) -> Self {
        self.priority = p;
        self
    }

    /// Sending process
    pub fn sender(mut self, pid: ProcessId) -> Self {
        self.sender_pid = pid;
        self
    }

    /// Correlation cookie (for RPC reply-matching)
    pub fn cookie(mut self, c: u64) -> Self {
        self.cookie = c;
        self
    }

    /// Payload: copies `data` into inline buffer.  
    /// Silently truncate if too large? No — returns an error
    /// in `build()` via `self.error`.
    pub fn payload(mut self, data: &[u8]) -> Self {
        if data.len() > MAX_MSG_INLINE {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        let len = data.len();
        self.payload_buf[..len].copy_from_slice(data);
        self.payload_len = len;
        self
    }

    /// Payload from a slice of u32 (little-endian)
    pub fn payload_u32(mut self, values: &[u32]) -> Self {
        let byte_len = values.len() * 4;
        if byte_len > MAX_MSG_INLINE {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        for (i, v) in values.iter().enumerate() {
            let off = i * 4;
            self.payload_buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.payload_len = byte_len;
        self
    }

    /// Adds a descriptor to message.
    pub fn add_descriptor(mut self, desc: IpcDescriptor) -> Self {
        if self.desc_count >= MAX_MSG_DESCRIPTORS {
            self.error = Some(IpcError::Invalid);
            return self;
        }
        self.descriptors[self.desc_count] = desc;
        self.desc_count += 1;
        self
    }

    // -----------------------------------------------------------------------
    // build()
    // -----------------------------------------------------------------------

    /// Builds final IPC message.
    ///
    /// Returns `Err` if an error was recorded during building, or
    /// if destination is invalid (EndpointId(0)).
    pub fn build(self) -> Result<IpcMessage, IpcError> {
        if let Some(e) = self.error {
            IPC_STATS.record(StatEvent::InvalidParam);
            return Err(e);
        }

        if self.dst == EndpointId::INVALID {
            IPC_STATS.record(StatEvent::InvalidParam);
            return Err(IpcError::InvalidEndpoint);
        }

        let mut msg = IpcMessage::empty();
        msg.seq = next_seq();
        msg.src = self.src;
        msg.dst = self.dst;
        msg.msg_type = self.msg_type;
        msg.flags = self.flags;
        msg.priority = self.priority;
        msg.sender_pid = self.sender_pid;
        msg.cookie = self.cookie;
        msg.desc_count = self.desc_count as u8;
        msg.payload_len = self.payload_len as u16;

        // Copy inline payload
        if self.payload_len > 0 {
            msg.payload[..self.payload_len].copy_from_slice(&self.payload_buf[..self.payload_len]);
        }

        // Copy descriptors
        for i in 0..self.desc_count {
            msg.descriptors[i] = self.descriptors[i];
        }

        IPC_STATS.record(StatEvent::MessageSent);
        Ok(msg)
    }
}

impl Default for IpcMessageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers for common messages
// ---------------------------------------------------------------------------

/// Creates a simple data message.
pub fn msg_data(src: EndpointId, dst: EndpointId, data: &[u8]) -> Result<IpcMessage, IpcError> {
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Data)
        .payload(data)
        .build()
}

/// Creates a control message.
pub fn msg_control(src: EndpointId, dst: EndpointId, cmd: u32) -> Result<IpcMessage, IpcError> {
    let buf = cmd.to_le_bytes();
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Control)
        .payload(&buf)
        .build()
}

/// Creates a signal message (lightweight notification, no payload).
pub fn msg_signal(src: EndpointId, dst: EndpointId, signum: u32) -> Result<IpcMessage, IpcError> {
    let buf = signum.to_le_bytes();
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::Signal)
        .priority(200)
        .payload(&buf)
        .build()
}

/// Creates an RPC reply message (for reply-matching via cookie).
pub fn msg_rpc_reply(
    src: EndpointId,
    dst: EndpointId,
    cookie: u64,
    result: &[u8],
) -> Result<IpcMessage, IpcError> {
    IpcMessageBuilder::new()
        .src(src)
        .dst(dst)
        .msg_type(MessageType::RpcReply)
        .cookie(cookie)
        .payload(result)
        .build()
}
