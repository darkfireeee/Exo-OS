// kernel/src/ipc/endpoint/lifecycle.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// LIFECYCLE — Creation, destruction, and cleanup of endpoints
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Manages the complete lifecycle of an endpoint:
//   CREATE → LISTEN → [accept/connect cycles...] → DRAIN → CLOSE → DESTROY
//
// CLEANUP:
//   Upon closing, all in-progress connections receive a
//   CHANNEL_CLOSED message before the endpoint is marked Destroyed.
//   Threads waiting in backlog receive ConnRefused.
//
// RULE: endpoint is never released while active_conns > 0.
//       A drain period is mandatory.
// ═══════════════════════════════════════════════════════════════════════════════

use super::descriptor::{EndpointDesc, EndpointName, EndpointState};
use super::registry::{register_endpoint, unregister_endpoint};
use crate::ipc::core::constants::MAX_ENDPOINTS;
use crate::ipc::core::types::{alloc_endpoint_id, EndpointId, IpcError};
use crate::scheduler::core::task::ThreadId;
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// EndpointPool — static pool of EndpointDesc
// ─────────────────────────────────────────────────────────────────────────────

/// State of a slot in the pool.
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
enum SlotState {
    Free = 0,
    InUse = 1,
}

/// Static pool of endpoints.
///
/// Allocates MAX_ENDPOINTS endpoint descriptors without heap.
/// Allocation is protected by a SpinLock on the bitmap.
struct EndpointPool {
    /// Bitmap of free / occupied slots (MAX_ENDPOINTS / 64 words).
    bitmap: [AtomicU64; MAX_ENDPOINTS / 64],
    /// Number of active endpoints.
    active: AtomicU64,
}

impl EndpointPool {
    const fn new() -> Self {
        Self {
            bitmap: [const { AtomicU64::new(0) }; MAX_ENDPOINTS / 64],
            active: AtomicU64::new(0),
        }
    }

    /// Allocates a free slot. Returns index or None if pool is exhausted.
    fn alloc(&self) -> Option<usize> {
        for (word_idx, word) in self.bitmap.iter().enumerate() {
            let bits = word.load(Ordering::Relaxed);
            if bits == u64::MAX {
                continue; // all occupied
            }
            // Find the first bit set to 0.
            let bit = (!bits).trailing_zeros() as usize;
            let mask = 1u64 << bit;
            // CAS to reserve slot.
            if word
                .compare_exchange_weak(bits, bits | mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.active.fetch_add(1, Ordering::Relaxed);
                return Some(word_idx * 64 + bit);
            }
        }
        None
    }

    /// Releases a slot.
    fn free(&self, idx: usize) {
        debug_assert!(idx < MAX_ENDPOINTS, "idx hors bornes");
        let word = idx / 64;
        let bit = idx % 64;
        let mask = 1u64 << bit;
        self.bitmap[word].fetch_and(!mask, Ordering::Release);
        self.active.fetch_sub(1, Ordering::Relaxed);
    }

    fn active_count(&self) -> u64 {
        self.active.load(Ordering::Relaxed)
    }
}

// Global pool.
static EP_POOL_BITMAP: EndpointPool = EndpointPool::new();

// Descriptor table — accessed via index obtained from EndpointPool.
// SAFETY: access protected by bitmap protocol + SpinLock per descriptor.
static EP_POOL_DESCS: SpinLock<[Option<EndpointDescBox>; MAX_ENDPOINTS]> =
    SpinLock::new([const { None }; MAX_ENDPOINTS]);

/// Newtype wrapper to make EndpointDesc statically Option-able.
struct EndpointDescBox(core::mem::MaybeUninit<EndpointDesc>);

