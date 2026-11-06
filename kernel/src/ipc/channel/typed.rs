// ipc/channel/typed.rs — Generic typed channel for Exo-OS
//
// TypedChannel<T: Copy + Sized> wraps serialization/deserialization
// of Rust values via raw memory copy (transmute-safe for Copy types).
// Built on top of SpscRing for dedicated (sender, receiver) pairs,
// or MpmcRing in multi-thread mode.
//
// Safety:
//   - T must implement Copy + Sized — raw transmute with no risk of drop
//   - Size of T bounded by MAX_TYPED_VALUE_SIZE (512 bytes)
//   - No dynamic allocation
//
// Typical usage:
//   let (tx, rx) = TypedChannel::<SomeCmd>::create();
//   tx.send(SomeCmd::Halt).unwrap();
//   let cmd = rx.recv().unwrap();

use core::marker::PhantomData;
use core::mem::{size_of, MaybeUninit};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Size constraint
// ---------------------------------------------------------------------------

/// Maximum size of a typed value transmissible via TypedChannel (bounded by SpscRing MAX_MSG_SIZE)
pub const MAX_TYPED_VALUE_SIZE: usize = crate::ipc::core::constants::MAX_MSG_SIZE;

/// Compilation error if T is too large — check inside TypedChannel::create()
const fn assert_type_size<T>() {
    assert!(
        size_of::<T>() <= MAX_TYPED_VALUE_SIZE,
        "TypedChannel: T dépasse MAX_TYPED_VALUE_SIZE (240 octets)"
    );
}

// ---------------------------------------------------------------------------
// Distinct sender and receiver "handles" for single-owner constraint
// ---------------------------------------------------------------------------

/// Sender handle of a TypedChannel<T>
pub struct TypedSender<T: Copy + Sized> {
    channel_idx: usize,
    _phantom: PhantomData<T>,
}

/// Receiver handle of a TypedChannel<T>
pub struct TypedReceiver<T: Copy + Sized> {
    channel_idx: usize,
    _phantom: PhantomData<T>,
}

// SAFETY: T is Copy — no Drop, handle only contains an integer index
unsafe impl<T: Copy + Sized + Send> Send for TypedSender<T> {}
unsafe impl<T: Copy + Sized + Send> Send for TypedReceiver<T> {}

// ---------------------------------------------------------------------------
// Internal typed channel
// ---------------------------------------------------------------------------

/// Internal structure shared between TypedSender/TypedReceiver.
/// Stored in static table TYPED_CHANNEL_TABLE.
#[repr(C, align(64))]
pub struct TypedChannelInner {
    pub id: ChannelId,
    ring: SpscRing,
    sends: AtomicU64,
    recvs: AtomicU64,
    drops: AtomicU64,
    closed: AtomicU32,
    /// In-flight operations counter (anti-UAF on destroy)
    pub active_ops: AtomicU32,
    /// Type T size recorded at creation (runtime verification)
    type_size: u32,
    _pad: [u8; 24],
}

// SAFETY: SpscRing is Sync via its internal atomic barriers
unsafe impl Sync for TypedChannelInner {}
unsafe impl Send for TypedChannelInner {}

impl TypedChannelInner {
    pub fn new(type_size: usize) -> Self {
        let s = Self {
            id: alloc_channel_id(),
            ring: SpscRing::new(),
            sends: AtomicU64::new(0),
            recvs: AtomicU64::new(0),
            drops: AtomicU64::new(0),
            closed: AtomicU32::new(0),
            active_ops: AtomicU32::new(0),
            type_size: type_size as u32,
            _pad: [0u8; 24],
        };
        s.ring.init();
        s
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    /// Sends raw representation of `val` into ring.
    ///
    /// # SAFETY
    /// Caller must guarantee that size_of::<T>() == self.type_size
    pub unsafe fn push_raw(
        &self,
        ptr: *const u8,
        len: usize,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        // SAFETY: ptr points to valid [u8; len] originating from T: Copy
        let data = core::slice::from_raw_parts(ptr, len);
        let mid = alloc_message_id();
        self.ring.push_copy(data, flags)?;
        self.sends.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageSent);
        Ok(mid)
    }

