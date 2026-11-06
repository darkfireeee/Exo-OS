// kernel/src/ipc/core/types.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FUNDAMENTAL IPC TYPES — MessageId, ChannelId, EndpointId, Cookie
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE IPC-LAYER: ipc/ resides in Layer 2a.
//   DEPENDS ON: memory/ (Layer 0) + scheduler/ (Layer 1) + security/capability/
//   NEVER IMPORTS: fs/, process/ (Upper layers)
//
// RULE UNSAFE: every unsafe block preceded by // SAFETY:
// RULE NO-ALLOC: this file — zero Vec/Box/Arc (basic types only)
//
// sender_pid contract:
// IPC messages exposed to servers may contain a sender_pid field
// in the first bytes of the payload. This field is untrusted from
// userspace: `SYS_EXO_IPC_SEND` replaces it with the real PID only if
// the caller sets the ABI flag IPC_FLAG_INJECT_SRC_PID. Raw formats whose
// first four bytes are not a sender_pid are therefore no longer corrupted
// by a size heuristic.
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, Ordering};

// Re-export ProcessId from scheduler (ProcessId is defined there as ProcessId(pub u32)).
pub use crate::scheduler::ProcessId;

// ─────────────────────────────────────────────────────────────────────────────
// Opaque identifiers — Newtype wrappers around u64
// Value 0 is reserved as "invalid / uninitialized".
// ─────────────────────────────────────────────────────────────────────────────

/// Unique identifier of an IPC message.
/// Monotonically generated, never reused (rollover = panic in debug).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct MessageId(pub NonZeroU64);

impl MessageId {
    /// Creates a MessageId from a raw non-zero value.
    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    /// Creates a MessageId from a raw value without checking.
    ///
    /// # Safety
    /// `v` must not be zero.
    #[inline(always)]
    pub unsafe fn new_unchecked(v: u64) -> Self {
        Self(NonZeroU64::new_unchecked(v))
    }

    /// Returns the underlying value.
    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgId({})", self.0.get())
    }
}

/// Unique identifier of an IPC channel (bidirectional or unidirectional).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ChannelId(pub NonZeroU64);

impl ChannelId {
    /// "Dangling" sentinel — invalid value used as initial value
    /// before an actual `ChannelId` is allocated.
    pub const DANGLING: Self = Self(match NonZeroU64::new(u64::MAX) {
        Some(v) => v,
        None => panic!("u64::MAX is non-zero"),
    });

    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChanId({})", self.0.get())
    }
}

/// Unique identifier of an endpoint (named communication point).
/// An endpoint can accept incoming connections.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct EndpointId(pub NonZeroU64);

impl EndpointId {
    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }

    /// "Invalid" sentinel — sentinel value used in place of
    /// `EndpointId(0)` which is impossible (`NonZeroU64`).
    pub const INVALID: Self = Self(match NonZeroU64::new(u64::MAX) {
        Some(v) => v,
        None => panic!("u64::MAX is non-zero"),
    });

    /// Converts to ObjectId for the capability system.
    /// High 32 bits encode the type (0x01 = IPC endpoint).
    #[inline(always)]
    pub fn to_object_id(self) -> u64 {
        (0x01u64 << 32) | (self.0.get() & 0xFFFF_FFFF)
    }
}

impl fmt::Debug for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EpId({})", self.0.get())
    }
}

/// Opaque cookie — arbitrary 64-bit value attached to a message or connection.
/// Allows correlating requests/replies statelessly on server side.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct Cookie(pub u64);

impl Cookie {
    pub const ZERO: Self = Self(0);

    #[inline(always)]
    pub fn new(v: u64) -> Self {
        Self(v)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread-safe monotonic generators
// ─────────────────────────────────────────────────────────────────────────────

/// Monotonic identifier generator for messages.
static MSG_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocates a unique, monotonic MessageId.
/// Upon rollover (u64 max), system is exhausted — panics in debug.
#[inline]
pub fn alloc_message_id() -> MessageId {
    let v = MSG_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "MessageId counter rollover — system exhausted");
    // SAFETY: v originates from a fetch_add from 1 — never zero until rollover.
    MessageId(unsafe { NonZeroU64::new_unchecked(v) })
}

/// Monotonic identifier generator for channels.
static CHAN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocates a unique, monotonic ChannelId.
#[inline]
pub fn alloc_channel_id() -> ChannelId {
    let v = CHAN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "ChannelId counter rollover");
    // SAFETY: v >= 1 by construction.
    ChannelId(unsafe { NonZeroU64::new_unchecked(v) })
}

