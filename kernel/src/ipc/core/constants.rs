// kernel/src/ipc/core/constants.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// GLOBAL IPC CONSTANTS
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// All sizing constants for IPC are centralized here.
// Any modification must be accompanied by an update of the impact
// comment and validation of the SHM pool memory budget.
// ═══════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// Message constraints
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum size of an inline message in the ring (physical copy).
/// 240 bytes = 256 - 16 (header) -> 1 slot fits in 4 cache lines.
/// Messages > 240 bytes must take the zero-copy SHM path.
pub use crate::arch::constants::MAX_MSG_SIZE;

/// Inline size reserved for protocols that append their own header.
pub use crate::arch::constants::IPC_INLINE_MAX;

/// Payload size in the Ring1 ABI envelope (`syscall_abi::IpcMessage`).
/// Ring1 servers use 8 bytes of header (`sender_pid`, `msg_type`)
/// and 192 bytes of payload, i.e., 200 bytes per userspace envelope.
/// The kernel also accepts raw messages up to `MAX_MSG_SIZE`; if the
/// receiver provides a buffer smaller than the message at head of queue,
/// `recv_raw` returns `MessageTooLarge` without consuming or truncating the message.
pub const ABI_IPC_HEADER_SIZE: usize = 8;
pub const ABI_IPC_CAP_TOKEN_SIZE: usize = 20;
pub const ABI_IPC_PAYLOAD_SIZE: usize = 192;
pub const ABI_IPC_ENVELOPE_SIZE: usize = ABI_IPC_HEADER_SIZE + ABI_IPC_PAYLOAD_SIZE;
pub const IPC_CAP_TOKEN_PAYLOAD_OFFSET: usize = ABI_IPC_PAYLOAD_SIZE - ABI_IPC_CAP_TOKEN_SIZE;
pub const IPC_CAP_TOKEN_OFFSET: usize = ABI_IPC_HEADER_SIZE + IPC_CAP_TOKEN_PAYLOAD_OFFSET;
pub const IPC_CAP_TOKEN_SIZE: usize = ABI_IPC_CAP_TOKEN_SIZE;

const _: () = assert!(
    ABI_IPC_ENVELOPE_SIZE <= MAX_MSG_SIZE,
    "l'enveloppe IPC ABI doit tenir dans un slot SPSC"
);
const _: () = assert!(
    ABI_IPC_ENVELOPE_SIZE == 200,
    "l'enveloppe IPC ABI v0.2 doit faire 200 octets"
);
const _: () = assert!(
    IPC_CAP_TOKEN_OFFSET + IPC_CAP_TOKEN_SIZE == ABI_IPC_ENVELOPE_SIZE,
    "le jeton de capabilite doit terminer l'enveloppe IPC ABI"
);

/// Message header size (MessageHeader in the ring).
/// 16 bytes: msg_id(8) + flags(4) + len(2) + pad(2).
pub const MSG_HEADER_SIZE: usize = 16;

/// Total size of a ring slot = header + payload.
/// 240 + 16 = 256 bytes = 4 cache lines of 64 bytes.
pub const RING_SLOT_SIZE: usize = MSG_HEADER_SIZE + MAX_MSG_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Ring sizing
// ─────────────────────────────────────────────────────────────────────────────

/// Number of slots in a standard ring (SPSC/MPMC).
/// Power of 2 — mod mask = RING_SIZE - 1.
/// 16 slots are sufficient for the fast path; large volumes pass
/// through the zero-copy SHM ring (no inline copy in slots).
pub const RING_SIZE: usize = 16;

/// Modulo mask for a ring of RING_SIZE slots.
/// N/A if RING_SIZE is not a power of 2 (statically verified).
pub const RING_MASK: usize = RING_SIZE - 1;

const _RING_SIZE_IS_POW2: () = assert!(
    RING_SIZE.is_power_of_two(),
    "RING_SIZE doit être une puissance de 2"
);

/// Minimum size of a batch ring (fusion ring).
pub const FUSION_RING_SIZE: usize = 256;

/// Batching threshold for the Fusion Ring (number of msgs before forced flush).
pub const FUSION_BATCH_THRESHOLD: usize = 16;

/// Maximum retention delay in the Fusion Ring (in scheduler ticks = 1ms at HZ=1000).
pub const FUSION_MAX_DELAY_TICKS: u64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Shared Memory
// ─────────────────────────────────────────────────────────────────────────────

/// Number of pre-allocated pages in the SHM pool.
/// 256 pages × 4096 bytes = 1 MiB reserved at boot.
pub const SHM_POOL_PAGES: usize = 256;

/// Size of a physical page (identical to memory/core/constants.rs PAGE_SIZE).
pub const PAGE_SIZE: usize = 4_096;

/// Size of the SHM pool in bytes.
pub const SHM_POOL_BYTES: usize = SHM_POOL_PAGES * PAGE_SIZE;

/// Maximum number of simultaneously active SHM regions.
pub const SHM_MAX_REGIONS: usize = 1_024;

