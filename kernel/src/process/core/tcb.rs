// kernel/src/process/core/tcb.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessThread — Extension process/ du ThreadControlBlock scheduler
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   Le scheduler/core/task.rs définit `ThreadControlBlock` (256 bytes, hot path).
//   Ce fichier définit `ProcessThread` qui ENVELOPPE le TCB scheduler et ajoute
//   les données niveau process (adresse de stack, TLS, signaux...) sans toucher
//   aux 128 bytes du TCB scheduler.
//
// RÈGLES :
//   • ProcessThread est alloué par lifecycle/create.rs, libéré par lifecycle/exit.rs.
//   • Le pointeur `sched_tcb` pointe vers un TCB en mémoire statique ou heap.
//   • Tous les accès à sched_tcb depuis process/ sont documentaires.
//   • PROC-04 : signal_pending est ÉCRIT ici (process/signal/), LU par scheduler.
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

/// Taille du stack kernel par thread.
///
/// Les chemins userspace critiques (fork/exec/IPC) traversent assez de Rust
/// noyau pour que 16 KiB soit fragile en debug. 64 KiB garde une marge saine
/// tout en restant modeste pour la charge de services Ring1 actuelle.
pub const KSTACK_SIZE: usize = 64 * 1024;
const KSTACK_PAGE_SIZE: usize = crate::memory::core::constants::PAGE_SIZE;
const KSTACK_USABLE_PAGES: usize = KSTACK_SIZE / KSTACK_PAGE_SIZE;
const KSTACK_TOTAL_PAGES: usize = KSTACK_USABLE_PAGES + 2;

const _: () = assert!(KSTACK_SIZE % KSTACK_PAGE_SIZE == 0);

/// Canari de stack pour détecter les débordements.
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

#[derive(Clone, Copy)]
enum KernelStackBacking {
    Heap,
    GuardedVmalloc {
        frames: [crate::memory::core::Frame; KSTACK_USABLE_PAGES],
        guard_id: Option<crate::memory::integrity::GuardRegionId>,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// ThreadAddress — adresses de l'espace utilisateur d'un thread
// ─────────────────────────────────────────────────────────────────────────────

/// Adresses liées au cycle de vie du thread côté userspace.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct ThreadAddress {
    /// Base du stack utilisateur alloué (plus basse adresse).
    pub stack_base: u64,
    /// Taille du stack utilisateur (bytes).
    pub stack_size: u64,
    /// Registre d’instruction de retour (RIP initial au lancement).
    pub entry_point: u64,
    /// Pointeur de cadre utilisateur initial (RSP au démarrage).
    pub initial_rsp: u64,
    /// Pointeur vers la TLS statique (GS.base pour x86_64).
    pub tls_base: u64,
    /// Premier argument injecté en RDI au point d'entrée initial.
    pub entry_arg0: u64,
    /// Pointeur vers la structure `pthread_t` userspace.
    pub pthread_ptr: u64,
    /// Zone `sigaltstack` (stack alternatif pour signaux).
    pub sigaltstack_base: u64,
    pub sigaltstack_size: u64,
}

impl ThreadAddress {
    /// Adresse du sommet du sigaltstack (base + size).
    #[inline(always)]
    pub fn sigaltstack_top(&self) -> u64 {
        self.sigaltstack_base.saturating_add(self.sigaltstack_size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KernelStack — stack kernel dédié du thread
// ─────────────────────────────────────────────────────────────────────────────

/// Stack kernel alloué dynamiquement pour un thread.
///
/// Le chemin normal alloue une région physique dédiée :
/// `[guard !PRESENT][stack utile][guard !PRESENT]`. Si le noyau est encore trop
/// tôt dans le boot pour modifier la PML4, le backend heap garde le canari bas.
pub struct KernelStack {
    /// Pointeur vers la mémoire allouée (bas du buffer).
    base: *mut u8,
    /// Taille totale en bytes.
    size: usize,
    /// Adresse du sommet (base + size, aligné 16).
    top: u64,
    backing: KernelStackBacking,
}

impl KernelStack {
    /// Alloue un nouveau stack kernel de `size` bytes.
    /// Pose un canari 8 bytes au bas et, si possible, deux vraies pages garde.
    pub fn alloc(size: usize, tid: Tid) -> Option<Self> {
        Self::alloc_guarded(size, tid).or_else(|| Self::alloc_heap_canary(size))
    }

    fn alloc_heap_canary(size: usize) -> Option<Self> {
        if !crate::memory::heap::is_heap_ready() {
            return None;
        }
        let layout = Layout::from_size_align(size, 16).ok()?;
        // SAFETY: layout est valide, on vérifie le pointeur.
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            return None;
        }
        // Écriture du canari au bas du stack.
        // SAFETY: base pointe vers `size` bytes alloués, canari à offset 0.
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }
        // SAFETY: base a été alloué avec `size` bytes ; base.add(size) pointe juste après la fin,
        //         ce qui est un pointeur valide (sentinel, jamais déréférencé).
        let top = unsafe { base.add(size) } as u64;
        // Aligner sur 16 bytes (x86_64 ABI) : le top doit être 16-aligné - 8.
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
        let mut mapped_pages = 0usize;
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
                for j in 0..mapped_pages {
                    let virt =
                        VirtAddr::new(stack_base_virt.as_u64() + (j * KSTACK_PAGE_SIZE) as u64);
                    if let Some(frame) =
                        unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(virt) }
                    {
                        let _ = crate::memory::physical::free_page(frame);
                    }
                }
                for frame in frames.iter().copied().skip(mapped_pages) {
                    let _ = crate::memory::physical::free_page(frame);
                }
                return None;
            }
            mapped_pages += 1;
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

