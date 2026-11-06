// ipc/message/router.rs — Multi-hop IPC routing table for Exo-OS
//
// This module implements an IPC message router: each EndpointId can
// be associated with a next-hop (another EndpointId) in a static table.
// The router enables transparent forwarding of messages between endpoints
// without explicit knowledge of the final recipient.
//
// Architecture:
//   - `RoutingTable`: static table of MAX_ROUTES entries (EndpointId src -> dst)
//   - `IpcRouter`: encapsulates table with spinlock read/write lock
//   - `route()`: resolves next-hop in O(1) if compact table, O(N) otherwise
//   - `forward()`: routes + returns modified message descriptor (dst replaced)
//   - Multi-hop route support (max MAX_HOPS before declaring a loop)
//
// RULE ROUTE-01: static table, no allocation.
// RULE ROUTE-02: MAX_HOPS = 8 to prevent infinite loops.
// RULE ROUTE-03: circular routes are detected and return Err(Loop).

use core::num::NonZeroU64;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::ipc::message::builder::IpcMessage;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of routes in table
pub const MAX_ROUTES: usize = 512;

/// Maximum number of hops before detecting a loop
pub const MAX_HOPS: usize = 8;

// ---------------------------------------------------------------------------
// Route entry
// ---------------------------------------------------------------------------

/// An entry in the routing table: src_id -> next_hop + metadata.
#[repr(C, align(32))]
pub struct RouteEntry {
    /// Source endpoint (lookup key)
    pub src: AtomicU32,
    /// Next-hop (destination EndpointId)
    pub next_hop: AtomicU32,
    /// Route cost (for path selection if multiple routes to same src)
    pub cost: AtomicU32,
    /// Entry is valid
    pub valid: AtomicBool,
    /// Forwarding statistics
    pub fwd_count: AtomicU64,
    _pad: [u8; 3],
}

// SAFETY: all fields are atomic
unsafe impl Sync for RouteEntry {}
unsafe impl Send for RouteEntry {}

impl RouteEntry {
    pub const fn empty() -> Self {
        Self {
            src: AtomicU32::new(0),
            next_hop: AtomicU32::new(0),
            cost: AtomicU32::new(0),
            valid: AtomicBool::new(false),
            fwd_count: AtomicU64::new(0),
            _pad: [0u8; 3],
        }
    }

    pub fn is_match(&self, ep_id: u32) -> bool {
        self.valid.load(Ordering::Relaxed) && self.src.load(Ordering::Relaxed) == ep_id
    }
}

// ---------------------------------------------------------------------------
// RoutingTable — static table
// ---------------------------------------------------------------------------

/// Static IPC routing table.
///
/// Linear lookup O(N) — N <= MAX_ROUTES = 512.
/// For a kernel with few endpoints, this is optimal (cache-friendly).
struct RoutingTable {
    entries: [RouteEntry; MAX_ROUTES],
    count: AtomicU32,
}

// SAFETY: RouteEntry is Sync
unsafe impl Sync for RoutingTable {}

impl RoutingTable {
    const fn new() -> Self {
        Self {
            entries: [const { RouteEntry::empty() }; MAX_ROUTES],
            count: AtomicU32::new(0),
        }
    }