impl EndpointDescBox {
    fn init(desc: EndpointDesc) -> Self {
        Self(core::mem::MaybeUninit::new(desc))
    }
    /// # Safety: called only when slot is InUse.
    unsafe fn as_ref(&self) -> &EndpointDesc {
        self.0.assume_init_ref()
    }
    /// # Safety: called only when slot is InUse.
    #[allow(dead_code)]
    unsafe fn as_mut(&mut self) -> &mut EndpointDesc {
        self.0.assume_init_mut()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public lifecycle API
// ─────────────────────────────────────────────────────────────────────────────

/// Creates and registers a new endpoint.
///
/// # Arguments
/// - `name`  : ASCII name of endpoint (max 64 chars).
/// - `owner` : initial owning thread.
///
/// # Return
/// `Ok(EndpointId)` — unique identifier of created endpoint.
pub fn endpoint_create(name: &[u8], owner: ThreadId) -> Result<EndpointId, IpcError> {
    let ep_name = EndpointName::from_bytes(name)?;
    let ep_id = alloc_endpoint_id();

    let idx = EP_POOL_BITMAP.alloc().ok_or(IpcError::ResourceExhausted)?;
    let desc = EndpointDesc::new(ep_id, ep_name, owner);

    {
        let mut pool = EP_POOL_DESCS.lock();
        pool[idx] = Some(EndpointDescBox::init(desc));
    }

    // Registration in name -> id registry.
    if let Err(e) = register_endpoint(name, ep_id) {
        // Rollback: free the slot.
        {
            let mut pool = EP_POOL_DESCS.lock();
            pool[idx] = None;
        }
        EP_POOL_BITMAP.free(idx);
        return Err(e);
    }

    Ok(ep_id)
}

/// Puts endpoint into listening mode (transition Created -> Listening).
pub fn endpoint_listen(ep_id: EndpointId) -> Result<(), IpcError> {
    with_endpoint(ep_id, |desc| desc.start_listen())
}

/// Initiates endpoint shutdown (transition -> Draining).
///
/// Existing connections will continue until their natural closure.
/// New connections are refused.
pub fn endpoint_close(ep_id: EndpointId) -> Result<(), IpcError> {
    with_endpoint(ep_id, |desc| {
        desc.state
            .store(EndpointState::Draining as u32, Ordering::Release);
        Ok(())
    })
}

/// Destroys an endpoint (only if active_conns == 0).
pub fn endpoint_destroy(name: &[u8], ep_id: EndpointId) -> Result<(), IpcError> {
    let mut pool = EP_POOL_DESCS.lock();
    let slot_idx = pool.iter().position(|slot| {
        if let Some(ref b) = slot {
            // SAFETY: accessed under lock, slot is Some.
            unsafe { b.as_ref().id == ep_id }
        } else {
            false
        }
    });

    let idx = match slot_idx {
        Some(i) => i,
        None => return Err(IpcError::EndpointNotFound),
    };

    let conns = pool[idx]
        .as_ref()
        .map(|desc| {
            // SAFETY: slot is Some and protected by EP_POOL_DESCS.
            unsafe { desc.as_ref().active_conns.load(Ordering::Acquire) }
        })
        .unwrap_or(u32::MAX);

    if conns != 0 {
        return Err(IpcError::WouldBlock);
    }

    pool[idx] = None;
    drop(pool); // release lock before modifying bitmap and registry
    EP_POOL_BITMAP.free(idx);
    unregister_endpoint(name);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper: endpoint access by id
// ─────────────────────────────────────────────────────────────────────────────

/// Executes a closure on the descriptor of an endpoint identified by its id.
/// Returns `Err(EndpointNotFound)` if id is unknown.
fn with_endpoint<F, R>(ep_id: EndpointId, f: F) -> Result<R, IpcError>
where
    F: FnOnce(&EndpointDesc) -> Result<R, IpcError>,
{
    let pool = EP_POOL_DESCS.lock();
    for ref b in pool.iter().flatten() {
        // SAFETY: accessed under lock.
        let desc = unsafe { b.as_ref() };
        if desc.id == ep_id {
            return f(desc);
        }
    }
    Err(IpcError::EndpointNotFound)
}

/// Returns number of active endpoints in pool.
pub fn active_endpoint_count() -> u64 {
    EP_POOL_BITMAP.active_count()
}
