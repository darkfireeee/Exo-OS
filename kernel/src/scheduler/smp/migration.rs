// kernel/src/scheduler/smp/migration.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Migration de threads — déplacement d'un thread d'un CPU à un autre
// ═══════════════════════════════════════════════════════════════════════════════
//
// Les migrations se font via IPI : le CPU source envoie un IPI au CPU cible
// avec le TCB à migrer. Le CPU cible intègre le thread dans sa run queue lors
// du prochain tick.
//
// Canal de migration : tableau circulaire lock-free par CPU cible
// (jusqu'à MIGRATION_QUEUE_DEPTH migrations en attente).
// ═══════════════════════════════════════════════════════════════════════════════

use super::topology::{nr_cpus, MAX_CPUS};
use crate::scheduler::core::task::{CpuId, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Canal de migration par CPU
// ─────────────────────────────────────────────────────────────────────────────

const MIGRATION_QUEUE_DEPTH: usize = 16;

/// File circulaire de migration pour un CPU cible.
struct MigrationQueue {
    head: AtomicU32,
    tail: AtomicU32,
    slots: [core::sync::atomic::AtomicPtr<ThreadControlBlock>; MIGRATION_QUEUE_DEPTH],
}

impl MigrationQueue {
    const fn new() -> Self {
        // SAFETY: AtomicPtr<T> has same representation as *mut T (null = 0).
        const NULL: core::sync::atomic::AtomicPtr<ThreadControlBlock> =
            core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: [NULL; MIGRATION_QUEUE_DEPTH],
        }
    }

    /// Enfile `tcb` dans la queue. Retourne `false` si pleine.
    fn push(&self, tcb: NonNull<ThreadControlBlock>) -> bool {
        let tail = self.tail.load(Ordering::Acquire);
        let next = (tail + 1) % MIGRATION_QUEUE_DEPTH as u32;
        if next == self.head.load(Ordering::Acquire) {
            return false; // Pleine.
        }
        self.slots[tail as usize].store(tcb.as_ptr(), Ordering::Release);
        self.tail.store(next, Ordering::Release);
        true
    }

    /// Défile le prochain TCB. Retourne `None` si vide.
    fn pop(&self) -> Option<NonNull<ThreadControlBlock>> {
        let head = self.head.load(Ordering::Acquire);
        if head == self.tail.load(Ordering::Acquire) {
            return None; // Vide.
        }
        let ptr = self.slots[head as usize].load(Ordering::Acquire);
        let next = (head + 1) % MIGRATION_QUEUE_DEPTH as u32;
        self.head.store(next, Ordering::Release);
        NonNull::new(ptr)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tableaux globaux (statique, no_alloc)
// ─────────────────────────────────────────────────────────────────────────────

static MIGRATION_QUEUES: [MigrationQueue; MAX_CPUS] = {
    // Initialisation par macro répétée — ne peut pas itérer avec const.
    // Utilisation d'un tableau d'initialiseurs.
    const INIT: MigrationQueue = MigrationQueue::new();
    [INIT; MAX_CPUS]
};

// ─────────────────────────────────────────────────────────────────────────────
// FFI vers l'arch (envoi IPI)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_send_reschedule_ipi(target_cpu: u32);
}

// ─────────────────────────────────────────────────────────────────────────────
// Métriques
// ─────────────────────────────────────────────────────────────────────────────

pub static MIGRATIONS_SENT: AtomicU64 = AtomicU64::new(0);
pub static MIGRATIONS_RECEIVED: AtomicU64 = AtomicU64::new(0);
pub static MIGRATIONS_DROPPED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Demande la migration du thread `tcb` vers le CPU `target`.
///
/// Le thread est enfilé dans la queue de migration du CPU cible, et un IPI de
/// reschedule est envoyé.
///
/// # Safety
/// Le TCB doit être retiré de la run queue source AVANT cet appel.
pub unsafe fn request_migration(tcb: NonNull<ThreadControlBlock>, target: CpuId) {
    let target_idx = target.0 as usize;
    if target_idx >= nr_cpus() {
        return;
    }

    // MIGRATED flag: encodé dans sched_state si nécessaire — no-op pour les stats seules.

    if !MIGRATION_QUEUES[target_idx].push(tcb) {
        // Queue pleine. Le thread a DÉJÀ été retiré de la run queue source
        // par cfs_dequeue_for_migration(). Sans réenfilage, il disparaît
        // définitivement (le commentaire original était erroné).
        // BUG-FIX J : réenfiler sur le CPU home pour ne pas perdre le thread.
        MIGRATIONS_DROPPED.fetch_add(1, Ordering::Relaxed);
        let home_raw = tcb.as_ref().cpu_id.load(Ordering::Acquire) as usize;
        if home_raw < nr_cpus() {
            // SAFETY: home_raw < nr_cpus() ≤ MAX_CPUS, run queue initialisée.
            let home_cpu = CpuId(home_raw as u32);
            crate::scheduler::core::runqueue::run_queue(home_cpu).enqueue(tcb);
        }
        return;
    }
    MIGRATIONS_SENT.fetch_add(1, Ordering::Relaxed);
    arch_send_reschedule_ipi(target.0);
}

/// Traite les migrations en attente pour le CPU courant.
///
/// `drain_cb` est appelé pour chaque TCB reçu.
pub unsafe fn drain_pending_migrations(
    cpu: CpuId,
    drain_cb: unsafe fn(tcb: NonNull<ThreadControlBlock>),
) {
    let q = &MIGRATION_QUEUES[cpu.0 as usize];
    while let Some(tcb) = q.pop() {
        // Mettre à jour le CPU courant dans le TCB.
        let tcb_mut = &mut *(tcb.as_ptr());
        tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Release);
        MIGRATIONS_RECEIVED.fetch_add(1, Ordering::Relaxed);
        drain_cb(tcb);
    }
}