/// Monotonic identifier generator for endpoints.
static EP_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocates a unique, monotonic EndpointId.
#[inline]
pub fn alloc_endpoint_id() -> EndpointId {
    let v = EP_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "EndpointId counter rollover");
    // SAFETY: v >= 1 by construction.
    EndpointId(unsafe { NonZeroU64::new_unchecked(v) })
}

// ─────────────────────────────────────────────────────────────────────────────
// Message flags
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask flags for an IPC message.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct MsgFlags(pub u32);

impl MsgFlags {
    /// No flag set.
    pub const EMPTY: Self = Self(0);
    /// Real-time message (highest priority in queues).
    pub const RT: Self = Self(1 << 0);
    /// Reply message (correlates to an existing MessageId).
    pub const REPLY: Self = Self(1 << 1);
    /// Zero-copy: content is a physical reference, not a copy.
    pub const ZEROCOPY: Self = Self(1 << 2);
    /// Broadcast: deliver to all receivers.
    pub const BROADCAST: Self = Self(1 << 3);
    /// Error message.
    pub const ERROR: Self = Self(1 << 4);
    /// Synchronous: blocks sender until acknowledgement.
    pub const SYNC: Self = Self(1 << 5);
    /// Non-blocking: returns immediately if queue is full.
    pub const NOWAIT: Self = Self(1 << 6);

