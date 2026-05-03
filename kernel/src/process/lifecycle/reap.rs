// kernel/src/process/lifecycle/reap.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Reaper kthread — libération asynchrone des ressources zombie (Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE PROC-07 : la libération des ressources d'un thread/processus terminé
// est effectuée par un kthread dédié (jamais inline dans exit()).
//
// File reaper :
//   • SPSC lock-free ring de 512 entrées.
//   • Chaque entrée = (pid, tid) du thread à reaper.
//   • Le kthread reaper lit la file, trouve le ProcessThread via registry,
//     et libère toutes les ressources.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pcb::ProcessState;
use crate::process::core::pid::{Pid, Tid, PID_ALLOCATOR};
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::process::lifecycle::fork::AddressSpaceCloner;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::{current_thread_raw, schedule_block};
use crate::scheduler::core::task::{CpuId, Priority, TaskState};
use crate::scheduler::sync::wait_queue::{WaitNode, WaitQueue};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// ReaperEntry — une entrée dans la file reaper
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Default)]
pub(crate) struct ReaperEntry {
    /// PID du processus (non-zéro si dernier thread).
    pid: u32,
    /// TID du thread à libérer.
    tid: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// ReaperQueue — ring MPSC borné + débordement dynamique
// ─────────────────────────────────────────────────────────────────────────────

const REAPER_RING_SIZE: usize = 4096;

struct OverflowQueue {
    entries: Vec<ReaperEntry>,
    head: usize,
}

impl OverflowQueue {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            head: 0,
        }
    }

    fn push(&mut self, entry: ReaperEntry) -> bool {
        if self.entries.len() == self.entries.capacity() && self.entries.try_reserve(1).is_err() {
            return false;
        }
        self.entries.push(entry);
        true
    }

    fn pop(&mut self) -> Option<ReaperEntry> {
        if self.head >= self.entries.len() {
            return None;
        }
        let entry = self.entries[self.head];
        self.head += 1;

        if self.head >= self.entries.len() {
            self.entries.clear();
            self.head = 0;
        } else if self.head >= 64 && self.head * 2 >= self.entries.len() {
            self.entries.drain(0..self.head);
            self.head = 0;
        }

        Some(entry)
    }

    fn is_empty(&self) -> bool {
        self.head >= self.entries.len()
    }
}

static REAPER_OVERFLOW: Mutex<OverflowQueue> = Mutex::new(OverflowQueue::new());
static REAPER_SLEEP_LOCK: Mutex<()> = Mutex::new(());
static REAPER_WAIT_QUEUE: WaitQueue = WaitQueue::new();

/// File SPSC lock-free pour le reaper.
pub struct ReaperQueue {
    /// Anneau principal borné.
    ring: [core::cell::UnsafeCell<ReaperEntry>; REAPER_RING_SIZE],
    /// Position producteur.
    head: AtomicUsize,
    /// Position consommateur.
    tail: AtomicUsize,
    /// Sérialise les producteurs multiples (`do_exit()` peut courir sur plusieurs CPUs).
    producer_lock: Mutex<()>,
    /// Compteur total d'enqueue à des fins de debugging.
    total_enqueued: AtomicU64,
    /// Entrées perdues faute de place même dans le débordement.
    lost: AtomicU64,
}

// SAFETY: `head` est protégé par `producer_lock`, `tail` est atomique, et le
// consommateur est unique (kthread reaper).
unsafe impl Sync for ReaperQueue {}

pub static REAPER_QUEUE: ReaperQueue = ReaperQueue::new();

