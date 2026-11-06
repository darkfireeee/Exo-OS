// kernel/src/process/core/tcb.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessThread — process/ extension of the scheduler's ThreadControlBlock
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   scheduler/core/task.rs defines `ThreadControlBlock` (256 bytes, hot path).
//   This file defines `ProcessThread` which WRAPS the scheduler TCB and adds
//   process-level data (stack address, TLS, signals...) without modifying
//   the 128 bytes of the scheduler TCB.
//
// RULES:
//   • ProcessThread is allocated by lifecycle/create.rs, freed by lifecycle/exit.rs.
//   • The `sched_tcb` pointer points to a TCB in static memory or heap.
//   • All accesses to sched_tcb from process/ are documented.
//   • PROC-04: signal_pending is WRITTEN here (process/signal/), READ by scheduler.
// ═══════════════════════════════════════════════════════════════════════════════

use super::pid::{Pid, Tid};
use crate::process::signal::queue::{RTSigQueue, SigQueue};
use crate::scheduler::core::task::{
    Priority, ProcessId, SchedPolicy, TaskState, ThreadControlBlock, ThreadId,
};
use alloc::alloc::{alloc, dealloc, Layout};
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};

fn try_box_new<T>(value: T) -> Option<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return None;
    }

    // SAFETY: `layout` matches T. A null allocation is converted to None, and
    // a successful allocation is immediately initialized and owned by Box.
    let raw = unsafe { alloc(layout) as *mut T };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is uniquely owned and valid for writes of T.
    unsafe {
        raw.write(value);
        Some(Box::from_raw(raw))
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
#[inline]
fn tcb_trace(message: &[u8]) {
    crate::arch::x86_64::terminal::debug_write(message);
}

#[cfg(not(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace)))]
#[inline]
fn tcb_trace(_message: &[u8]) {}

#[inline]
fn sync_thread_kernel_half(cr3: u64) {
    if cr3 == 0 {
        return;
    }
    unsafe {
        crate::memory::virt::address_space::KERNEL_AS
            .sync_kernel_half_into(crate::memory::core::PhysAddr::new(cr3));
    }
}

fn try_process_thread_box(
    sched_tcb: Box<ThreadControlBlock>,
    kernel_stack: KernelStack,
    pid: Pid,
    tid: Tid,
) -> Option<Box<ProcessThread>> {
    let layout = Layout::new::<ProcessThread>();
    let raw = unsafe { alloc(layout) as *mut ProcessThread };
    if raw.is_null() {
        return None;
    }

    unsafe {
        core::ptr::addr_of_mut!((*raw).sched_tcb).write(sched_tcb);
        core::ptr::addr_of_mut!((*raw).kernel_stack).write(kernel_stack);
        core::ptr::addr_of_mut!((*raw).pid).write(pid);
        core::ptr::addr_of_mut!((*raw).tid).write(tid);
        core::ptr::addr_of_mut!((*raw).addresses).write(ThreadAddress::default());
        core::ptr::addr_of_mut!((*raw).tls_gs_base).write(AtomicU64::new(0));
        core::ptr::addr_of_mut!((*raw).tls_block).write(AtomicUsize::new(0));
        core::ptr::addr_of_mut!((*raw).tls_size).write(0);
        core::ptr::addr_of_mut!((*raw).detached).write(AtomicBool::new(false));
        core::ptr::addr_of_mut!((*raw).join_done).write(AtomicBool::new(false));
        core::ptr::addr_of_mut!((*raw).join_result).write(AtomicU64::new(0));
        SigQueue::init_at(core::ptr::addr_of_mut!((*raw).sig_queue));
        RTSigQueue::init_at(core::ptr::addr_of_mut!((*raw).rt_sig_queue));
        Some(Box::from_raw(raw))
    }
}

/// Kernel stack size per thread.
///
/// Critical userspace paths (fork/exec/IPC) traverse enough kernel
/// Rust for 16 KiB to be fragile in debug. The `do_fork` path (CoW clone,
/// fds clone, namespaces, credentials, identity BLAKE3 hash) alone
/// consumes ~56–64 KiB in debug build: 64 KiB overflowed the guard page by ~360
/// bytes (kernel overflow → ExoPhoenix). 256 KiB keeps a comfortable margin
/// for nested forks (fork→execve) while remaining modest (vmalloc, 256
/// MiB RAM, a few dozen threads).
pub const KSTACK_SIZE: usize = 256 * 1024;
const KSTACK_PAGE_SIZE: usize = crate::memory::core::constants::PAGE_SIZE;
const KSTACK_USABLE_PAGES: usize = KSTACK_SIZE / KSTACK_PAGE_SIZE;
const KSTACK_TOTAL_PAGES: usize = KSTACK_USABLE_PAGES + 2;

const _: () = assert!(KSTACK_SIZE.is_multiple_of(KSTACK_PAGE_SIZE));

/// Stack canary to detect overflows.
const STACK_CANARY: u64 = 0xDEAD_BEEF_CAFE_BABE;

struct KernelStackPtAlloc;

impl crate::memory::virt::page_table::FrameAllocatorForWalk for KernelStackPtAlloc {
    fn alloc_frame(
        &self,
        flags: crate::memory::core::AllocFlags,
    ) -> Result<crate::memory::core::Frame, crate::memory::core::AllocError> {
        crate::memory::alloc_page(flags)
    }

    fn free_frame(&self, frame: crate::memory::core::Frame) {
        let _ = crate::memory::free_page(frame);
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum KernelStackBacking {
    Heap,
    GuardedVmalloc {
        frames: [crate::memory::core::Frame; KSTACK_USABLE_PAGES],
        guard_id: Option<crate::memory::integrity::GuardRegionId>,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// ThreadAddress — userspace addresses of a thread
// ─────────────────────────────────────────────────────────────────────────────

/// Addresses tied to the thread lifecycle on the userspace side.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct ThreadAddress {
    /// Base of allocated user stack (lowest address).
    pub stack_base: u64,
    /// User stack size (bytes).
    pub stack_size: u64,
    /// Return instruction register (initial RIP at startup).
    pub entry_point: u64,
    /// Initial user frame pointer (RSP at startup).
    pub initial_rsp: u64,
    /// Pointer to static TLS (FS.base for x86_64).
    pub tls_base: u64,
    /// First argument injected into RDI at initial entry point.
    pub entry_arg0: u64,
    /// Pointer to userspace `pthread_t` structure.
    pub pthread_ptr: u64,
    /// `sigaltstack` area (alternative stack for signals).
    pub sigaltstack_base: u64,
    pub sigaltstack_size: u64,
}

impl ThreadAddress {
    /// Address of top of sigaltstack (base + size).
    #[inline(always)]
    pub fn sigaltstack_top(&self) -> u64 {
        self.sigaltstack_base.saturating_add(self.sigaltstack_size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelStack — dedicated kernel stack of the thread
// ─────────────────────────────────────────────────────────────────────────────

/// Dynamically allocated kernel stack for a thread.
///
/// The normal path allocates a dedicated physical region:
/// `[guard !PRESENT][usable stack][guard !PRESENT]`. If the kernel is still too
/// early in boot to modify PML4, the heap backend retains the bottom canary.
pub struct KernelStack {
    /// Pointer to allocated memory (bottom of buffer).
    base: *mut u8,
    /// Total size in bytes.
    size: usize,
    /// Address of top (base + size, 16-aligned).
    top: u64,
    backing: KernelStackBacking,
}

impl KernelStack {
    /// Allocates a new kernel stack of `size` bytes.
    /// Places an 8-byte canary at the bottom and, if possible, two real guard pages.
    pub fn alloc(size: usize, tid: Tid) -> Option<Self> {
        Self::alloc_guarded(size, tid).or_else(|| Self::alloc_heap_canary(size))
    }

    fn alloc_heap_canary(size: usize) -> Option<Self> {
        if !crate::memory::heap::is_heap_ready() {
            return None;
        }
        let layout = Layout::from_size_align(size, 16).ok()?;
        // SAFETY: layout is valid, we check the pointer.
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            return None;
        }
        // Write canary at the bottom of the stack.
        // SAFETY: base points to `size` allocated bytes, canary at offset 0.
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }
        // SAFETY: base was allocated with `size` bytes; base.add(size) points just past the end,
        //         which is a valid pointer (sentinel, never dereferenced).
        let top = unsafe { base.add(size) } as u64;
        // Align to 16 bytes (x86_64 ABI): top must be 16-aligned - 8.
        let top_aligned = (top & !0xF) - 8;
        Some(Self {
            base,
            size,
            top: top_aligned,
            backing: KernelStackBacking::Heap,
        })
    }

    fn alloc_guarded(size: usize, tid: Tid) -> Option<Self> {
        use crate::memory::core::{AllocFlags, Frame, PageFlags, VirtAddr};

        tcb_trace(b"kstack: guarded enter\n");
        if size != KSTACK_SIZE
            || crate::memory::virt::address_space::KERNEL_AS
                .pml4_phys()
                .as_u64()
                == 0
        {
            tcb_trace(b"kstack: guarded unavailable\n");
            return None;
        }

        let mut frames = [Frame::NULL; KSTACK_USABLE_PAGES];
        for i in 0..KSTACK_USABLE_PAGES {
            frames[i] = match crate::memory::physical::alloc_page(AllocFlags::ZEROED) {
                Ok(frame) => frame,
                Err(_) => {
                    for allocated in frames.iter().copied().take(i) {
                        if allocated != Frame::NULL {
                            let _ = crate::memory::physical::free_page(allocated);
                        }
                    }
                    return None;
                }
            };
        }
        tcb_trace(b"kstack: frames\n");

        let reserved = match crate::memory::virt::address_space::KERNEL_AS
            .reserve_vmalloc_pages(KSTACK_TOTAL_PAGES)
        {
            Ok(start) => start,
            Err(_) => {
                for frame in frames {
                    let _ = crate::memory::physical::free_page(frame);
                }
                return None;
            }
        };
        tcb_trace(b"kstack: reserved\n");

        let stack_base_virt =
            VirtAddr::new(reserved.as_u64().saturating_add(KSTACK_PAGE_SIZE as u64));
        for (i, frame) in frames.iter().copied().enumerate() {
            tcb_trace(b"kstack: map page\n");
            let virt = VirtAddr::new(stack_base_virt.as_u64() + (i * KSTACK_PAGE_SIZE) as u64);
            let mapped = unsafe {
                crate::memory::virt::address_space::KERNEL_AS.map(
                    virt,
                    frame,
                    PageFlags::KERNEL_DATA,
                    &KernelStackPtAlloc,
                )
            };
            if mapped.is_err() {
                for j in 0..i {
                    let virt =
                        VirtAddr::new(stack_base_virt.as_u64() + (j * KSTACK_PAGE_SIZE) as u64);
                    if let Some(frame) =
                        unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(virt) }
                    {
                        let _ = crate::memory::physical::free_page(frame);
                    }
                }
                for frame in frames.iter().copied().skip(i) {
                    let _ = crate::memory::physical::free_page(frame);
                }
                return None;
            }
        }
        tcb_trace(b"kstack: mapped\n");

        let guard_id = crate::memory::integrity::register_guard_region(
            stack_base_virt.as_u64(),
            size as u64,
            crate::memory::integrity::GuardRegionKind::KernelThreadStack { tid: tid.0 as u64 },
        );
        tcb_trace(b"kstack: guard\n");

        let base = stack_base_virt.as_u64() as *mut u8;
        tcb_trace(b"kstack: canary before\n");
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }
        tcb_trace(b"kstack: canary after\n");
        let top = unsafe { base.add(size) } as u64;
        let top_aligned = (top & !0xF) - 8;

        Some(Self {
            base,
            size,
            top: top_aligned,
            backing: KernelStackBacking::GuardedVmalloc { frames, guard_id },
        })
    }

    /// Address of usable top (initial value of kernel RSP).
    #[inline(always)]
    pub fn top_addr(&self) -> u64 {
        self.top
    }

    /// Checks the canary — returns false if overflow detected.
    pub fn check_canary(&self) -> bool {
        // SAFETY: base was allocated with at least 8 bytes and the canary is placed there.
        unsafe { core::ptr::read(self.base as *const u64) == STACK_CANARY }
    }

    /// Size in bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        match self.backing {
            KernelStackBacking::Heap => {
                // SAFETY: base was allocated with this layout.
                unsafe {
                    let layout = Layout::from_size_align_unchecked(self.size, 16);
                    dealloc(self.base, layout);
                }
            }
            KernelStackBacking::GuardedVmalloc { frames, guard_id } => {
                if let Some(id) = guard_id {
                    let _ = crate::memory::integrity::unregister_guard_region(id);
                }

                for i in 0..KSTACK_USABLE_PAGES {
                    let virt = crate::memory::core::VirtAddr::new(
                        self.base as u64 + (i * KSTACK_PAGE_SIZE) as u64,
                    );
                    if let Some(frame) =
                        unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(virt) }
                    {
                        let _ = crate::memory::physical::free_page(frame);
                    } else if frames[i] != crate::memory::core::Frame::NULL {
                        let _ = crate::memory::physical::free_page(frames[i]);
                    }
                }
            }
        }
    }
}