/// Mandatory alignment for SHM allocations (NO_COW flag).
pub const SHM_ALIGN: usize = PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Endpoints and channels
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of endpoints in the global registry.
pub use crate::arch::constants::MAX_ENDPOINTS;

/// Maximum length of an endpoint name (ASCII, null-terminated in registry).
pub const MAX_ENDPOINT_NAME_LEN: usize = 64;

/// Incoming connection backlog per endpoint.
pub const ENDPOINT_BACKLOG: usize = 32;

/// Maximum number of simultaneously open channels (all directions).
pub const MAX_CHANNELS: usize = 65_536;

/// Maximum number of concurrent owners of a single endpoint.
pub const MAX_ENDPOINT_OWNERS: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// RPC
// ─────────────────────────────────────────────────────────────────────────────

/// Default RPC timeout in nanoseconds (5 ms).
pub const RPC_DEFAULT_TIMEOUT_NS: u64 = 5_000_000;

/// Maximum RPC timeout (100 ms).
pub const RPC_MAX_TIMEOUT_NS: u64 = 100_000_000;

/// Maximum number of in-flight RPC requests simultaneously (per client).
pub const RPC_MAX_INFLIGHT: usize = 64;

/// Maximum number of RPC retry attempts (after timeout).
pub const RPC_MAX_RETRIES: usize = 3;

/// RPC magic in protocol header.
/// 0xEA045250 = 'E','A',0x04,'R','P' — RPC frame marker.
pub const RPC_MAGIC: u32 = 0xEA04_5250;

/// Binary RPC protocol version.
pub const RPC_PROTOCOL_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Synchronization
// ─────────────────────────────────────────────────────────────────────────────

/// Magic value for IPC futexes (distinguishes from normal memory futexes).
pub const IPC_FUTEX_MAGIC: u32 = 0x1FCF_07E0;

/// Maximum timeout of a wait on IPC WaitQueue (in nanoseconds) = 1 second.
pub const IPC_WAIT_MAX_NS: u64 = 1_000_000_000;

/// Maximum number of simultaneous waiters on the same rendezvous.
pub const RENDEZVOUS_MAX_WAITERS: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// Sequence numbers
// ─────────────────────────────────────────────────────────────────────────────

/// Number of bits of the sequence sliding window (ordering detection).
pub const SEQ_WINDOW_BITS: u32 = 7; // window = 128 messages

/// Sequence tolerance window: beyond this, the message is considered out-of-order.
pub const SEQ_WINDOW_SIZE: u64 = 1 << SEQ_WINDOW_BITS;

// ─────────────────────────────────────────────────────────────────────────────
// Performance / latency targets
// ─────────────────────────────────────────────────────────────────────────────

/// Target latency for fast inline IPC (without syscall): < 200 ns.
pub const TARGET_FAST_IPC_NS: u64 = 200;

/// Target latency for full path synchronous message: < 2 µs.
pub const TARGET_SYNC_IPC_NS: u64 = 2_000;

/// Target throughput of msgs/s per SPSC channel: > 10 million msgs/s.
pub const TARGET_SPSC_THROUGHPUT: u64 = 10_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// Version constants and global limits
// ─────────────────────────────────────────────────────────────────────────────

/// IPC subsystem version (kernel internal version).
pub const IPC_VERSION: u32 = 1;

/// Maximum number of simultaneously open channels.
/// Explicit alias of MAX_CHANNELS for public re-exports.
pub const IPC_MAX_CHANNELS: usize = MAX_CHANNELS;

/// Maximum number of endpoints in the global registry.
/// Explicit alias of MAX_ENDPOINTS.
pub const IPC_MAX_ENDPOINTS: usize = MAX_ENDPOINTS;

/// Maximum number of processes that can hold IPC resources.
///
/// FIX-IPC-PROCS (rapport_analyse §4.3, inconsistency #3):
/// IPC_MAX_PROCESSES was 65,536 whereas the process table (MAX_PROCESSES)
/// is limited to 32,768. IPC cannot track more processes than the kernel
/// can host — both constants must remain synchronized.
/// Aligned on arch::constants::MAX_PROCESSES via re-export.
pub use crate::arch::constants::MAX_PROCESSES as IPC_MAX_PROCESSES;

// ─────────────────────────────────────────────────────────────────────────────
// Message constants (header)
// ─────────────────────────────────────────────────────────────────────────────

/// Magic value in message frame header (MsgFrameHeader).
/// 0x4D534748 = 'M','S','G','H'
pub const MSG_HEADER_MAGIC: u32 = 0x4D53_4748;

// ─────────────────────────────────────────────────────────────────────────────
// Synchronous channel timeouts
// ─────────────────────────────────────────────────────────────────────────────

/// Default timeout of a synchronous channel (spin-wait) in nanoseconds = 5 ms.
pub const SYNC_CHANNEL_TIMEOUT_NS: u64 = 5_000_000;
