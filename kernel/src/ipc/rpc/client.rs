// ipc/rpc/client.rs — IPC RPC client for Exo-OS
//
// This module implements a synchronous IPC RPC client.
// A client sends a serialized RpcCallFrame to a server endpoint and awaits
// the returning RpcReplyFrame via an IPC SyncChannel.
//
// Architecture:
//   - `RpcClient`: client handle with cookie counter + endpoint + retry state
//   - `call()`: serializes call, sends it, spin-waits for reply
//   - `RpcClientTable`: static table of MAX_RPC_CLIENTS clients
//
// Dependencies:
//   - `ipc::channel::sync` for synchronous transport
//   - `ipc::rpc::protocol` for frames
//   - `ipc::rpc::timeout` for retry policy
//
// RULE CLIENT-01: call() is SYNCHRONOUS — blocks until reply or timeout.
// RULE CLIENT-02: unique cookie per call (incremental AtomicU64).
// RULE CLIENT-03: transparent retry with exponential backoff.

use core::mem::MaybeUninit;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError, ProcessId};
use crate::ipc::rpc::protocol::{
    MethodId, RpcCallFrame, RpcHeader, RpcReplyFrame, RpcStatus, MAX_RPC_PAYLOAD,
};
use crate::ipc::rpc::timeout::{RetryState, RpcTimeout, RPC_TIMEOUT_STATS};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of RPC clients in global table
pub const MAX_RPC_CLIENTS: usize = 64;

/// Serialization buffer size for call/reply (header + max payload)
pub const RPC_CALL_BUF_SIZE: usize = 64 + MAX_RPC_PAYLOAD;

// ---------------------------------------------------------------------------
// Result of an RPC call
// ---------------------------------------------------------------------------

/// Result of an RPC call.
pub struct RpcResult {
    /// Reply status
    pub status: RpcStatus,
    /// Reply payload (copied inline)
    pub payload: [u8; MAX_RPC_PAYLOAD],
    /// Payload size
    pub payload_len: usize,
    /// Correlation cookie (for verification)
    pub cookie: u64,
}

impl RpcResult {
    pub fn empty() -> Self {
        Self {
            status: RpcStatus::Ok,
            payload: [0u8; MAX_RPC_PAYLOAD],
            payload_len: 0,
            cookie: 0,
        }
    }

    pub fn payload_data(&self) -> &[u8] {
        &self.payload[..self.payload_len]
    }

    pub fn is_ok(&self) -> bool {
        self.status.is_ok()
    }
}

// ---------------------------------------------------------------------------
// Abstract transport: fn pointer
// ---------------------------------------------------------------------------

/// RPC transport function: takes a serialized call_buf, returns
/// reply in reply_buf. Returns number of reply bytes.
///
/// This abstraction allows using different transports
/// (SyncChannel, direct shared memory, loopback...).
pub type RpcTransportFn = fn(
    server_ep: EndpointId,
    caller_ep: EndpointId,
    call_buf: &[u8],
    reply_buf: &mut [u8],
) -> Result<usize, IpcError>;

// ---------------------------------------------------------------------------
// RpcClient
// ---------------------------------------------------------------------------

/// IPC RPC client.
///
/// Maintains a cookie counter, a client endpoint, and a configurable
/// transport function.
pub struct RpcClient {
    pub id: u32,
    /// Client endpoint (source of calls) stored as u64 (EndpointId.0.get()).
    pub client_ep: AtomicU64,
    /// Client process PID
    pub pid: AtomicU32,
    /// Client active
    pub active: AtomicBool,
    _pad: [u8; 3],
    /// Cookie counter (incremental)
    cookie_counter: AtomicU64,
    /// Configured transport (fn pointer)
    transport: core::sync::atomic::AtomicUsize,
    /// Statistics
    pub total_calls: AtomicU64,
    pub total_errors: AtomicU64,
    pub total_timeouts: AtomicU64,
    pub total_retries_used: AtomicU64,
}

// SAFETY: AtomicUsize for fn pointer
unsafe impl Sync for RpcClient {}
unsafe impl Send for RpcClient {}