    #[inline(always)]
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    #[inline(always)]
    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    #[inline(always)]
    pub fn set(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    #[inline(always)]
    pub fn clear(self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }

    /// Returns the raw bitmask value (bitflags compatibility).
    #[inline(always)]
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Constructs from a raw value by truncating unknown bits.
    #[inline(always)]
    pub fn from_bits_truncate(v: u32) -> Self {
        Self(v & 0x7F)
    }

    /// Sets a flag in place (bitflags `.insert()` compatibility).
    #[inline(always)]
    pub fn insert(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    /// Clears a flag in place.
    #[inline(always)]
    pub fn remove(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl From<MessageFlags> for MsgFlags {
    #[inline(always)]
    fn from(flags: MessageFlags) -> Self {
        MsgFlags::from_bits_truncate(flags.bits() as u32)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IPC results and errors
// ─────────────────────────────────────────────────────────────────────────────

/// IPC errors — exhaustive, no _ catch-all.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum IpcError {
    /// Non-blocking operation on a full queue.
    WouldBlock = 1,
    /// Endpoint not found in registry.
    EndpointNotFound = 2,
    /// Channel closed by peer.
    ChannelClosed = 3,
    /// Permission denied by capability system.
    PermissionDenied = 4,
    /// Message too large (exceeds MAX_MSG_SIZE).
    MessageTooLarge = 5,
    /// Timeout expired.
    Timeout = 6,
    /// IPC resource exhausted (SHM pool, CapToken, etc.).
    ResourceExhausted = 7,
    /// Connection refused (endpoint not listening).
    ConnRefused = 8,
    /// Already connected.
    AlreadyConnected = 9,
    /// Invalid parameter.
    InvalidParam = 10,
    /// Handshake failed (version, magic mismatch).
    HandshakeFailed = 11,
    /// Operation interrupted by signal.
    Interrupted = 12,
    /// Unexpected kernel internal error.
    InternalError = 13,
    /// SHM pool saturated.
    ShmPoolFull = 14,
    /// Out-of-order sequence.
    OutOfOrder = 15,
    /// Invalid or expired IPC handle.
    InvalidHandle = 16,
    /// Channel / connection closed — expressive alias for ChannelClosed.
    Closed = 17,
    /// Kernel internal error — expressive alias for InternalError.
    Internal = 18,
    /// Invalid argument / data — expressive alias for InvalidParam.
    Invalid = 19,
    /// Queue/ring full.
    Full = 20,
    /// Routing loop detected.
    Loop = 21,
    /// Resource not found.
    NotFound = 22,
    /// Null endpoint (EndpointId 0) passed to an operation.
    NullEndpoint = 23,
    /// Invalid endpoint (corrupted, deallocated, wrong type).
    InvalidEndpoint = 24,
    /// Operation to retry (exponential backoff).
    Retry = 25,
    /// Invalid argument — alias for InvalidParam with stronger semantics.
    InvalidArgument = 26,
    /// IPC resources exhausted (full table, empty pool, etc.).
    OutOfResources = 27,
    /// Queue/ring full on push (non-blocking).
    QueueFull = 28,
    /// Queue/ring empty on pop (non-blocking).
    QueueEmpty = 29,
    /// Protocol error (magic/version mismatch, invalid sequence).
    ProtocolError = 30,
    /// IPC memory mapping failed (SHM, zero-copy page fault).
    MappingFailed = 31,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WouldBlock => write!(f, "IPC: opération non bloquante sur file pleine"),
            Self::EndpointNotFound => write!(f, "IPC: endpoint introuvable"),
            Self::ChannelClosed => write!(f, "IPC: canal fermé"),
            Self::PermissionDenied => write!(f, "IPC: accès refusé (capability)"),
            Self::MessageTooLarge => write!(f, "IPC: message trop grand"),
            Self::Timeout => write!(f, "IPC: timeout expiré"),
            Self::ResourceExhausted => write!(f, "IPC: ressource épuisée"),
            Self::ConnRefused => write!(f, "IPC: connexion refusée"),
            Self::AlreadyConnected => write!(f, "IPC: déjà connecté"),
            Self::InvalidParam => write!(f, "IPC: paramètre invalide"),
            Self::HandshakeFailed => write!(f, "IPC: échec handshake"),
            Self::Interrupted => write!(f, "IPC: interrompu par signal"),
            Self::InternalError => write!(f, "IPC: erreur interne"),
            Self::ShmPoolFull => write!(f, "IPC: pool SHM saturé"),
            Self::OutOfOrder => write!(f, "IPC: séquence hors ordre"),
            Self::InvalidHandle => write!(f, "IPC: handle invalide"),
            Self::Closed => write!(f, "IPC: connexion fermée"),
            Self::Internal => write!(f, "IPC: erreur interne kernel"),
            Self::Invalid => write!(f, "IPC: données invalides"),
            Self::Full => write!(f, "IPC: file pleine"),
            Self::Loop => write!(f, "IPC: boucle de routage détectée"),
            Self::NotFound => write!(f, "IPC: ressource introuvable"),
            Self::NullEndpoint => write!(f, "IPC: endpoint nul"),
            Self::InvalidEndpoint => write!(f, "IPC: endpoint invalide"),
            Self::Retry => write!(f, "IPC: réessayer l'opération"),
            Self::InvalidArgument => write!(f, "IPC: argument invalide"),
            Self::OutOfResources => write!(f, "IPC: ressources épuisées"),
            Self::QueueFull => write!(f, "IPC: file pleine (push refusé)"),
            Self::QueueEmpty => write!(f, "IPC: file vide (pop échoué)"),
            Self::ProtocolError => write!(f, "IPC: erreur de protocole (magic/version)"),
            Self::MappingFailed => write!(f, "IPC: échec de mapping mémoire"),
        }
    }
}

impl IpcError {
    /// Normalizes expressive aliases to canonical variants.
    #[inline]
    pub const fn normalize(self) -> Self {
        match self {
            Self::Closed => Self::ChannelClosed,
            Self::Internal => Self::InternalError,
            Self::Invalid | Self::InvalidArgument => Self::InvalidParam,
            Self::Full | Self::QueueFull => Self::WouldBlock,
            Self::NotFound => Self::EndpointNotFound,
            Self::OutOfResources => Self::ResourceExhausted,
            other => other,
        }
    }

    /// True for all variants meaning channel/endpoint closure.
    #[inline]
    pub const fn is_closed(self) -> bool {
        matches!(self.normalize(), Self::ChannelClosed)
    }

    /// True for full-queue / non-blocking operation variants.
    #[inline]
    pub const fn is_queue_full(self) -> bool {
        matches!(self.normalize(), Self::WouldBlock)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageFlags — bitmask flags for messages in message/
// ─────────────────────────────────────────────────────────────────────────────

/// IPC message flags (used by message/builder.rs, serializer.rs, etc.)
/// Represented on 16 bits to fit in MsgFrameHeader.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct MessageFlags(pub u16);

impl MessageFlags {
    /// No flag set.
    pub const NONE: Self = Self(0);
    /// Real-time message.
    pub const RT: Self = Self(1 << 0);
    /// Reply message.
    pub const REPLY: Self = Self(1 << 1);
    /// Zero-copy.
    pub const ZEROCOPY: Self = Self(1 << 2);
    /// Broadcast.
    pub const BROADCAST: Self = Self(1 << 3);
    /// Error.
    pub const ERROR: Self = Self(1 << 4);
    /// Synchronous.
    pub const SYNC: Self = Self(1 << 5);
    /// Non-blocking.
    pub const NOWAIT: Self = Self(1 << 6);

    /// Returns raw value.
    #[inline(always)]
    pub const fn empty() -> Self {
        Self::NONE
    }

    #[inline(always)]
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Constructs from raw value by truncating unknown bits.
    #[inline(always)]
    pub fn from_bits_truncate(v: u16) -> Self {
        Self(v & 0x7F)
    }

    #[inline(always)]
    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) == flag.0
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl From<MsgFlags> for MessageFlags {
    #[inline(always)]
    fn from(flags: MsgFlags) -> Self {
        MessageFlags::from_bits_truncate(flags.bits() as u16)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageType — semantic message type
// ─────────────────────────────────────────────────────────────────────────────

/// Semantic type of an IPC message.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MessageType {
    #[default]
    /// Raw data message.
    Data = 0,
    /// Channel control message.
    Control = 1,
    /// Signal notification.
    Signal = 2,
    /// RPC reply.
    RpcReply = 3,
}

impl MessageType {
    /// Creates a MessageType from a raw byte.
    /// Unknown values -> `Data` (degraded policy, no panic).
    #[inline(always)]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Data,
            1 => Self::Control,
            2 => Self::Signal,
            3 => Self::RpcReply,
            _ => Self::Data,
        }
    }

    /// Returns the raw value.
    #[inline(always)]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IpcCapError — IPC variant of CapError (v6 — access via security::access_control)
// ─────────────────────────────────────────────────────────────────────────────

/// IPC-specific capability error.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum IpcCapError {
    /// Revoked token or incorrect generation.
    Revoked = 1,
    /// Capability object not found.
    ObjectNotFound = 2,
    /// Insufficient rights for this operation.
    InsufficientRights = 3,
    /// Delegation denied (DELEGATE flag missing).
    DelegationDenied = 4,
}

impl From<IpcCapError> for IpcError {
    fn from(e: IpcCapError) -> Self {
        match e {
            IpcCapError::Revoked => IpcError::PermissionDenied,
            IpcCapError::ObjectNotFound => IpcError::EndpointNotFound,
            IpcCapError::InsufficientRights => IpcError::PermissionDenied,
            IpcCapError::DelegationDenied => IpcError::PermissionDenied,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Spectre v1 mitigation — array_index_nospec (RULE IPC-08)
// ─────────────────────────────────────────────────────────────────────────────

/// Mitigates Spectre v1 during indexed buffer accesses.
///
/// Returns `index` if `index < size`, 0 otherwise — using a mask
/// computed without branching on the sign bit of `index - size`.
///
/// The CPU cannot speculate beyond buffer bounds because the mask
/// forces out-of-bounds index to 0 before memory access.
///
/// MANDATORY USAGE (IPC-08) on any access of the form:
///   `buffer[unsafe_index_from_user_or_ring]`
///
/// # Example
/// ```no_run
/// let safe_idx = array_index_nospec(user_idx, RING_SIZE);
/// let cell = &buffer[safe_idx];
/// ```
#[inline(always)]
pub fn array_index_nospec(index: usize, size: usize) -> usize {
    // Linux kernel technique: arithmetic right-shift of sign of (index - size).
    // If index < size  : index.wrapping_sub(size) -> negative value (signed) -> MSB=1
    //                    -> shift yields 0xFFFF...FF -> mask is all-ones -> index unchanged.
    // If index >= size : index.wrapping_sub(size) -> >= 0 (signed) -> MSB=0
    //                    -> shift yields 0x0000...00 -> mask is all-zeros -> result = 0.
    let mask = (index.wrapping_sub(size) as isize >> (isize::BITS - 1)) as usize;
    index & mask
}
