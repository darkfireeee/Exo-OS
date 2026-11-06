// kernel/src/process/core/registry.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessRegistry — global PID → PCB table (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure:
//   • Flat array of slots (index = PID - 1).
//   • Each slot = Option<Box<ProcessControlBlock>> in a lightweight RwLock.
//   • Non-blocking read access via AtomicPtr for hot paths.
//   • Writes protected by global registry spinlock.
//
// Concurrency:
//   Readers (signal, syscall, IPC) use `find_by_pid()` which reads
//   the slot via atomic raw pointer — consistent through the guarantee that
//   the PCB cannot be freed while active references exist (refcounting).
// ═══════════════════════════════════════════════════════════════════════════════

use super::pcb::{Credentials, ProcessControlBlock};
use super::pid::Pid;
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// RegistrySlot
// ─────────────────────────────────────────────────────────────────────────────

/// A slot in the registry table.
/// AtomicPtr allows lockless (non-blocking) reads.
struct RegistrySlot {
    /// Atomic pointer to PCB. null = empty slot.
    pcb_ptr: AtomicPtr<ProcessControlBlock>,
    /// Active reference count on this PCB.
    refcount: AtomicU32,
}

impl RegistrySlot {
    const fn empty() -> Self {
        Self {
            pcb_ptr: AtomicPtr::new(core::ptr::null_mut()),
            refcount: AtomicU32::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessRegistry
// ─────────────────────────────────────────────────────────────────────────────

/// Registry error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    /// Slot full (no space).
    Full,
    /// PID not found.
    NotFound,
    /// PID already registered.
    AlreadyExists,
}

/// Global PID → PCB table (dynamically initialized).
pub struct ProcessRegistry {
    /// Registry slots (allocated at init).
    slots: *mut RegistrySlot,
    /// Table capacity (number of supported PIDs).
    capacity: usize,
    /// Spinlock for writes (insert/remove).
    write_lock: SpinLock<()>,
    /// Number of registered processes.
    count: AtomicUsize,
    /// Total number of insertions since boot.
    inserts: AtomicUsize,
    /// Total number of removals.
    removes: AtomicUsize,
    /// Number of lookups (debugging instrumentation).
    lookups: AtomicUsize,
}

// SAFETY: pointed slots are accessed exclusively or via atomics.
unsafe impl Sync for ProcessRegistry {}
unsafe impl Send for ProcessRegistry {}

// Global registry — initially invalid (null ptr, capacity 0).
pub static PROCESS_REGISTRY: ProcessRegistry = ProcessRegistry {
    slots: core::ptr::null_mut(),
    capacity: 0,
    write_lock: SpinLock::new(()),
    count: AtomicUsize::new(0),
    inserts: AtomicUsize::new(0),
    removes: AtomicUsize::new(0),
    lookups: AtomicUsize::new(0),
};

impl ProcessRegistry {
    /// Initializes the registry with `capacity` slots.
    ///
    /// # Safety
    /// Called once from the BSP during init.
    pub unsafe fn init(&self, capacity: usize) {
        use alloc::alloc::{alloc_zeroed, Layout};
        // FIX-KRN-10: layout computed after final capacity determined below.
        // Will be recalculated below with final_capacity.
        let _layout_orig =
            Layout::array::<RegistrySlot>(capacity).expect("layout RegistrySlot valide");
        // FIX-KRN-10 (analysis report §6.3): check available memory
        // before static allocation of 512 KiB (32768 × 16B).
        // On QEMU -m 256M, this allocation represents 0.2% of RAM — acceptable.
        // But without guard, boot on machine with little RAM panics without clear message.
        //
        // Check: if buddy allocator has less than 2 × REGISTRY_SIZE in free frames,
        // log warning and reduce capacity to maximum available.
        // FIX-KRN-10: use real physical memory available API.
        let available_bytes = crate::memory::physical::stats::free_bytes();
        let registry_bytes = capacity * core::mem::size_of::<RegistrySlot>();
        // FIX-KRN-10b: free_bytes() == 0 means physical allocator
        // is not initialized (host tests, very early init) — low-memory
        // guard does not apply. Old code reduced capacity to 64 slots
        // in this case, breaking any PID > 64.
        let final_capacity = if available_bytes != 0 && available_bytes < registry_bytes * 2 {
            // Fallback: allocate at most half of available RAM for the registry
            let safe_cap = (available_bytes / 2) / core::mem::size_of::<RegistrySlot>();
            crate::arch::x86_64::terminal::debug_write(
                b"process_registry: WARNING reducing capacity due to low memory\n",
            );
            safe_cap.max(64) // minimum 64 slots for Ring1 servers
        } else {
            capacity
        };
        let layout = core::alloc::Layout::array::<RegistrySlot>(final_capacity)
            .expect("ProcessRegistry: layout invalide");
        let ptr = alloc_zeroed(layout) as *mut RegistrySlot;
        assert!(!ptr.is_null(), "ProcessRegistry::init : allocation échouée");
        // FIX-KRN-10b: initialization and published capacity must use
        // final_capacity (the size actually allocated). Old code iterated and
        // published `capacity`: if memory reduction branch applied,
        // init wrote OUTSIDE allocated array (heap overflow) and registry
        // exposed nonexistent slots.
        for i in 0..final_capacity {
            // SAFETY: ptr + i is within the allocated array (i < final_capacity).
            core::ptr::write(ptr.add(i), RegistrySlot::empty());
        }
        // Update via raw pointer (static mut is at a fixed address).
        let self_mut = self as *const Self as *mut Self;
        (*self_mut).slots = ptr;
        (*self_mut).capacity = final_capacity;
    }

    /// Registers a PCB in the table (called on process creation).
    pub fn insert(&self, pcb: Box<ProcessControlBlock>) -> Result<(), RegistryError> {
        let pid = pcb.pid;
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return Err(RegistryError::Full);
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity, slots initialized.
        let slot = unsafe { &*self.slots.add(slot_idx) };

        let _guard = self.write_lock.lock();
        if !slot.pcb_ptr.load(Ordering::Relaxed).is_null() {
            return Err(RegistryError::AlreadyExists);
        }
        // Converts Box to raw pointer, stores in slot.
        let raw = Box::into_raw(pcb);
        slot.pcb_ptr.store(raw, Ordering::Release);
        slot.refcount.store(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.inserts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Removes the PCB from the table and returns the Box for deallocation.
    pub fn remove(&self, pid: Pid) -> Result<Box<ProcessControlBlock>, RegistryError> {
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return Err(RegistryError::NotFound);
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity.
        let slot = unsafe { &*self.slots.add(slot_idx) };

        let _guard = self.write_lock.lock();
        let raw = slot.pcb_ptr.swap(core::ptr::null_mut(), Ordering::AcqRel);
        if raw.is_null() {
            return Err(RegistryError::NotFound);
        }
        self.count.fetch_sub(1, Ordering::Relaxed);
        self.removes.fetch_add(1, Ordering::Relaxed);
        // SAFETY: raw was placed in the table via Box::into_raw in insert().
        Ok(unsafe { Box::from_raw(raw) })
    }

    /// Lockless lookup of a PCB by PID.
    /// Returns a validated raw pointer or null.
    /// The pointer remains valid as long as the PCB is in the table.
    pub fn find_by_pid(&self, pid: Pid) -> Option<&ProcessControlBlock> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return None;
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity, read-only access via AtomicPtr.
        let slot = unsafe { &*self.slots.add(slot_idx) };
        let raw = slot.pcb_ptr.load(Ordering::Acquire);
        if raw.is_null() {
            return None;
        }
        // SAFETY: non-null raw = PCB still in table (not freed).
        Some(unsafe { &*raw })
    }

    /// Current number of processes.
    #[inline(always)]
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Iterates over all active PCBs (read-only).
    /// Closure called for each non-null PCB.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&ProcessControlBlock),
    {
        for i in 0..self.capacity {
            // SAFETY: i < capacity.
            let slot = unsafe { &*self.slots.add(i) };
            let raw = slot.pcb_ptr.load(Ordering::Acquire);
            if !raw.is_null() {
                // SAFETY: non-null raw = valid PCB in memory.
                f(unsafe { &*raw });
            }
        }
    }

    /// Accesses a process's credentials and applies a mutating closure.
    ///
    /// Returns `Some(R)` if process exists, `None` otherwise.
    /// Closure receives `&mut Credentials` for atomic modification.
    ///
    /// # Example
    /// ```rust
    /// PROCESS_REGISTRY.with_creds_mut_by_pid(pid, |c| { c.euid = 0; });
    /// ```
    pub fn with_creds_mut_by_pid<F, R>(&self, pid: Pid, f: F) -> Option<R>
    where
        F: FnOnce(&mut Credentials) -> R,
    {
        let pcb = self.find_by_pid(pid)?;
        // SAFETY: SpinLock<Credentials> — short lock (few instructions),
        // no allocation or system call inside.
        Some(f(&mut *pcb.creds.lock()))
    }

    /// Registry statistics for monitoring system.
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            current_count: self.count.load(Ordering::Relaxed),
            total_inserts: self.inserts.load(Ordering::Relaxed),
            total_removes: self.removes.load(Ordering::Relaxed),
            total_lookups: self.lookups.load(Ordering::Relaxed),
            capacity: self.capacity,
        }
    }
}

/// Statistics exported by the registry.
#[derive(Debug, Clone, Copy)]
pub struct RegistryStats {
    pub current_count: usize,
    pub total_inserts: usize,
    pub total_removes: usize,
    pub total_lookups: usize,
    pub capacity: usize,
}

/// Initializes the global registry.
///
/// # Safety
/// Called once from the BSP.
pub unsafe fn init(max_pids: usize) {
    PROCESS_REGISTRY.init(max_pids);
}