// SAFETY: KernelStack not shared across threads (exclusive ownership).
unsafe impl Send for KernelStack {}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessThread — process/ view of a thread
// ─────────────────────────────────────────────────────────────────────────────

/// ProcessThread: scheduler TCB extension with process-level data.
///
/// Unique owner of the kernel stack and the scheduler TCB.
/// Referenced from the ProcessControlBlock of its parent process.
pub struct ProcessThread {
    // ── Scheduler TCB (hot path) ───────────────────────────────────────────────
    /// Scheduler TCB — exclusive ownership of this ProcessThread.
    /// Borrowed exclusively by the scheduler for context switches.
    pub(crate) sched_tcb: Box<ThreadControlBlock>,

    // ── Kernel stack ───────────────────────────────────────────────────────────
    /// Kernel stack dedicated to this thread.
    pub kernel_stack: KernelStack,

    // ── Process identity ───────────────────────────────────────────────────────
    /// PID of owning process.
    pub pid: Pid,
    /// TID of this thread.
    pub tid: Tid,

    // ── Userspace addresses ────────────────────────────────────────────────────
    /// Thread addresses on the userspace side.
    pub addresses: ThreadAddress,

    // ── TLS (Thread Local Storage) ─────────────────────────────────────────────
    /// FS base of the TLS segment. Historic name preserves existing internal ABI.
    pub tls_gs_base: AtomicU64,
    /// Static TLS block (.tdata/.tbss segment of binary).
    pub tls_block: AtomicUsize, // opaque *mut u8
    /// TLS block size.
    pub tls_size: usize,

