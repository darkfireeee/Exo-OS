// kernel/src/ipc/core/mod.rs
//
// IPC core module — re-exports fundamental types, constants, and transfer engine.

core::arch::global_asm!(include_str!("fastcall_asm.s"), options(att_syntax));

pub mod constants;
pub mod sequence;
pub mod transfer;
pub mod types;

pub use constants::*;
pub use sequence::{SeqCheck, SeqPair, SeqReceiver, SeqSender};
pub use transfer::{MessageHeader, RingSlot, TransferEngine, TransferStats, ZeroCopyRef};
pub use types::{
    alloc_channel_id, alloc_endpoint_id, alloc_message_id, array_index_nospec, ChannelId, Cookie,
    EndpointId, IpcCapError, IpcError, MessageFlags, MessageId, MessageType, MsgFlags, ProcessId,
};

// ─────────────────────────────────────────────────────────────────────────────
// FFI — fast path ASM functions (fastcall_asm.s)
// ─────────────────────────────────────────────────────────────────────────────

/// Fast IPC message — 80 bytes (header 16 + data 64).
/// Designed to fit in one and a half cache lines.
#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct IpcFastMsg {
    /// Message identifier (0 = allocate).
    pub msg_id: u64,
    /// MsgFlags flags.
    pub flags: u32,
    /// Data length (0..=64).
    pub len: u16,
    pub _pad: u16,
    /// Inline data (max 64 bytes for fast path).
    pub data: [u8; 64],
}

impl IpcFastMsg {
    pub const fn zeroed() -> Self {
        Self {
            msg_id: 0,
            flags: 0,
            len: 0,
            _pad: 0,
            data: [0u8; 64],
        }
    }
}

// These functions are implemented in fastcall_asm.s and called from
// ring/spsc.rs as low-level primitives.
extern "C" {
    pub fn ipc_fast_send(msg: *const IpcFastMsg, channel_id: u64) -> u64;
    pub fn ipc_fast_recv(dst: *mut IpcFastMsg, channel_id: u64) -> u64;
    pub fn ipc_fast_call(
        req: *const IpcFastMsg,
        rep: *mut IpcFastMsg,
        channel_id: u64,
        timeout_ns: u64,
    ) -> u64;
    pub fn ipc_ring_fence();
}

// Rust functions called from ASM (no_mangle for linkage).
// Implemented in ring/spsc.rs.
/// # Safety
/// `msg` must point to a valid `IpcFastMsg` in memory.
#[no_mangle]
pub unsafe extern "C" fn ipc_ring_fast_write(msg: *const IpcFastMsg, channel_id: u64) -> u64 {
    use crate::ipc::ring::spsc::spsc_fast_write;
    // SAFETY: msg is a valid pointer (verified by ipc_fast_send asm).
    spsc_fast_write(msg, channel_id)
}

/// # Safety
/// `dst` must point to a valid writable `IpcFastMsg`.
#[no_mangle]
pub unsafe extern "C" fn ipc_ring_fast_read(dst: *mut IpcFastMsg, channel_id: u64) -> u64 {
    use crate::ipc::ring::spsc::spsc_fast_read;
    // SAFETY: dst is a valid pointer (verified by ipc_fast_recv asm).
    spsc_fast_read(dst, channel_id)
}

/// # Safety
/// `dst` must point to a valid writable `IpcFastMsg`.
#[no_mangle]
pub unsafe extern "C" fn ipc_ring_fast_wait_reply(
    dst: *mut IpcFastMsg,
    channel_id: u64,
    timeout_ns: u64,
) -> u64 {
    use crate::ipc::ring::spsc::spsc_wait_reply;
    // SAFETY: dst is a valid pointer (verified by ipc_fast_call asm).
    spsc_wait_reply(dst, channel_id, timeout_ns)
}