impl ReaperQueue {
    pub const fn new() -> Self {
        Self {
            ring: [const { core::cell::UnsafeCell::new(ReaperEntry { pid: 0, tid: 0 }) };
                REAPER_RING_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer_lock: Mutex::new(()),
            total_enqueued: AtomicU64::new(0),
            lost: AtomicU64::new(0),
        }
    }

    /// Enqueue un thread à reaper.
    /// Appelé depuis do_exit() au moment où un thread passe Dead.
    pub fn enqueue(&self, pid: Pid, tid: Tid) {
        let entry = ReaperEntry {
            pid: pid.0,
            tid: tid.0,
        };
        let enqueued = {
            let _producer_guard = self.producer_lock.lock();
            let head = self.head.load(Ordering::Relaxed);
            let next = (head + 1) % REAPER_RING_SIZE;
            let tail = self.tail.load(Ordering::Acquire);
            if next != tail {
                // SAFETY: `head < REAPER_RING_SIZE` garanti par le modulo.
                unsafe {
                    *self.ring[head].get() = entry;
                }
                self.head.store(next, Ordering::Release);
                true
            } else {
                REAPER_OVERFLOW.lock().push(entry)
            }
        };

        if enqueued {
            self.total_enqueued.fetch_add(1, Ordering::Relaxed);
            let _sleep_guard = REAPER_SLEEP_LOCK.lock();
            // SAFETY: réveil opportuniste; aucun effet si le reaper n'est pas endormi.
            unsafe {
                REAPER_WAIT_QUEUE.wake_one();
            }
        } else {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Dequeue une entrée. Retourne None si la file est vide.
    pub(crate) fn dequeue(&self) -> Option<ReaperEntry> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return REAPER_OVERFLOW.lock().pop();
        }
        // SAFETY: tail < REAPER_RING_SIZE.
        let entry = unsafe { core::ptr::read(self.ring[tail].get()) };
        self.tail
            .store((tail + 1) % REAPER_RING_SIZE, Ordering::Release);
        Some(entry)
    }

    /// Statistiques.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_enqueued.load(Ordering::Relaxed),
            self.lost.load(Ordering::Relaxed),
        )
    }

    fn has_pending(&self) -> bool {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        if tail != head {
            return true;
        }
        !REAPER_OVERFLOW.lock().is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reaper kthread
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de processus/threads reapés depuis le boot.
static REAPED_COUNT: AtomicU64 = AtomicU64::new(0);

fn sleep_until_work() {
    let tcb = current_thread_raw();
    if tcb.is_null() {
        core::hint::spin_loop();
        return;
    }

    // SAFETY: le reaper est un kthread scheduler valide; le WaitNode vit jusqu'au réveil.
    let Some(node) = (unsafe { WaitNode::alloc(tcb, 0) }) else {
        core::hint::spin_loop();
        return;
    };

    let sleep_guard = REAPER_SLEEP_LOCK.lock();
    if REAPER_QUEUE.has_pending() {
        // SAFETY: le nœud n'a jamais été inséré dans la file.
        unsafe {
            WaitNode::free(node);
        }
        return;
    }

    // SAFETY: `tcb` pointe vers le thread courant; il est sûr de l'endormir
    // puis de l'insérer dans la wait queue sous le verrou de rendez-vous.
    unsafe {
        (*tcb).set_state(TaskState::Sleeping);
        REAPER_WAIT_QUEUE.insert(node);
    }
    drop(sleep_guard);

    let cpu_id = CpuId(unsafe { (*tcb).cpu_id.load(Ordering::Relaxed) as u32 });
    // SAFETY: `cpu_id` provient du TCB courant en cours d'exécution.
    let rq = unsafe { run_queue(cpu_id) };
    // SAFETY: le thread est désormais visible dans la wait queue, donc le réveil
    // ne sera pas perdu. `schedule_block` reprend uniquement après wake_one().
    unsafe {
        schedule_block(rq, &mut *tcb);
    }
}

/// Boucle principale du kthread reaper.
/// Ne retourne JAMAIS.
fn reaper_loop(_arg: usize) -> ! {
    loop {
        while let Some(entry) = REAPER_QUEUE.dequeue() {
            reap_entry(entry);
        }
        sleep_until_work();
    }
}

/// Libère les ressources associées à une entrée reaper.
fn reap_entry(entry: ReaperEntry) {
    let pid = Pid(entry.pid);
    let _tid = Tid(entry.tid);

    // Vérifier si c'est le dernier thread du processus.
    let is_last_thread = PROCESS_REGISTRY
        .find_by_pid(pid)
        .map(|pcb| {
            pcb.thread_count.load(Ordering::Relaxed) == 0 && pcb.state() == ProcessState::Zombie
        })
        .unwrap_or(false);

    if is_last_thread {
        // Retirer de la registry et libérer le PCB.
        if let Ok(pcb_box) = PROCESS_REGISTRY.remove(pid) {
            let addr_space_ptr = pcb_box.address_space.load(Ordering::Acquire);
            let closed_handles = {
                let mut files = pcb_box.files.lock();
                files.close_all()
            };
            drop(closed_handles);
            crate::process::lifecycle::exit::close_all_pid_vfs(pid.0);
            if addr_space_ptr != 0 {
                crate::memory::virt::address_space::fork_impl::KERNEL_AS_CLONER
                    .free_addr_space(addr_space_ptr);
            }
            // Le PCB (Box<ProcessControlBlock>) est libéré ici par RAII.
            drop(pcb_box);
            // Libérer le PID.
            PID_ALLOCATOR.free(pid.0);
        }
    }

    REAPED_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Lance le kthread reaper. Appelé une seule fois depuis process::init().
///
/// # Safety
/// Appelé après `scheduler::init()` depuis le BSP.
pub fn init_reaper() {
    // SAFETY: trace E9 bornée pour localiser précisément l'init du reaper au boot.
    unsafe {
        core::arch::asm!("mov al, 0x52", "out 0xe9, al", options(nomem, nostack));
        // 'R'
    }
    create_kthread(&KthreadParams {
        name: "reaper",
        entry: reaper_loop,
        arg: 0,
        target_cpu: 0,
        priority: Priority::NORMAL_DEFAULT,
    })
    .expect("init_reaper: impossible de démarrer le kthread reaper");
    // SAFETY: trace E9 bornée pour confirmer la fin d'init_reaper.
    unsafe {
        core::arch::asm!("mov al, 0x72", "out 0xe9, al", options(nomem, nostack));
        // 'r'
    }
}

/// Nombre de processus/threads libérés depuis le boot.
pub fn reaped_count() -> u64 {
    REAPED_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_uses_overflow_when_ring_saturates() {
        let q = ReaperQueue::new();
        let total = REAPER_RING_SIZE + 8;

        for i in 0..total {
            q.enqueue(Pid((i + 1) as u32), Tid((i + 1) as u32));
        }

        let mut drained = 0usize;
        while q.dequeue().is_some() {
            drained += 1;
        }

        assert_eq!(drained, total);
        assert_eq!(q.stats().1, 0);
    }

    #[test]
    fn queue_reports_pending_when_only_overflow_has_entries() {
        let q = ReaperQueue::new();
        for i in 0..(REAPER_RING_SIZE + 1) {
            q.enqueue(Pid((i + 1) as u32), Tid((i + 1) as u32));
        }

        for _ in 0..(REAPER_RING_SIZE - 1) {
            let _ = q.dequeue();
        }

        assert!(q.has_pending());
        while q.dequeue().is_some() {}
        assert!(!q.has_pending());
    }
}