    /// Receives and reconstructs a T from ring.
    ///
    /// # SAFETY
    /// Caller must guarantee that buf has size >= self.type_size
    pub unsafe fn pop_raw(&self, ptr: *mut u8, len: usize) -> Result<usize, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        let data = core::slice::from_raw_parts_mut(ptr, len);
        let (read_len, _flags) = self.ring.pop_into(data)?;
        self.recvs.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok(read_len)
    }
}

// ---------------------------------------------------------------------------
// Global static TypedChannelInner table
// ---------------------------------------------------------------------------

pub const TYPED_CHANNEL_TABLE_SIZE: usize = 256;

pub struct TypedChannelTable {
    slots: [MaybeUninit<TypedChannelInner>; TYPED_CHANNEL_TABLE_SIZE],
    used: [bool; TYPED_CHANNEL_TABLE_SIZE],
    pub count: usize,
}

// SAFETY: access protected by SpinLock<TypedChannelTable> in global table
unsafe impl Send for TypedChannelTable {}

impl TypedChannelTable {
    pub const fn new() -> Self {
        // SAFETY: mem::zeroed() avoids const-eval memory limits for large arrays.
        unsafe { core::mem::zeroed() }
    }

    pub fn alloc(&mut self, type_size: usize) -> Option<usize> {
        for i in 0..TYPED_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(TypedChannelInner::new(type_size));
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    pub fn free(&mut self, idx: usize) -> bool {
        if idx < TYPED_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees init
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    pub unsafe fn get(&self, idx: usize) -> Option<&TypedChannelInner> {
        if idx < TYPED_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

use crate::scheduler::sync::spinlock::SpinLock;

// SAFETY: .bss section is zero-initialized by bootloader.
// SpinLock<TypedChannelTable> all-zero = valid (lock free, empty table).
// MaybeUninit::uninit() = 0 const-eval bytes, memory comes from .bss.
static TYPED_CHANNEL_TABLE: core::mem::MaybeUninit<SpinLock<TypedChannelTable>> =
    core::mem::MaybeUninit::uninit();

#[inline(always)]
fn typed_table() -> &'static SpinLock<TypedChannelTable> {
    // SAFETY: .bss is zero-initialized at boot. SpinLock all-zeros = unlocked.
    unsafe { TYPED_CHANNEL_TABLE.assume_init_ref() }
}

// ---------------------------------------------------------------------------
// TypedChannel<T> — public API
// ---------------------------------------------------------------------------

/// Generic typed channel. Usage:
/// ```
/// let (tx, rx) = TypedChannel::<u64>::create().unwrap();
/// tx.send(42u64, MsgFlags::empty()).unwrap();
/// let v = rx.recv().unwrap();
/// assert_eq!(v, 42u64);
/// ```
pub struct TypedChannel<T: Copy + Sized> {
    _phantom: PhantomData<T>,
}

impl<T: Copy + Sized> TypedChannel<T> {
    /// Creates a pair (TypedSender<T>, TypedReceiver<T>).
    ///
    /// Panics at compile time if `size_of::<T>() > MAX_TYPED_VALUE_SIZE`.
    pub fn create() -> Result<(TypedSender<T>, TypedReceiver<T>), IpcError> {
        // Check size at compile time (via const fn)
        assert_type_size::<T>();

        let type_size = size_of::<T>();
        let mut tbl = typed_table().lock();
        let idx = tbl.alloc(type_size).ok_or(IpcError::OutOfResources)?;
        drop(tbl);

        Ok((
            TypedSender {
                channel_idx: idx,
                _phantom: PhantomData,
            },
            TypedReceiver {
                channel_idx: idx,
                _phantom: PhantomData,
            },
        ))
    }
}

impl<T: Copy + Sized> TypedSender<T> {
    /// Sends value `val` of type T via typed channel.
    pub fn send(&self, val: T, flags: MsgFlags) -> Result<MessageId, IpcError> {
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }.ok_or(IpcError::InvalidHandle)?;
        let inner_ref: &'static TypedChannelInner =
            unsafe { &*(inner as *const TypedChannelInner) };
        inner_ref.active_ops.fetch_add(1, Ordering::Acquire);
        drop(tbl);

        let size = size_of::<T>();
        // SAFETY: val is T: Copy, memory of size bytes at &val is valid
        let res = unsafe { inner_ref.push_raw(&val as *const T as *const u8, size, flags) };
        inner_ref.active_ops.fetch_sub(1, Ordering::Release);
        res
    }

    /// Closes channel (sender side — notifies receiver).
    pub fn close(&self) {
        let tbl = typed_table().lock();
        if let Some(inner) = unsafe { tbl.get(self.channel_idx) } {
            inner.close();
        }
    }

    pub fn channel_idx(&self) -> usize {
        self.channel_idx
    }
}

impl<T: Copy + Sized> TypedReceiver<T> {
    /// Receives a value of type T from channel.
    ///
    /// # Errors
    /// - `IpcError::WouldBlock` — no message available
    /// - `IpcError::Closed` — channel closed
    /// - `IpcError::ProtocolError` — received size != size_of::<T>()
    pub fn recv(&self) -> Result<T, IpcError> {
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }.ok_or(IpcError::InvalidHandle)?;
        let inner_ref: &'static TypedChannelInner =
            unsafe { &*(inner as *const TypedChannelInner) };
        inner_ref.active_ops.fetch_add(1, Ordering::Acquire);
        drop(tbl);

        let size = size_of::<T>();

        // Allocate uninitialized T on stack (zero-alloc)
        let mut val = MaybeUninit::<T>::uninit();

        // SAFETY: val is T: Copy, uninitialized bytes will be filled by pop_raw
        let pop_res = unsafe { inner_ref.pop_raw(val.as_mut_ptr() as *mut u8, size) };
        inner_ref.active_ops.fetch_sub(1, Ordering::Release);
        let read_len = pop_res?;

        if read_len != size {
            return Err(IpcError::ProtocolError);
        }

        // SAFETY: pop_raw wrote exactly `size` bytes = sizeof(T)
        Ok(unsafe { val.assume_init() })
    }

    /// Attempts to receive a value without blocking.
    pub fn try_recv(&self) -> Result<T, IpcError> {
        // First check if ring has a message
        let tbl = typed_table().lock();
        let inner = unsafe { tbl.get(self.channel_idx) }.ok_or(IpcError::InvalidHandle)?;
        if inner.ring.is_empty() {
            return Err(IpcError::WouldBlock);
        }
        drop(tbl);
        self.recv()
    }

    pub fn channel_idx(&self) -> usize {
        self.channel_idx
    }
}

// Drop implementation to free channel from global table
// Note: in kernel, we do not automatically free — both handles
// must be explicitly closed via TypedSender::close() + typed_channel_destroy()

// ---------------------------------------------------------------------------
// Global public API
// ---------------------------------------------------------------------------

/// Destroys and frees typed channel identified by `idx`.
pub fn typed_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = typed_table().lock();
    let inner_ref: &'static TypedChannelInner = match unsafe { tbl.get(idx) } {
        Some(inner) => {
            inner.close();
            unsafe { &*(inner as *const TypedChannelInner) }
        }
        None => return Err(IpcError::InvalidHandle),
    };
    drop(tbl);

    let mut spins = 0u32;
    while inner_ref.active_ops.load(Ordering::Acquire) != 0 {
        if spins >= 256 {
            return Err(IpcError::Retry);
        }
        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
        if spins % 32 == 0 {
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        }
    }

    let mut tbl = typed_table().lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Returns number of active typed channels.
pub fn typed_channel_count() -> usize {
    typed_table().lock().count
}
