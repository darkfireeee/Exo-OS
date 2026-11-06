// ipc/message/serializer.rs — Zero-copy IPC serialization/deserialization for Exo-OS
//
// This module implements a fixed-layout binary IPC serializer:
// no capnproto, no serde. The format is defined by repr(C) structs
// and a minimal framing protocol.
//
// Frame format:
//   [MsgFrameHeader (32 bytes)] [payload (N bytes)] [padding (0-7 bytes)]
//
// The serializer operates on stack-allocated buffers or input slices.
// Deserialization is zero-copy (reference into source buffer).
//
// RULE SER-01: any serialized struct must implement SerdeFixed.
// RULE SER-02: no heap allocation.
// RULE SER-03: offsets are in bytes from the start of the buffer.

use core::mem::size_of;
use core::num::NonZeroU64;

use crate::ipc::core::types::{EndpointId, IpcError, MessageFlags, MessageType};
use crate::ipc::message::builder::{IpcMessage, MAX_MSG_INLINE};

// ---------------------------------------------------------------------------
// Serialization traits
// ---------------------------------------------------------------------------

/// Serialization/deserialization for fixed-layout types.
///
/// # Safety
/// Types implementing this trait MUST be `repr(C)` and contain only
/// primitive types (u8, u16, u32, u64, arrays of primitives).
/// No pointers, no references.
pub unsafe trait SerdeFixed: Sized + Copy {
    const SIZE: usize = size_of::<Self>();

    /// Serializes in place at `buf[offset..]`. Returns offset + SIZE.
    fn serialize_into(&self, buf: &mut [u8], offset: usize) -> Result<usize, IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        // SAFETY: Self is Copy + repr(C), no internal pointers.
        unsafe {
            let src = self as *const Self as *const u8;
            buf[offset..end].copy_from_slice(core::slice::from_raw_parts(src, Self::SIZE));
        }
        Ok(end)
    }

    /// Deserializes from `buf[offset..]`. Returns (Self, offset + SIZE).
    fn deserialize_from(buf: &[u8], offset: usize) -> Result<(Self, usize), IpcError> {
        let end = offset + Self::SIZE;
        if end > buf.len() {
            return Err(IpcError::Invalid);
        }
        let mut value = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: Self is Copy + repr(C), exact size.
        unsafe {
            let dst = value.as_mut_ptr() as *mut u8;
            dst.copy_from_nonoverlapping(buf[offset..end].as_ptr(), Self::SIZE);
            Ok((value.assume_init(), end))
        }
    }
}

// ---------------------------------------------------------------------------
// Frame header
// ---------------------------------------------------------------------------

/// Magic number to detect valid IPC frames
pub const FRAME_MAGIC: u32 = 0xEA04_4950; // "EA\x04IPC"

/// Serialization protocol version
pub const FRAME_VERSION: u8 = 1;

/// IPC frame header (32 bytes, repr(C))
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MsgFrameHeader {
    /// Validation magic number
    pub magic: u32,
    /// Protocol version
    pub version: u8,
    /// Message type (MessageType as u8)
    pub msg_type: u8,
    /// Flags (MessageFlags as u16)
    pub flags: u16,
    /// Payload length in bytes
    pub payload_len: u32,
    /// Sequence number
    pub seq: u64,
    /// Source endpoint
    pub src: u32,
    /// Destination endpoint
    pub dst: u32,
    /// Correlation cookie
    pub cookie: u64,
    // Total: 4+1+1+2+4+8+4+4+4 = 32 bytes — no _pad needed
}

// SAFETY: repr(C, packed), all fields are primitives
unsafe impl SerdeFixed for MsgFrameHeader {}

impl MsgFrameHeader {
    /// Creates a header from an IpcMessage
    pub fn from_message(msg: &IpcMessage) -> Self {
        Self {
            magic: FRAME_MAGIC,
            version: FRAME_VERSION,
            msg_type: msg.msg_type as u8,
            flags: msg.flags.bits(),
            payload_len: msg.payload_len as u32,
            seq: msg.seq,
            src: msg.src.0.get() as u32,
            dst: msg.dst.0.get() as u32,
            cookie: msg.cookie,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == FRAME_MAGIC && self.version == FRAME_VERSION
    }

    pub fn frame_size(&self) -> usize {
        size_of::<MsgFrameHeader>() + self.payload_len as usize
    }
}

// ---------------------------------------------------------------------------
// IpcSerializer — serializes an IpcMessage into a buffer
// ---------------------------------------------------------------------------

/// Serializes an `IpcMessage` into a buffer.
///
/// Output format:
/// ```text
/// [MsgFrameHeader: 32 bytes] [payload: payload_len bytes]
/// ```
///
/// Returns total bytes written, or `Err(IpcError::Invalid)` if
/// buffer is too small.
pub fn serialize_message(msg: &IpcMessage, buf: &mut [u8]) -> Result<usize, IpcError> {
    let total_needed = size_of::<MsgFrameHeader>() + msg.payload_len as usize;
    if buf.len() < total_needed {
        return Err(IpcError::Invalid);
    }

    let header = MsgFrameHeader::from_message(msg);
    let mut offset = 0;
    offset = header.serialize_into(buf, offset)?;

    // Copy payload
    let plen = msg.payload_len as usize;
    if plen > 0 {
        msg.copy_payload_to(&mut buf[offset..offset + plen]);
        offset += plen;
    }

    Ok(offset)
}

/// Computes required size to serialize a message without allocating.
pub fn serialized_size(msg: &IpcMessage) -> usize {
    size_of::<MsgFrameHeader>() + msg.payload_len as usize
}

// ---------------------------------------------------------------------------
// IpcDeserializer — deserializes a buffer into IpcMessage
// ---------------------------------------------------------------------------

/// Deserialization result (zero-copy reference in buffer)
pub struct DeserializedMessage<'a> {
    pub header: MsgFrameHeader,
    pub payload: &'a [u8],
}

