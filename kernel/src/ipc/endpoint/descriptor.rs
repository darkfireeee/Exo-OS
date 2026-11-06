// kernel/src/ipc/endpoint/descriptor.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ENDPOINT DESCRIPTOR — Full description of an IPC endpoint
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// An EndpointDesc describes a named communication point:
//   • Unique identifier (EndpointId)
//   • List of owners (ThreadId) allowed to accept connections
//   • Incoming connection queue (backlog)
//   • State (Listening, Closed, Drain)
//
// RULE: zero heap allocation here — fixed-size structures (NO-ALLOC Zone).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::{ENDPOINT_BACKLOG, MAX_ENDPOINT_NAME_LEN, MAX_ENDPOINT_OWNERS};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::scheduler::core::task::ThreadId;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// EndpointState — endpoint state machine
// ─────────────────────────────────────────────────────────────────────────────

/// State of an IPC endpoint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum EndpointState {
    /// Endpoint created, not yet listening.
    Created = 0,
    /// Listening — accepts connections.
    Listening = 1,
    /// Closed — rejects all connections.
    Closed = 2,
    /// Draining in-flight messages during shutdown.
    Draining = 3,
    /// Destroyed — structure can be reused.
    Destroyed = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// PendingConnection — connection pending in backlog
// ─────────────────────────────────────────────────────────────────────────────

/// A connection pending acceptance.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PendingConnection {
    /// Requesting thread.
    pub requester: ThreadId,
    /// Channel proposed for connection.
    pub channel_id: u64,
    /// Cookie to correlate the reply.
    pub cookie: u64,
    /// Request timestamp (ticks).
    pub timestamp_ticks: u64,
}

impl PendingConnection {
    pub const EMPTY: Self = Self {
        requester: ThreadId(0),
        channel_id: 0,
        cookie: 0,
        timestamp_ticks: 0,
    };

    pub fn is_valid(&self) -> bool {
        self.requester.0 != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointName — null-terminated ASCII name
// ─────────────────────────────────────────────────────────────────────────────

/// Endpoint name — max 64 bytes ASCII.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct EndpointName {
    bytes: [u8; MAX_ENDPOINT_NAME_LEN],
    len: u8,
}

impl EndpointName {
    pub const EMPTY: Self = Self {
        bytes: [0u8; MAX_ENDPOINT_NAME_LEN],
        len: 0,
    };

    /// Creates a name from an ASCII slice.
    pub fn from_bytes(src: &[u8]) -> Result<Self, IpcError> {
        if src.len() >= MAX_ENDPOINT_NAME_LEN {
            return Err(IpcError::InvalidParam);
        }
        let mut name = Self::EMPTY;
        name.bytes[..src.len()].copy_from_slice(src);
        name.len = src.len() as u8;
        Ok(name)
    }