    // ── Join state ─────────────────────────────────────────────────────────────
    /// true = detached thread (joiner will not wait).
    pub detached: AtomicBool,
    /// true = join completed (result available in join_result).
    pub join_done: AtomicBool,
    /// Return value of thread (ptr to userspace data).
    pub join_result: AtomicU64,

    // ── Signal queues ──────────────────────────────────────────────────────────
    /// Standard signal queue (signals 1..31).
    pub sig_queue: SigQueue,
    /// Real-time signal queue (signals 32..63).
    pub rt_sig_queue: RTSigQueue,
}

impl ProcessThread {
    /// Creates a new ProcessThread with a fresh kernel stack.
    ///
    /// # Arguments
    /// * `tid`   — TID allocated from TID_ALLOCATOR.
    /// * `pid`   — PID of owning process.
    /// * `cr3`   — CR3 of address space.
    /// * `policy`/`prio` — scheduling policy and priority.
    pub fn new(
        tid: Tid,
        pid: Pid,
        cr3: u64,
        policy: SchedPolicy,
        prio: Priority,
    ) -> Option<Box<Self>> {
        if !crate::memory::heap::is_heap_ready() {
            return None;
        }
        tcb_trace(b"thread_new: enter\n");
        let kstack = KernelStack::alloc(KSTACK_SIZE, tid)?;
        tcb_trace(b"thread_new: kstack\n");
        let stack_top = kstack.top_addr();
        sync_thread_kernel_half(cr3);
        tcb_trace(b"thread_new: sync\n");

        let mut sched_tcb = try_box_new(ThreadControlBlock::new(
            ThreadId(tid.0 as u64),
            ProcessId(pid.0),
            policy,
            prio,
            cr3,
            stack_top,
        ))?;
        tcb_trace(b"thread_new: tcb\n");

        if unsafe { !crate::scheduler::fpu::save_restore::alloc_fpu_state(&mut sched_tcb) } {
            return None;
        }
        tcb_trace(b"thread_new: fpu\n");

        if crate::security::is_cet_global_enabled() {
            unsafe {
                crate::security::enable_cet_for_thread(&mut sched_tcb).ok()?;
            }
        }
        tcb_trace(b"thread_new: cet\n");

        let thread = try_process_thread_box(sched_tcb, kstack, pid, tid)?;
        sync_thread_kernel_half(cr3);
        tcb_trace(b"thread_new: final sync\n");

        Some(thread)
    }