impl RpcClient {
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            client_ep: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            active: AtomicBool::new(false),
            _pad: [0u8; 3],
            cookie_counter: AtomicU64::new(1),
            transport: core::sync::atomic::AtomicUsize::new(0),
            total_calls: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            total_retries_used: AtomicU64::new(0),
        }
    }

    pub fn start(&self, ep: EndpointId, pid: ProcessId, transport: RpcTransportFn) {
        self.client_ep.store(ep.0.get(), Ordering::Relaxed);
        self.pid.store(pid.0, Ordering::Relaxed);
        self.transport.store(transport as usize, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }

    fn next_cookie(&self) -> u64 {
        self.cookie_counter.fetch_add(1, Ordering::Relaxed)
    }

    fn transport_fn(&self) -> Option<RpcTransportFn> {
        let p = self.transport.load(Ordering::Relaxed);
        if p == 0 {
            return None;
        }
        // SAFETY: fn pointer stored via start()
        Some(unsafe { core::mem::transmute::<usize, RpcTransportFn>(p) })
    }

    // -----------------------------------------------------------------------
    // call()
    // -----------------------------------------------------------------------

    /// Performs a synchronous RPC call.
    ///
    /// - `server_ep`: target server endpoint
    /// - `method_id`: method to invoke
    /// - `args`: call payload (bytes)
    /// - `timeout`: timeout policy
    ///
    /// Returns `RpcResult` or `Err(IpcError)`.
    pub fn call(
        &self,
        server_ep: EndpointId,
        method_id: MethodId,
        args: &[u8],
        timeout: RpcTimeout,
    ) -> Result<RpcResult, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        let transport = self.transport_fn().ok_or(IpcError::Internal)?;
        // SAFETY: client_ep is always non-null after start() — loads a value
        //         stored via ep.0.get() in start(), thus never 0 on active client.
        let ep_raw = self.client_ep.load(Ordering::Relaxed);
        if ep_raw == 0 {
            return Err(IpcError::InvalidEndpoint);
        }
        let client_ep = EndpointId(unsafe { NonZeroU64::new_unchecked(ep_raw) });
        let pid = ProcessId(self.pid.load(Ordering::Relaxed));
        let cookie = self.next_cookie();

        self.total_calls.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::RpcCall);

        let mut retry = RetryState::new(1, timeout);

        loop {
            // --- Serialize call ---
            let plen = args.len().min(MAX_RPC_PAYLOAD);
            let header = RpcHeader::new_call(
                method_id,
                cookie,
                client_ep,
                server_ep,
                pid,
                plen as u32,
                retry.timeout.remaining_ns(),
            );

            let mut call_frame = RpcCallFrame::empty();
            call_frame.header = header;
            if plen > 0 {
                call_frame.payload[..plen].copy_from_slice(&args[..plen]);
            }

            let mut call_buf = [0u8; RPC_CALL_BUF_SIZE];
            let call_size = call_frame
                .serialize(&mut call_buf)
                .map_err(|_| IpcError::Invalid)?;

            // --- Send + receive ---
            let mut reply_buf = [0u8; RPC_CALL_BUF_SIZE];
            match transport(server_ep, client_ep, &call_buf[..call_size], &mut reply_buf) {
                Ok(reply_size) if reply_size > 0 => {
                    // Deserialize reply
                    match RpcReplyFrame::deserialize(&reply_buf[..reply_size]) {
                        Ok(reply) => {
                            if reply.header.cookie != cookie {
                                // Bad cookie — stale reply
                                continue;
                            }

                            let status = reply.header.reply_status();
                            if !status.is_ok() {
                                IPC_STATS.record(StatEvent::RpcProtocolError);
                                self.total_errors.fetch_add(1, Ordering::Relaxed);

                                if status.is_retryable() && retry.should_retry() {
                                    let wait_ns = retry.next_attempt()?;
                                    RetryState::spin_wait_ns(wait_ns);
                                    self.total_retries_used.fetch_add(1, Ordering::Relaxed);
                                    RPC_TIMEOUT_STATS.record_retry();
                                    continue;
                                }
                                return Err(status.to_ipc_error());
                            }

                            let plen = reply.header.payload_len as usize;
                            let mut result = RpcResult::empty();
                            result.status = status;
                            result.cookie = reply.header.cookie;
                            result.payload_len = plen.min(MAX_RPC_PAYLOAD);
                            if result.payload_len > 0 {
                                result.payload[..result.payload_len]
                                    .copy_from_slice(&reply.payload[..result.payload_len]);
                            }

                            IPC_STATS.record(StatEvent::RpcReturn);
                            if retry.attempts() > 0 {
                                RPC_TIMEOUT_STATS.record_success_after_retry();
                            }
                            return Ok(result);
                        }
                        Err(_) => {
                            self.total_errors.fetch_add(1, Ordering::Relaxed);
                            IPC_STATS.record(StatEvent::RpcProtocolError);
                        }
                    }
                }
                Ok(_) => {} // empty reply — retry
                Err(IpcError::Timeout) => {
                    if retry.should_retry() {
                        let wait_ns = retry.next_attempt()?;
                        RetryState::spin_wait_ns(wait_ns);
                        self.total_retries_used.fetch_add(1, Ordering::Relaxed);
                        RPC_TIMEOUT_STATS.record_retry();
                        continue;
                    }
                    self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                    RPC_TIMEOUT_STATS.record_timeout();
                    IPC_STATS.record(StatEvent::RpcTimeout);
                    return Err(IpcError::Timeout);
                }
                Err(e) => {
                    self.total_errors.fetch_add(1, Ordering::Relaxed);
                    return Err(e);
                }
            }

            // Timeout?
            if retry.timeout.is_expired() {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::RpcTimeout);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// RPC call with default timeout.
    pub fn call_default(
        &self,
        server_ep: EndpointId,
        method_id: MethodId,
        args: &[u8],
    ) -> Result<RpcResult, IpcError> {
        self.call(server_ep, method_id, args, RpcTimeout::default())
    }

    /// RPC call with retry (max retries = RPC_MAX_RETRIES).
    pub fn call_with_retry(
        &self,
        server_ep: EndpointId,
        method_id: MethodId,
        args: &[u8],
        max_retries: u32,
        timeout: RpcTimeout,
    ) -> Result<RpcResult, IpcError> {
        let mut retry = RetryState::new(max_retries, timeout);

        loop {
            match self.call(
                server_ep,
                method_id,
                args,
                RpcTimeout::with_ns(retry.timeout.remaining_ns()),
            ) {
                Ok(r) => return Ok(r),
                Err(IpcError::Timeout) | Err(IpcError::WouldBlock) => {
                    if retry.should_retry() {
                        let wait_ns = retry.next_attempt()?;
                        RetryState::spin_wait_ns(wait_ns);
                        RPC_TIMEOUT_STATS.record_retry();
                        continue;
                    }
                    return Err(IpcError::Timeout);
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn snapshot(&self) -> RpcClientStats {
        let ep_raw = self.client_ep.load(Ordering::Relaxed);
        // SAFETY: ep_raw is 0 only prior to start(); a sentinel is used in this case.
        let client_ep = if ep_raw != 0 {
            EndpointId(unsafe { NonZeroU64::new_unchecked(ep_raw) })
        } else {
            EndpointId(unsafe { NonZeroU64::new_unchecked(u64::MAX) }) // "invalid" sentinel
        };
        RpcClientStats {
            id: self.id,
            client_ep,
            total_calls: self.total_calls.load(Ordering::Relaxed),
            total_errors: self.total_errors.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
            total_retries_used: self.total_retries_used.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RpcClientStats {
    pub id: u32,
    pub client_ep: EndpointId,
    pub total_calls: u64,
    pub total_errors: u64,
    pub total_timeouts: u64,
    pub total_retries_used: u64,
}

// ---------------------------------------------------------------------------
// Global RPC client table
// ---------------------------------------------------------------------------

struct RpcClientSlot {
    client: MaybeUninit<RpcClient>,
    occupied: AtomicBool,
}

impl RpcClientSlot {
    const fn empty() -> Self {
        Self {
            client: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct RpcClientTable {
    slots: [RpcClientSlot; MAX_RPC_CLIENTS],
    count: AtomicU32,
}

unsafe impl Sync for RpcClientTable {}

impl RpcClientTable {
    const fn new() -> Self {
        Self {
            slots: [const { RpcClientSlot::empty() }; MAX_RPC_CLIENTS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32) -> Option<usize> {
        for i in 0..MAX_RPC_CLIENTS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: CAS AcqRel guarantees exclusivity; client MaybeUninit<RpcClient> write-once.
                    unsafe {
                        (self.slots[i].client.as_ptr() as *mut RpcClient).write(RpcClient::new(id));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&RpcClient> {
        if idx >= MAX_RPC_CLIENTS {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { &*self.slots[idx].client.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_RPC_CLIENTS {
            return false;
        }
        if let Some(c) = self.get(idx) {
            c.stop();
        }
        if self.slots[idx]
            .occupied
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static RPC_CLIENT_TABLE: RpcClientTable = RpcClientTable::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Creates an RPC client.
pub fn rpc_client_create(
    ep: EndpointId,
    pid: ProcessId,
    transport: RpcTransportFn,
) -> Option<usize> {
    let idx = RPC_CLIENT_TABLE.alloc(ep.0.get() as u32)?;
    RPC_CLIENT_TABLE.get(idx)?.start(ep, pid, transport);
    Some(idx)
}

/// RPC call from client `idx`.
pub fn rpc_call(
    idx: usize,
    server_ep: EndpointId,
    method_id: MethodId,
    args: &[u8],
    timeout_ns: u64,
) -> Result<RpcResult, IpcError> {
    let timeout = if timeout_ns == 0 {
        RpcTimeout::default()
    } else {
        RpcTimeout::with_ns(timeout_ns)
    };
    RPC_CLIENT_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .call(server_ep, method_id, args, timeout)
}

/// RPC call with retry.
pub fn rpc_call_retry(
    idx: usize,
    server_ep: EndpointId,
    method_id: MethodId,
    args: &[u8],
    max_retries: u32,
    timeout_ns: u64,
) -> Result<RpcResult, IpcError> {
    let timeout = RpcTimeout::with_ns(timeout_ns);
    RPC_CLIENT_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .call_with_retry(server_ep, method_id, args, max_retries, timeout)
}

/// Destroys an RPC client.
pub fn rpc_client_destroy(idx: usize) -> bool {
    RPC_CLIENT_TABLE.free(idx)
}

/// Client statistics.
pub fn rpc_client_stats(idx: usize) -> Option<RpcClientStats> {
    RPC_CLIENT_TABLE.get(idx).map(|c| c.snapshot())
}
