// kernel/src/scheduler/timer/deadline_timer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Deadline Timer — planification EDF basée sur les échéances absolues
// ═══════════════════════════════════════════════════════════════════════════════
//
// Gère la queue de threads SCHED_DEADLINE triés par échéance absolue croissante.
// Intègre la détection de miss d'échéance et la reprogrammation de la prochaine période.
// ═══════════════════════════════════════════════════════════════════════════════
crate::arch::x86_64::cpu::tsc::read_tsc()
// ─────────────────────────────────────────────────────────────────────────────
// File EDF par CPU — tableau trié par deadline_abs_ns croissant
// ─────────────────────────────────────────────────────────────────────────────

const MAX_DL_TASKS: usize = 64;

struct DeadlineQueue {
    tasks: [Option<NonNull<ThreadControlBlock>>; MAX_DL_TASKS],
    count: usize,
}

unsafe impl Send for DeadlineQueue {}
unsafe impl Sync for DeadlineQueue {}

impl DeadlineQueue {
    const fn new() -> Self {
        Self { tasks: [None; MAX_DL_TASKS], count: 0 }
    }

    fn insert(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        if self.count >= MAX_DL_TASKS { return false; }
        let dl = unsafe { tcb.as_ref().deadline_abs.load(core::sync::atomic::Ordering::Relaxed) };
        // Insertion triée par deadline_abs_ns croissant.
        let mut pos = self.count;
        for i in 0..self.count {
            let d = unsafe { self.tasks[i].unwrap().as_ref().deadline_abs.load(core::sync::atomic::Ordering::Relaxed) };
            if d > dl { pos = i; break; }
        }
        let mut j = self.count;
        while j > pos {
            self.tasks[j] = self.tasks[j - 1];
            j -= 1;
        }
        self.tasks[pos] = Some(tcb);
        self.count += 1;
        true
    }

    /// Extrait le thread avec l'échéance la plus proche.
    fn pop_earliest(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        if self.count == 0 { return None; }
        let tcb = self.tasks[0];
        let mut j = 0;
        while j + 1 < self.count { self.tasks[j] = self.tasks[j + 1]; j += 1; }
        self.tasks[self.count - 1] = None;
        self.count -= 1;
        tcb
    }

    /// Retourne (sans extraire) le thread avec l'échéance la plus proche.
    fn peek_earliest(&self) -> Option<NonNull<ThreadControlBlock>> {
        self.tasks[0]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Instances globales
// ─────────────────────────────────────────────────────────────────────────────

use core::mem::MaybeUninit;

static mut DL_QUEUES: [MaybeUninit<DeadlineQueue>; MAX_CPUS] =
    [const { MaybeUninit::uninit() }; MAX_CPUS];

pub static DL_ENQUEUES:    AtomicU64 = AtomicU64::new(0);
pub static DL_DEQUEUES:    AtomicU64 = AtomicU64::new(0);
pub static DL_MISS_EVENTS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise les queues deadline.
///
/// # Safety
/// Appelé une seule fois depuis scheduler::init().
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        DL_QUEUES[cpu].write(DeadlineQueue::new());
    }
}

/// Enfile un thread SCHED_DEADLINE dans la queue EDF du CPU `cpu`.
///
/// Rafraîchit l'échéance absolue via `refresh_deadline()` avant insertion.
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn dl_enqueue(cpu: usize, tcb_ptr: NonNull<ThreadControlBlock>) {
    let tcb = &mut *tcb_ptr.as_ptr();
    refresh_deadline(tcb);
    let q = DL_QUEUES[cpu].assume_init_mut();
    if q.insert(tcb_ptr) {
        DL_ENQUEUES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Extrait le thread EDF de priorité la plus haute (échéance la plus proche).
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn dl_pick_next(cpu: usize) -> Option<NonNull<ThreadControlBlock>> {
    let q = DL_QUEUES[cpu].assume_init_mut();
    let tcb_opt = q.pop_earliest();
    if tcb_opt.is_some() {
        DL_DEQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    tcb_opt
}

/// Retire un thread SCHED_DEADLINE spécifique de la queue EDF du CPU `cpu`.
/// Utilisé lors d'une migration ou d'une terminaison de thread.
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn dl_remove(cpu: usize, target: NonNull<ThreadControlBlock>) -> bool {
    let q = DL_QUEUES[cpu].assume_init_mut();
    for i in 0..q.count {
        if q.tasks[i] == Some(target) {
            let mut j = i;
            while j + 1 < q.count {
                q.tasks[j] = q.tasks[j + 1];
                j += 1;
            }
            q.tasks[q.count - 1] = None;
            q.count -= 1;
            return true;
        }
    }
    false
}

/// Vérifie les miss d'échéance sur le CPU `cpu` à chaque tick.
///
/// # Safety
/// Préemption désactivée requise.
pub unsafe fn dl_tick(cpu: usize) {
    let q = DL_QUEUES[cpu].assume_init_mut();
    for i in 0..q.count {
        if let Some(tcb_ptr) = q.tasks[i] {
            let tcb = tcb_ptr.as_ref();
            if check_deadline_miss(tcb) {
                DL_MISS_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