    /// Creates a dedicated kernel thread (pid=0, KTHREAD flag).
    pub fn new_kthread(tid: Tid, cr3: u64) -> Option<Box<Self>> {
        if !crate::memory::heap::is_heap_ready() {
            return None;
        }

        let kstack = KernelStack::alloc(KSTACK_SIZE, tid)?;
        let stack_top = kstack.top_addr();
        sync_thread_kernel_half(cr3);

        let mut sched_tcb = try_box_new(ThreadControlBlock::new_kthread(
            ThreadId(tid.0 as u64),
            cr3,
            stack_top,
        ))?;

        if crate::security::is_cet_global_enabled() {
            unsafe {
                crate::security::enable_cet_for_thread(&mut sched_tcb).ok()?;
            }
        }

        let thread = try_process_thread_box(sched_tcb, kstack, Pid::IDLE, tid)?;
        sync_thread_kernel_half(cr3);
        Some(thread)
    }

    /// Reference to scheduler TCB (short-lived, hot path).
    #[inline(always)]
    pub fn tcb(&self) -> &ThreadControlBlock {
        &self.sched_tcb
    }

    /// Mutable reference to scheduler TCB.
    #[inline(always)]
    pub fn tcb_mut(&mut self) -> &mut ThreadControlBlock {
        &mut self.sched_tcb
    }