    /// Adresse du sommet utile (valeur initiale du RSP kernel).
    #[inline(always)]
    pub fn top_addr(&self) -> u64 {
        self.top
    }

    /// Vérifie le canari — retourne false si débordement détecté.
    pub fn check_canary(&self) -> bool {
        // SAFETY: base a été alloué avec au moins 8 bytes et le canari y est posé.
        unsafe { core::ptr::read(self.base as *const u64) == STACK_CANARY }
    }

    /// Taille en bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        match self.backing {
            KernelStackBacking::Heap => {
                // SAFETY: base a été alloué avec ce layout.
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

// SAFETY: KernelStack non partagé entre threads (propriété exclusive).
unsafe impl Send for KernelStack {}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessThread — vue process/ d'un thread
// ─────────────────────────────────────────────────────────────────────────────

/// ProcessThread : extension du TCB scheduler avec les données niveau process.
///
/// Propriétaire unique du stack kernel et du TCB scheduler.
/// Référencé depuis le ProcessControlBlock de son processus parent.
pub struct ProcessThread {
    // ── TCB scheduler (hot path) ───────────────────────────────────────────────
    /// TCB scheduler — propriété exclusive de ce ProcessThread.
    /// Borrowé de manière exclusive par le scheduler pour les context switches.
    pub(crate) sched_tcb: Box<ThreadControlBlock>,

    // ── Stack kernel ───────────────────────────────────────────────────────────
    /// Stack kernel dédié à ce thread.
    pub kernel_stack: KernelStack,

    // ── Identité process ───────────────────────────────────────────────────────
    /// PID du processus propriétaire.
    pub pid: Pid,
    /// TID de ce thread.
    pub tid: Tid,

    // ── Adresses userspace ─────────────────────────────────────────────────────
    /// Adresses du thread côté userspace.
    pub addresses: ThreadAddress,

    // ── TLS (Thread Local Storage) ─────────────────────────────────────────────
    /// Base du segment TLS (valeur de GS.base en mode kernel).
    pub tls_gs_base: AtomicU64,
    /// Bloc TLS statique (segment .tdata/.tbss du binaire).
    pub tls_block: AtomicUsize, // *mut u8 opaque
    /// Taille du bloc TLS.
    pub tls_size: usize,

    // ── État de join ───────────────────────────────────────────────────────────
    /// true = thread detaché (le joineur n'attendra pas).
    pub detached: AtomicBool,
    /// true = join terminé (résultat disponible dans join_result).
    pub join_done: AtomicBool,
    /// Valeur de retour du thread (ptr vers donnée userspace).
    pub join_result: AtomicU64,

    // ── Files de signaux ───────────────────────────────────────────────────────
    /// File de signaux standard (signaux 1..31).
    pub sig_queue: SigQueue,
    /// File de signaux temps-réel (signaux 32..63).
    pub rt_sig_queue: RTSigQueue,
}

impl ProcessThread {
    /// Crée un nouveau ProcessThread avec un stack kernel frais.
    ///
    /// # Arguments
    /// * `tid`   — TID alloué depuis TID_ALLOCATOR.
    /// * `pid`   — PID du processus propriétaire.
    /// * `cr3`   — CR3 de l'espace d'adressage.
    /// * `policy`/`prio` — politique et priorité d'ordonnancement.
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

    /// Crée un thread kernel dédié (pid=0, KTHREAD flag).
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

    /// Référence au TCB scheduler (short-lived, hot path).
    #[inline(always)]
    pub fn tcb(&self) -> &ThreadControlBlock {
        &self.sched_tcb
    }

    /// Référence mutable au TCB scheduler.
    #[inline(always)]
    pub fn tcb_mut(&mut self) -> &mut ThreadControlBlock {
        &mut self.sched_tcb
    }

    /// Pointeur brut vers le TCB (utilisé par les run queues).
    #[inline(always)]
    pub fn tcb_ptr(&self) -> *mut ThreadControlBlock {
        &*self.sched_tcb as *const ThreadControlBlock as *mut ThreadControlBlock
    }

    /// Vérifie l'intégrité du canari kernel stack.
    #[inline(always)]
    pub fn check_stack_canary(&self) -> bool {
        self.kernel_stack.check_canary()
    }

    /// Elève le signal_pending dans le TCB scheduler (PROC-04).
    /// Appelé UNIQUEMENT depuis process/signal/delivery.rs.
    #[inline(always)]
    pub fn raise_signal_pending(&self) {
        self.sched_tcb.set_signal_pending();
        // Requérir un reschedule pour livraison rapide.
        self.sched_tcb.request_preemption();
    }

    /// Lit l'état courant du thread via le TCB scheduler.
    #[inline(always)]
    pub fn state(&self) -> TaskState {
        self.sched_tcb.state()
    }

    /// Définit l'état du thread.
    #[inline(always)]
    pub fn set_state(&self, s: TaskState) {
        self.sched_tcb.set_state(s);
    }
}

// SAFETY: ProcessThread est accédé depuis un seul CPU à la fois (propriété scheduler).
// Les champs atomiques permettent les lectures concurrentes ciblées ; la
// structure complète ne doit pas être partagée comme référence Sync.
unsafe impl Send for ProcessThread {}