/// Deserializes an IPC buffer into a zero-copy `DeserializedMessage`.
///
/// Payload lifetime is tied to source buffer.
pub fn deserialize_message(buf: &[u8]) -> Result<DeserializedMessage<'_>, IpcError> {
    if buf.len() < size_of::<MsgFrameHeader>() {
        return Err(IpcError::Invalid);
    }

    let (header, off) = MsgFrameHeader::deserialize_from(buf, 0)?;

    if !header.is_valid() {
        return Err(IpcError::Invalid);
    }

    let plen = header.payload_len as usize;
    if buf.len() < off + plen {
        return Err(IpcError::Invalid);
    }

    Ok(DeserializedMessage {
        header,
        payload: &buf[off..off + plen],
    })
}

/// Deserializes and copies result into an owned `IpcMessage` (payload copy).
pub fn deserialize_into_owned(buf: &[u8]) -> Result<IpcMessage, IpcError> {
    let dm = deserialize_message(buf)?;
    let plen = dm.payload.len();
    if plen > MAX_MSG_INLINE {
        return Err(IpcError::Invalid);
    }

    let mut msg = IpcMessage::empty();
    msg.seq = dm.header.seq;
    msg.src = NonZeroU64::new(dm.header.src as u64)
        .map(EndpointId)
        .unwrap_or(EndpointId::INVALID);
    msg.dst = NonZeroU64::new(dm.header.dst as u64)
        .map(EndpointId)
        .unwrap_or(EndpointId::INVALID);
    msg.msg_type = MessageType::from_u8(dm.header.msg_type);
    msg.flags = MessageFlags::from_bits_truncate(dm.header.flags);
    msg.cookie = dm.header.cookie;
    msg.payload_len = plen as u16;

    if plen > 0 {
        msg.payload_mut().copy_from_slice(dm.payload);
    }

    Ok(msg)
}

// ---------------------------------------------------------------------------
// Multi-messages: framing multiple messages into a single buffer
// ---------------------------------------------------------------------------

/// Serializes a sequence of messages by chaining them into `buf`.
/// Returns total bytes written.
pub fn serialize_batch(messages: &[IpcMessage], buf: &mut [u8]) -> Result<usize, IpcError> {
    let mut offset = 0;
    for msg in messages {
        let written = serialize_message(msg, &mut buf[offset..])?;
        offset += written;
    }
    Ok(offset)
}

/// Zero-copy iterator over a buffer of serialized messages.
pub struct MessageFrameIter<'a> {
    buf: &'a [u8],
    offset: usize,
}

impl<'a> MessageFrameIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, offset: 0 }
    }
}

impl<'a> Iterator for MessageFrameIter<'a> {
    type Item = Result<DeserializedMessage<'a>, IpcError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }
        match deserialize_message(&self.buf[self.offset..]) {
            Err(e) => Some(Err(e)),
            Ok(dm) => {
                let frame_size = dm.header.frame_size();
                self.offset += frame_size;
                Some(Ok(dm))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ad-hoc serialization helpers for primitives
// ---------------------------------------------------------------------------

/// Writes a u32 LE to `buf[offset]`. Returns offset + 4.
pub fn write_u32(buf: &mut [u8], offset: usize, v: u32) -> Result<usize, IpcError> {
    let end = offset + 4;
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    buf[offset..end].copy_from_slice(&v.to_le_bytes());
    Ok(end)
}

/// Reads a u32 LE from `buf[offset]`. Returns (value, offset + 4).
pub fn read_u32(buf: &[u8], offset: usize) -> Result<(u32, usize), IpcError> {
    let end = offset + 4;
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    let v = u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]);
    Ok((v, end))
}

/// Writes a u64 LE to `buf[offset]`.
pub fn write_u64(buf: &mut [u8], offset: usize, v: u64) -> Result<usize, IpcError> {
    let end = offset + 8;
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    buf[offset..end].copy_from_slice(&v.to_le_bytes());
    Ok(end)
}

/// Reads a u64 LE from `buf[offset]`.
pub fn read_u64(buf: &[u8], offset: usize) -> Result<(u64, usize), IpcError> {
    let end = offset + 8;
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    let b: [u8; 8] = [
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ];
    Ok((u64::from_le_bytes(b), end))
}

/// Writes a byte slice into buffer, preceded by its length (u32 LE).
pub fn write_bytes(buf: &mut [u8], offset: usize, data: &[u8]) -> Result<usize, IpcError> {
    let off = write_u32(buf, offset, data.len() as u32)?;
    let end = off + data.len();
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    buf[off..end].copy_from_slice(data);
    Ok(end)
}

/// Reads a byte slice from buffer (read with its u32 LE length).
pub fn read_bytes(buf: &[u8], offset: usize) -> Result<(&[u8], usize), IpcError> {
    let (len, off) = read_u32(buf, offset)?;
    let end = off + len as usize;
    if end > buf.len() {
        return Err(IpcError::Invalid);
    }
    Ok((&buf[off..end], end))
}