    /// Adds or replaces a route src -> next_hop.
    fn add(&self, src: u32, next_hop: u32, cost: u32) -> Result<(), IpcError> {
        // Search for an existing entry for src
        for i in 0..MAX_ROUTES {
            if self.entries[i].valid.load(Ordering::Relaxed)
                && self.entries[i].src.load(Ordering::Relaxed) == src
            {
                // Update
                self.entries[i].next_hop.store(next_hop, Ordering::Relaxed);
                self.entries[i].cost.store(cost, Ordering::Relaxed);
                return Ok(());
            }
        }

        // Search for a free slot
        for i in 0..MAX_ROUTES {
            if !self.entries[i].valid.load(Ordering::Relaxed) {
                self.entries[i].src.store(src, Ordering::Relaxed);
                self.entries[i].next_hop.store(next_hop, Ordering::Relaxed);
                self.entries[i].cost.store(cost, Ordering::Relaxed);
                self.entries[i].valid.store(true, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
        }

        Err(IpcError::NullEndpoint)
    }

    /// Removes route for `src`.
    fn remove(&self, src: u32) -> bool {
        for i in 0..MAX_ROUTES {
            if self.entries[i].is_match(src) {
                self.entries[i].valid.store(false, Ordering::Release);
                self.count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Resolves next-hop for `src`. Returns None if no route.
    fn lookup(&self, src: u32) -> Option<u32> {
        for i in 0..MAX_ROUTES {
            if self.entries[i].is_match(src) {
                self.entries[i].fwd_count.fetch_add(1, Ordering::Relaxed);
                return Some(self.entries[i].next_hop.load(Ordering::Relaxed));
            }
        }
        None
    }

    /// Resolves with loop detection (multi-hop).
    /// Returns the final EndpointId after MAX_HOPS hops max.
    fn resolve_final(&self, start: u32) -> Result<u32, IpcError> {
        let mut current = start;
        let mut hops = 0usize;

        loop {
            match self.lookup(current) {
                None => return Ok(current), // end of chain
                Some(next) => {
                    if next == start || next == current {
                        return Err(IpcError::Loop);
                    }
                    current = next;
                    hops += 1;
                    if hops >= MAX_HOPS {
                        return Err(IpcError::Loop);
                    }
                }
            }
        }
    }

    fn route_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// IpcRouter — main router
// ---------------------------------------------------------------------------

/// Main IPC router.
///
/// Concurrent access protected by SpinLock.
pub struct IpcRouter {
    table: SpinLock<RoutingTable>,
    /// Total forwarded messages
    pub forwarded: AtomicU64,
    /// Number of routes not found (misses)
    pub miss: AtomicU64,
    /// Number of loops detected
    pub loops_detected: AtomicU64,
}

// SAFETY: SpinLock<RoutingTable> is Sync
unsafe impl Sync for IpcRouter {}
unsafe impl Send for IpcRouter {}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            table: SpinLock::new(RoutingTable::new()),
            forwarded: AtomicU64::new(0),
            miss: AtomicU64::new(0),
            loops_detected: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Route management
    // -----------------------------------------------------------------------

    /// Adds a route src -> next_hop with an optional cost.
    pub fn add_route(
        &self,
        src: EndpointId,
        next_hop: EndpointId,
        cost: u32,
    ) -> Result<(), IpcError> {
        let guard = self.table.lock();
        guard.add(src.0.get() as u32, next_hop.0.get() as u32, cost)
    }

    /// Removes route for src.
    pub fn remove_route(&self, src: EndpointId) -> bool {
        let guard = self.table.lock();
        guard.remove(src.0.get() as u32)
    }

    /// Direct lookup: immediate next-hop for src.
    pub fn next_hop(&self, src: EndpointId) -> Option<EndpointId> {
        let guard = self.table.lock();
        guard
            .lookup(src.0.get() as u32)
            .and_then(|v| NonZeroU64::new(v as u64))
            .map(EndpointId)
    }

    /// Multi-hop final resolution: returns terminal endpoint.
    pub fn resolve(&self, src: EndpointId) -> Result<EndpointId, IpcError> {
        let guard = self.table.lock();
        match guard.resolve_final(src.0.get() as u32) {
            Ok(id) => Ok(NonZeroU64::new(id as u64)
                .map(EndpointId)
                .unwrap_or(EndpointId::INVALID)),
            Err(e) => {
                self.loops_detected.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    /// Active route count.
    pub fn route_count(&self) -> u32 {
        let guard = self.table.lock();
        guard.route_count()
    }

    // -----------------------------------------------------------------------
    // Message forwarding
    // -----------------------------------------------------------------------

    /// Routes a message: resolves final destination and updates msg.dst.
    ///
    /// If no route is found, msg.dst remains unchanged (direct delivery).
    pub fn route_message(&self, msg: &mut IpcMessage) -> Result<(), IpcError> {
        let guard = self.table.lock();
        let dst_raw = msg.dst.0.get() as u32;
        match guard.resolve_final(dst_raw) {
            Ok(final_dst) => {
                if final_dst != dst_raw {
                    msg.dst = NonZeroU64::new(final_dst as u64)
                        .map(EndpointId)
                        .unwrap_or(EndpointId::INVALID);
                    self.forwarded.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                }
                Ok(())
            }
            Err(IpcError::Loop) => {
                self.loops_detected.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::GenericError);
                Err(IpcError::Loop)
            }
            Err(e) => Err(e),
        }
    }

    /// Checks if an endpoint is reachable (path exists).
    pub fn is_reachable(&self, dst: EndpointId) -> bool {
        // An endpoint is always reachable to itself.
        // Resolution succeeds if no loop exists.
        let guard = self.table.lock();
        guard.resolve_final(dst.0.get() as u32).is_ok()
    }

    /// Router statistics snapshot.
    pub fn snapshot(&self) -> IpcRouterStats {
        IpcRouterStats {
            forwarded: self.forwarded.load(Ordering::Relaxed),
            miss: self.miss.load(Ordering::Relaxed),
            loops_detected: self.loops_detected.load(Ordering::Relaxed),
            route_count: self.route_count(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcRouterStats {
    pub forwarded: u64,
    pub miss: u64,
    pub loops_detected: u64,
    pub route_count: u32,
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

/// Global IPC router.
///
/// All routing operations go through this instance.
pub static IPC_ROUTER: IpcRouter = IpcRouter::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Adds a route to global router.
pub fn router_add(src: EndpointId, next_hop: EndpointId) -> Result<(), IpcError> {
    IPC_ROUTER.add_route(src, next_hop, 1)
}

/// Removes a route from global router.
pub fn router_remove(src: EndpointId) -> bool {
    IPC_ROUTER.remove_route(src)
}

/// Resolves next-hop for an endpoint.
pub fn router_lookup(src: EndpointId) -> Option<EndpointId> {
    IPC_ROUTER.next_hop(src)
}

/// Resolves terminal endpoint (multi-hop).
pub fn router_resolve(src: EndpointId) -> Result<EndpointId, IpcError> {
    IPC_ROUTER.resolve(src)
}

/// Routes a message (modifies msg.dst if forwarding is needed).
pub fn router_dispatch(msg: &mut IpcMessage) -> Result<(), IpcError> {
    IPC_ROUTER.route_message(msg)
}

/// Global router statistics.
pub fn router_stats() -> IpcRouterStats {
    IPC_ROUTER.snapshot()
}