    /// Returns byte slice of name.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Returns length of name.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Compares two names (case sensitivity not required).
    pub fn eq_bytes(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointDesc — main descriptor
// ─────────────────────────────────────────────────────────────────────────────

/// Full descriptor of an IPC endpoint.
/// Fixed size — allocated from a static pool (endpoint/registry.rs).
#[repr(C, align(64))]
pub struct EndpointDesc {
    /// Unique identifier of endpoint.
    pub id: EndpointId,
    /// Current state (atomic for lock-free read).
    pub state: AtomicU32,
    /// Number of active connections.
    pub active_conns: AtomicU32,
    /// Generation (incremented on each close/reopen).
    pub generation: AtomicU64,
    /// Owners allowed to accept connections (max MAX_ENDPOINT_OWNERS).
    pub owners: [ThreadId; MAX_ENDPOINT_OWNERS],
    /// Number of active owners.
    pub owner_count: u32,
    /// Configuration flags (e.g. BROADCAST, RPC, STREAMING).
    pub config_flags: u32,
    /// Endpoint name (ASCII).
    pub name: EndpointName,
    /// Backlog: queue of connections pending acceptance.
    pub backlog: [PendingConnection; ENDPOINT_BACKLOG],
    /// Backlog head (next index to accept).
    pub backlog_head: AtomicU32,
    /// Backlog tail (next free slot index).
    pub backlog_tail: AtomicU32,
    /// Total counter of accepted connections.
    pub total_accepted: AtomicU64,
    /// Total counter of refused connections (backlog full).
    pub total_refused: AtomicU64,
}

impl EndpointDesc {
    /// Creates a new endpoint descriptor.
    pub fn new(id: EndpointId, name: EndpointName, owner: ThreadId) -> Self {
        let mut owners = [ThreadId(0); MAX_ENDPOINT_OWNERS];
        owners[0] = owner;
        Self {
            id,
            state: AtomicU32::new(EndpointState::Created as u32),
            active_conns: AtomicU32::new(0),
            generation: AtomicU64::new(1),
            owners,
            owner_count: 1,
            config_flags: 0,
            name,
            backlog: [PendingConnection::EMPTY; ENDPOINT_BACKLOG],
            backlog_head: AtomicU32::new(0),
            backlog_tail: AtomicU32::new(0),
            total_accepted: AtomicU64::new(0),
            total_refused: AtomicU64::new(0),
        }
    }

    /// Returns current state.
    #[inline(always)]
    pub fn state(&self) -> EndpointState {
        match self.state.load(Ordering::Acquire) {
            0 => EndpointState::Created,
            1 => EndpointState::Listening,
            2 => EndpointState::Closed,
            3 => EndpointState::Draining,
            4 => EndpointState::Destroyed,
            _ => EndpointState::Closed, // invalid value -> closed for safety
        }
    }

    /// Transitions to Listening state.
    pub fn start_listen(&self) -> Result<(), IpcError> {
        self.state
            .compare_exchange(
                EndpointState::Created as u32,
                EndpointState::Listening as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .map(|_| ())
            .map_err(|_| IpcError::InvalidParam)
    }

    /// Returns true if endpoint accepts connections.
    #[inline(always)]
    pub fn is_listening(&self) -> bool {
        self.state.load(Ordering::Acquire) == EndpointState::Listening as u32
    }

    /// Enqueues a connection into backlog.
    /// Returns `Err(WouldBlock)` if backlog is full.
    pub fn enqueue_connection(&self, conn: PendingConnection) -> Result<(), IpcError> {
        let tail = self.backlog_tail.load(Ordering::Relaxed);
        let head = self.backlog_head.load(Ordering::Acquire);
        let next = (tail + 1) % ENDPOINT_BACKLOG as u32;
        if next == head {
            self.total_refused.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: tail in [0, ENDPOINT_BACKLOG), space verified (next != head), single producer.
        unsafe {
            let slot = core::ptr::addr_of!(self.backlog[tail as usize]) as *mut PendingConnection;
            core::ptr::write(slot, conn);
        }
        self.backlog_tail.store(next, Ordering::Release);
        Ok(())
    }

    /// Dequeues a connection from backlog.
    pub fn dequeue_connection(&self) -> Option<PendingConnection> {
        let head = self.backlog_head.load(Ordering::Relaxed);
        let tail = self.backlog_tail.load(Ordering::Acquire);
        if head == tail {
            return None; // empty backlog
        }
        // SAFETY: head is in [0, ENDPOINT_BACKLOG) and tail != head.
        let conn = unsafe { core::ptr::read(&self.backlog[head as usize]) };
        let next = (head + 1) % ENDPOINT_BACKLOG as u32;
        self.backlog_head.store(next, Ordering::Release);
        self.total_accepted.fetch_add(1, Ordering::Relaxed);
        Some(conn)
    }

    /// Returns number of pending connections in backlog.
    #[inline(always)]
    pub fn backlog_len(&self) -> usize {
        let h = self.backlog_head.load(Ordering::Relaxed) as usize;
        let t = self.backlog_tail.load(Ordering::Relaxed) as usize;
        t.wrapping_sub(h) % ENDPOINT_BACKLOG
    }

    /// Checks whether a ThreadId owns this endpoint.
    pub fn is_owner(&self, tid: ThreadId) -> bool {
        self.owners[..self.owner_count as usize].contains(&tid)
    }

    /// Adds an additional owner.
    pub fn add_owner(&mut self, tid: ThreadId) -> Result<(), IpcError> {
        if self.owner_count as usize >= MAX_ENDPOINT_OWNERS {
            return Err(IpcError::ResourceExhausted);
        }
        self.owners[self.owner_count as usize] = tid;
        self.owner_count += 1;
        Ok(())
    }
}
