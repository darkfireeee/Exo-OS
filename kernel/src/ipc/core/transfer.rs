// kernel/src/ipc/core/transfer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MESSAGE TRANSFER ENGINE — IPC message transfer engine
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module implements the core of message transfer between
// address spaces. It supports two modes:
//
//   1. COPY     — physical copy of data (for small messages <= MAX_MSG_SIZE)
//   2. ZEROCOPY — physical page sharing (NO_COW, zero-copy, for large volumes)
//
// RULES:
//   • Zero heap allocation in this file (critical NO-ALLOC Zone)
//   • All unsafe documented with // SAFETY:
//   • No imports of fs/, process/ (layer 2a)
//   • Systematic bounds checking before any copy
// ═══════════════════════════════════════════════════════════════════════════════

use super::constants::{MAX_MSG_SIZE, MSG_HEADER_SIZE, RING_SLOT_SIZE};
use super::types::{alloc_message_id, IpcError, MessageId, MsgFlags};
use core::ptr;
use core::sync::atomic::{fence, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// MessageHeader — inline header in each ring slot
// ─────────────────────────────────────────────────────────────────────────────

/// Message header inlined in the ring slot.
/// Exactly 16 bytes — statically verified.
#[derive(Copy, Clone, Debug)]
#[repr(C, align(8))]
pub struct MessageHeader {
    /// Unique message identifier.
    pub msg_id: u64,
    /// Flags (RT, ZEROCOPY, BROADCAST…).
    pub flags: u32,
    /// Payload length (0..=MAX_MSG_SIZE).
    pub len: u16,
    /// Padding to align on 16 bytes.
    pub _pad: u16,
}

const _: () = assert!(
    core::mem::size_of::<MessageHeader>() == MSG_HEADER_SIZE,
    "MessageHeader doit faire exactement MSG_HEADER_SIZE bytes"
);

impl MessageHeader {
    /// Creates a header for an inline message.
    #[inline(always)]
    pub fn new_inline(id: MessageId, flags: MsgFlags, len: usize) -> Self {
        debug_assert!(len <= MAX_MSG_SIZE, "payload dépasse MAX_MSG_SIZE");
        Self {
            msg_id: id.get(),
            flags: flags.0,
            len: len as u16,
            _pad: 0,
        }
    }

    /// Creates a header for a zero-copy message.
    #[inline(always)]
    pub fn new_zerocopy(id: MessageId, flags: MsgFlags) -> Self {
        let f = MsgFlags(flags.0 | MsgFlags::ZEROCOPY.0);
        Self {
            msg_id: id.get(),
            flags: f.0,
            len: 0, // payload is an external physical reference
            _pad: 0,
        }
    }

    /// Returns true if message is zero-copy.
    #[inline(always)]
    pub fn is_zerocopy(&self) -> bool {
        (self.flags & MsgFlags::ZEROCOPY.0) != 0
    }

    /// Returns true if message is real-time.
    #[inline(always)]
    pub fn is_rt(&self) -> bool {
        (self.flags & MsgFlags::RT.0) != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RingSlot — complete slot in ring (header + inlined payload)
// ─────────────────────────────────────────────────────────────────────────────

/// A complete slot in the communication ring.
/// Fixed size RING_SLOT_SIZE = MSG_HEADER_SIZE + MAX_MSG_SIZE.
#[repr(C, align(64))]
pub struct RingSlot {
    pub header: MessageHeader,
    pub payload: [u8; MAX_MSG_SIZE],
}

const _: () = assert!(
    core::mem::size_of::<RingSlot>() == RING_SLOT_SIZE,
    "RingSlot doit faire RING_SLOT_SIZE bytes"
);

impl RingSlot {
    /// Creates an empty slot (zero-initialized).
    pub const fn zeroed() -> Self {
        Self {
            header: MessageHeader {
                msg_id: 0,
                flags: 0,
                len: 0,
                _pad: 0,
            },
            payload: [0u8; MAX_MSG_SIZE],
        }
    }

    /// Writes an inline message into this slot.
    ///
    /// # Safety
    /// `src` must point to at least `len` valid bytes.
    /// `len` must be <= MAX_MSG_SIZE.
    #[inline]
    pub unsafe fn write_inline(
        &mut self,
        id: MessageId,
        flags: MsgFlags,
        src: *const u8,
        len: usize,
    ) -> Result<(), IpcError> {
        if len > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        self.header = MessageHeader::new_inline(id, flags, len);
        if len > 0 {
            // SAFETY: src is valid (precondition), payload[..len] is within the slot.
            ptr::copy_nonoverlapping(src, self.payload.as_mut_ptr(), len);
        }
        // Release barrier — data is visible before the ring
        // increments tail and makes the slot readable.
        fence(Ordering::Release);
        Ok(())
    }

    /// Reads the payload from this slot into a destination buffer.
    ///
    /// # Safety
    /// `dst` must point to at least `buf_len` valid bytes.
    /// Called only after an Acquire barrier on the ring.
    #[inline]
    pub unsafe fn read_inline(&self, dst: *mut u8, buf_len: usize) -> Result<usize, IpcError> {
        let len = self.header.len as usize;
        if len > buf_len {
            return Err(IpcError::MessageTooLarge);
        }
        if len > MAX_MSG_SIZE {
            return Err(IpcError::InternalError); // corruption
        }
        // SAFETY: dst is valid (precondition), payload[..len] initialized by write_inline.
        ptr::copy_nonoverlapping(self.payload.as_ptr(), dst, len);
        Ok(len)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZeroCopyRef — physical reference for zero-copy transfer
// ─────────────────────────────────────────────────────────────────────────────

/// Physical reference to a shared buffer (zero-copy IPC).
/// The underlying page is mapped in both address spaces
/// with the NO_COW flag (managed by shared_memory/pool.rs).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct ZeroCopyRef {
    /// Physical address of the buffer (PAGE_SIZE-aligned).
    pub phys_addr: u64,
    /// Buffer length in bytes.
    pub length: u32,
    /// Index in SHM pool (for release).
    pub pool_idx: u32,
}

impl ZeroCopyRef {
    /// Creates a zero-copy reference.
    #[inline(always)]
    pub fn new(phys_addr: u64, length: u32, pool_idx: u32) -> Self {
        Self {
            phys_addr,
            length,
            pool_idx,
        }
    }

    /// Returns true if the reference is valid (non-null).
    #[inline(always)]
    pub fn is_valid(&self) -> bool {
        self.phys_addr != 0 && self.length > 0
    }

    /// Null reference (physical address 0, without buffer).
    #[inline(always)]
    pub const fn null() -> Self {
        Self {
            phys_addr: 0,
            length: 0,
            pool_idx: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TransferEngine — optimized copy engine
// ─────────────────────────────────────────────────────────────────────────────

/// Transfer engine statistics.
#[derive(Default, Debug)]
pub struct TransferStats {
    /// Number of inline transfers performed.
    pub inline_count: u64,
    /// Number of zero-copy transfers performed.
    pub zerocopy_count: u64,
    /// Total bytes copied (inline only).
    pub bytes_copied: u64,
    /// Number of errors (bounds violations, etc.).
    pub errors: u64,
}

/// Stateless transfer engine.
/// All methods are pure functions — state resides in slots.
pub struct TransferEngine;

impl TransferEngine {
    /// Copies `src[0..len]` into the destination slot.
    ///
    /// # Safety
    /// - `src` must point to at least `len` valid bytes.
    /// - `slot` must be exclusively held by the caller.
    #[inline]
    pub unsafe fn copy_to_slot(
        slot: &mut RingSlot,
        src: *const u8,
        len: usize,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        if len > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        let id = alloc_message_id();
        // SAFETY: valid src and len <= MAX_MSG_SIZE verified above.
        slot.write_inline(id, flags, src, len)?;
        Ok(id)
    }

    /// Copies the slot's payload to `dst[0..buf_len]`.
    ///
    /// # Safety
    /// - `dst` must point to at least `buf_len` valid bytes.
    /// - An Acquire barrier must have been issued before this call.
    #[inline]
    pub unsafe fn copy_from_slot(
        slot: &RingSlot,
        dst: *mut u8,
        buf_len: usize,
    ) -> Result<usize, IpcError> {
        // SAFETY: valid dst and buf_len verified by caller.
        slot.read_inline(dst, buf_len)
    }

    /// Transfer slice -> slot (safe typed version for Copy types).
    ///
    /// # Constraint
    /// `T` must be `Copy + 'static` (no internal pointers).
    #[inline]
    pub fn transfer_value<T: Copy>(
        slot: &mut RingSlot,
        value: &T,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        let len = core::mem::size_of::<T>();
        // SAFETY: value is a valid reference to a T of size `len`.
        unsafe { Self::copy_to_slot(slot, value as *const T as *const u8, len, flags) }
    }

    /// Receives a slot into a type `T` (safe typed version).
    ///
    /// # Safety
    /// The slot must have been written by `transfer_value::<T>`.
    /// Alignment of T must be <= that of `RingSlot::payload`.
    #[inline]
    pub fn receive_value<T: Copy>(slot: &RingSlot) -> Result<T, IpcError> {
        let len = core::mem::size_of::<T>();
        if slot.header.len as usize != len {
            return Err(IpcError::InvalidParam);
        }
        // SAFETY: payload contains exactly `len` bytes of type T.
        // Alignment of payload (=[u8]) is 1 — using read_unaligned.
        let t: T = unsafe { ptr::read_unaligned(slot.payload.as_ptr() as *const T) };
        Ok(t)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Compile-time invariant validation
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    core::mem::size_of::<ZeroCopyRef>() == 16,
    "ZeroCopyRef doit faire 16 bytes"
);
