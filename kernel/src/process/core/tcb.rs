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

fn order_for_pages(pages: usize) -> usize {
    let mut order = 0usize;
    let mut cap = 1usize;
    while cap < pages {
        cap <<= 1;
        order += 1;
    }
    order
}

/// Taille du stack kernel par thread (4 pages × 4096 = 16 384 bytes).
pub const KSTACK_SIZE: usize = 16 * 1024;
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
    GuardedPhysmap {
        frame: crate::memory::core::Frame,
        order: usize,
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
        use crate::memory::core::{AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};

        if size != KSTACK_SIZE
            || crate::memory::virt::address_space::KERNEL_AS
                .pml4_phys()
                .as_u64()
                == 0
        {
            return None;
        }

        let order = order_for_pages(KSTACK_TOTAL_PAGES);
        let frame = crate::memory::physical::alloc_pages(order, AllocFlags::ZEROED).ok()?;
        let phys_base = frame.start_address();
        let low_guard_virt = crate::memory::phys_to_virt(phys_base);
        let stack_base_virt = VirtAddr::new(low_guard_virt.as_u64() + KSTACK_PAGE_SIZE as u64);
        let high_guard_virt = VirtAddr::new(
            stack_base_virt.as_u64() + (KSTACK_USABLE_PAGES * KSTACK_PAGE_SIZE) as u64,
        );

        let low_guard_frame =
            unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(low_guard_virt) };
        let high_guard_frame =
            unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(high_guard_virt) };

        if low_guard_frame.is_none() || high_guard_frame.is_none() {
            if let Some(frame) = low_guard_frame {
                let _ = unsafe {
                    crate::memory::virt::address_space::KERNEL_AS.map(
                        low_guard_virt,
                        frame,
                        PageFlags::KERNEL_DATA,
                        &KernelStackPtAlloc,
                    )
                };
            }
            if let Some(frame) = high_guard_frame {
                let _ = unsafe {
                    crate::memory::virt::address_space::KERNEL_AS.map(
                        high_guard_virt,
                        frame,
                        PageFlags::KERNEL_DATA,
                        &KernelStackPtAlloc,
                    )
                };
            }
            let _ = crate::memory::physical::free_pages(frame, order);
            return None;
        }

        let guard_id = crate::memory::integrity::register_guard_region(
            stack_base_virt.as_u64(),
            size as u64,
            crate::memory::integrity::GuardRegionKind::KernelThreadStack { tid: tid.0 as u64 },
        );

        let base = stack_base_virt.as_u64() as *mut u8;
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }
        let top = unsafe { base.add(size) } as u64;
        let top_aligned = (top & !0xF) - 8;

        let expected_low = Frame::from_phys_addr(phys_base);
        let expected_high = Frame::from_phys_addr(PhysAddr::new(
            phys_base.as_u64() + ((KSTACK_USABLE_PAGES + 1) * KSTACK_PAGE_SIZE) as u64,
        ));
        debug_assert_eq!(low_guard_frame, Some(expected_low));
        debug_assert_eq!(high_guard_frame, Some(expected_high));

        Some(Self {
            base,
            size,
            top: top_aligned,
            backing: KernelStackBacking::GuardedPhysmap {
                frame,
                order,
                guard_id,
            },
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
            KernelStackBacking::GuardedPhysmap {
                frame,
                order,
                guard_id,
            } => {
                if let Some(id) = guard_id {
                    let _ = crate::memory::integrity::unregister_guard_region(id);
                }

                let low_guard_virt =
                    crate::memory::core::VirtAddr::new(self.base as u64 - KSTACK_PAGE_SIZE as u64);
                let high_guard_virt =
                    crate::memory::core::VirtAddr::new(self.base as u64 + self.size as u64);
                let low_frame = frame;
                let high_frame =
                    crate::memory::core::Frame::from_phys_addr(crate::memory::core::PhysAddr::new(
                        frame.start_address().as_u64()
                            + ((KSTACK_USABLE_PAGES + 1) * KSTACK_PAGE_SIZE) as u64,
                    ));

                let _ =
                    unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(low_guard_virt) };
                let _ =
                    unsafe { crate::memory::virt::address_space::KERNEL_AS.unmap(high_guard_virt) };
                let _ = unsafe {
                    crate::memory::virt::address_space::KERNEL_AS.map(
                        low_guard_virt,
                        low_frame,
                        crate::memory::core::PageFlags::KERNEL_DATA,
                        &KernelStackPtAlloc,
                    )
                };
                let _ = unsafe {
                    crate::memory::virt::address_space::KERNEL_AS.map(
                        high_guard_virt,
                        high_frame,
                        crate::memory::core::PageFlags::KERNEL_DATA,
                        &KernelStackPtAlloc,
                    )
                };

                if crate::memory::virt::address_space::KERNEL_AS
                    .translate(low_guard_virt)
                    .is_some()
                    && crate::memory::virt::address_space::KERNEL_AS
                        .translate(high_guard_virt)
                        .is_some()
                {
                    let _ = crate::memory::physical::free_pages(frame, order);
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
        let kstack = KernelStack::alloc(KSTACK_SIZE, tid)?;
        let stack_top = kstack.top_addr();

        let mut sched_tcb = try_box_new(ThreadControlBlock::new(
            ThreadId(tid.0 as u64),
            ProcessId(pid.0),
            policy,
            prio,
            cr3,
            stack_top,
        ))?;

        if crate::security::is_cet_global_enabled() {
            unsafe {
                crate::security::enable_cet_for_thread(&mut sched_tcb).ok()?;
            }
        }

        let thread = try_box_new(Self {
            sched_tcb,
            kernel_stack: kstack,
            pid,
            tid,
            addresses: ThreadAddress::default(),
            tls_gs_base: AtomicU64::new(0),
            tls_block: AtomicUsize::new(0),
            tls_size: 0,
            detached: AtomicBool::new(false),
            join_done: AtomicBool::new(false),
            join_result: AtomicU64::new(0),
            sig_queue: SigQueue::new(),
            rt_sig_queue: RTSigQueue::new(),
        })?;

        Some(thread)
    }

    /// Crée un thread kernel dédié (pid=1, KTHREAD flag).
    pub fn new_kthread(tid: Tid, cr3: u64) -> Option<Box<Self>> {
        Self::new(
            tid,
            Pid::INIT,
            cr3,
            SchedPolicy::Normal,
            Priority::NORMAL_DEFAULT,
        )
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