    /// Raw pointer to TCB (used by run queues).
    #[inline(always)]
    pub fn tcb_ptr(&self) -> *mut ThreadControlBlock {
        &*self.sched_tcb as *const ThreadControlBlock as *mut ThreadControlBlock
    }

    /// Checks kernel stack canary integrity.
    #[inline(always)]
    pub fn check_stack_canary(&self) -> bool {
        self.kernel_stack.check_canary()
    }

    /// Raises signal_pending in the scheduler TCB (PROC-04).
    /// Called ONLY from process/signal/delivery.rs.
    #[inline(always)]
    pub fn raise_signal_pending(&self) {
        self.sched_tcb.set_signal_pending();
        // Request a reschedule for prompt delivery.
        self.sched_tcb.request_preemption();
    }

    /// Reads current thread state via scheduler TCB.
    #[inline(always)]
    pub fn state(&self) -> TaskState {
        self.sched_tcb.state()
    }

    /// Sets thread state.
    #[inline(always)]
    pub fn set_state(&self, s: TaskState) {
        self.sched_tcb.set_state(s);
    }
}

// SAFETY: ProcessThread is accessed from a single CPU at a time (scheduler ownership).
// Atomic fields allow targeted concurrent reads; the complete
// structure must not be shared as a Sync reference.
unsafe impl Send for ProcessThread {}
