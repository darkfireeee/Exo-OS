// kernel/src/ipc/core/mod.rs
//
// Module core IPC — réexporte les types fondamentaux, constantes et moteur de transfert.

core::arch::global_asm!(include_str!("fastcall_asm.s"), options(att_syntax));

pub mod types;
pub mod constants;
pub mod sequence;
pub mod transfer;

pub use types::{
    MessageId, ChannelId, EndpointId, Cookie, ProcessId,
    MsgFlags, MessageFlags, MessageType, IpcError, IpcCapError,
    alloc_message_id, alloc_channel_id, alloc_endpoint_id,
    array_index_nospec,
};
pub use constants::*;
pub use sequence::{SeqSender, SeqReceiver, SeqPair, SeqCheck};
pub use transfer::{MessageHeader, RingSlot, ZeroCopyRef, TransferEngine, TransferStats};

// ─────────────────────────────────────────────────────────────────────────────
// FFI — fonctions ASM fast path (fastcall_asm.s)
// ─────────────────────────────────────────────────────────────────────────────

/// Message IPC rapide — 80 bytes (header 16 + data 64).
/// Conçu pour tenir dans une cache line et demi.
#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub struct IpcFastMsg {
    /// Identifiant du message (0 = allouer).
    pub msg_id: u64,
    /// Drapeaux MsgFlags.
    pub flags:  u32,
    /// Longueur des données (0..=64).
    pub len:    u16,
    pub _pad:   u16,
    /// Données inline (max 64 bytes pour le fast path).
    pub data:   [u8; 64],
}

impl IpcFastMsg {
    pub const fn zeroed() -> Self {
        Self {
            msg_id: 0,
            flags:  0,
            len:    0,
            _pad:   0,
            data:   [0u8; 64],
        }
    }
}

// Ces fonctions sont implémentées dans fastcall_asm.s et appelées depuis
// ring/spsc.rs comme primitives de bas niveau.
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

// Fonctions Rust appelées depuis l'ASM (no_mangle pour linkage).
// Implémentées dans ring/spsc.rs.
#[no_mangle]
pub extern "C" fn ipc_ring_fast_write(msg: *const IpcFastMsg, channel_id: u64) -> u64 {
    use crate::ipc::ring::spsc::spsc_fast_write;
    // SAFETY: msg est un pointeur valide (vérifié par ipc_fast_send asm).
    unsafe { spsc_fast_write(msg, channel_id) }
}

#[no_mangle]
pub extern "C" fn ipc_ring_fast_read(dst: *mut IpcFastMsg, channel_id: u64) -> u64 {
    use crate::ipc::ring::spsc::spsc_fast_read;
    // SAFETY: dst est un pointeur valide (vérifié par ipc_fast_recv asm).
    unsafe { spsc_fast_read(dst, channel_id) }
}

#[no_mangle]
pub extern "C" fn ipc_ring_fast_wait_reply(
    dst:        *mut IpcFastMsg,
    channel_id: u64,
    timeout_ns: u64,
) -> u64 {
    use crate::ipc::ring::spsc::spsc_wait_reply;
    // SAFETY: dst est un pointeur valide (vérifié par ipc_fast_call asm).
    unsafe { spsc_wait_reply(dst, channel_id, timeout_ns) }
}
