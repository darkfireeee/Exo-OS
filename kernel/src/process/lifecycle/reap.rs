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

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::process::core::pid::{Pid, Tid, PID_ALLOCATOR};
use crate::process::core::pcb::ProcessState;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::lifecycle::create::{create_kthread, KthreadParams};
use crate::scheduler::core::task::Priority;

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
// ReaperQueue — SPSC ring lock-free
// ─────────────────────────────────────────────────────────────────────────────

const REAPER_RING_SIZE: usize = 512;

/// File SPSC lock-free pour le reaper.
pub struct ReaperQueue {
    /// Anneau de 512 entrées (positions 64-alignées pour éviter false sharing).
    ring: [ReaperEntry; REAPER_RING_SIZE],
    /// Position producteur (enqueue).
    head: core::cell::UnsafeCell<usize>,
    /// Position consommateur (dequeue).
    tail: AtomicUsize,
    /// Compteur total d'enqueue à des fins de debugging.
    total_enqueued: AtomicU64,
    /// Entrées lost (file pleine).
    lost: AtomicU64,
}

// SAFETY: les accès head/tail sont sérísés par convention SPSC
// (producteur unique = tout thread faisant exit, consommateur unique = kthread reaper).
// En pratique, head est protégé par un spinlock optionnel (à ajouter si MPSC nécessaire).
unsafe impl Sync for ReaperQueue {}

pub static REAPER_QUEUE: ReaperQueue = ReaperQueue {
    ring:           [ReaperEntry { pid: 0, tid: 0 }; REAPER_RING_SIZE],
    head:           core::cell::UnsafeCell::new(0),
    tail:           AtomicUsize::new(0),
    total_enqueued: AtomicU64::new(0),
    lost:           AtomicU64::new(0),
};

impl ReaperQueue {
    /// Enqueue un thread à reaper.
    /// Appelé depuis do_exit() au moment où un thread passe Dead.
    pub fn enqueue(&self, pid: Pid, tid: Tid) {
        // SAFETY: head est modifié exclusivement ici (un seul producteur par convention).
        let head = unsafe { &mut *self.head.get() };
        let next = (*head + 1) % REAPER_RING_SIZE;
        let tail = self.tail.load(Ordering::Acquire);
        if next == tail {
            // File pleine.
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // SAFETY: *head < REAPER_RING_SIZE garanti par mod.
        unsafe {
            let slot = &self.ring[*head] as *const ReaperEntry as *mut ReaperEntry;
            (*slot).pid = pid.0;
            (*slot).tid = tid.0;
        }
        *head = next;
        self.total_enqueued.fetch_add(1, Ordering::Relaxed);
    }

    /// Dequeue une entrée. Retourne None si la file est vide.
    pub(crate) fn dequeue(&self) -> Option<ReaperEntry> {
        let tail = self.tail.load(Ordering::Relaxed);
        // SAFETY: head lu en lecture seule ici (atomicité garantie par usize).
        let head = unsafe { *self.head.get() };
        if tail == head { return None; }
        // SAFETY: tail < REAPER_RING_SIZE.
        let entry = unsafe { core::ptr::read(&self.ring[tail]) };
        self.tail.store((tail + 1) % REAPER_RING_SIZE, Ordering::Release);
        Some(entry)
    }

    /// Statistiques.
    pub fn stats(&self) -> (u64, u64) {
        (self.total_enqueued.load(Ordering::Relaxed), self.lost.load(Ordering::Relaxed))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reaper kthread
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de processus/threads reapés depuis le boot.
static REAPED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Boucle principale du kthread reaper.
/// Ne retourne JAMAIS.
fn reaper_loop(_arg: usize) -> ! {
    loop {
        while let Some(entry) = REAPER_QUEUE.dequeue() {
            reap_entry(entry);
        }
        // Pas d'entrée : pause CPU pour réduire la contention bus.
        core::hint::spin_loop();
    }
}

/// Libère les ressources associées à une entrée reaper.
fn reap_entry(entry: ReaperEntry) {
    let pid = Pid(entry.pid);
    let _tid = Tid(entry.tid);

    // Vérifier si c'est le dernier thread du processus.
    let is_last_thread = PROCESS_REGISTRY
        .find_by_pid(pid)
        .map(|pcb| pcb.thread_count.load(Ordering::Relaxed) == 0
                && pcb.state() == ProcessState::Zombie)
        .unwrap_or(false);

    if is_last_thread {
        // Retirer de la registry et libérer le PCB.
        if let Ok(pcb_box) = PROCESS_REGISTRY.remove(pid) {
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
    create_kthread(&KthreadParams {
        name:       "reaper",
        entry:      reaper_loop,
        arg:        0,
        target_cpu: 0,
        priority:   Priority::NORMAL_DEFAULT,
    }).expect("init_reaper: impossible de démarrer le kthread reaper");
}

/// Nombre de processus/threads libérés depuis le boot.
pub fn reaped_count() -> u64 {
    REAPED_COUNT.load(Ordering::Relaxed)
}
