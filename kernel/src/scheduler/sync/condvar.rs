// kernel/src/scheduler/sync/condvar.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CondVar — variable de condition pour KMutex
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente le motif classique :
//   mutex.lock_blocking(tid, tcb)
//   while !condition {
//       condvar.wait_on(tcb, &mut mutex_guard);  ← relâche le mutex, dort, réacquiert
//   }
//
// RÈGLE WAITQ-01 : les WaitNodes sont alloués depuis l'EmergencyPool.
//
// GARANTIE : wait_on() est équivalent à POSIX pthread_cond_wait() — aucune perte
// de signal si notify_one/all est appelé après l'insertion du thread dans la queue
// mais avant qu'il ne dorme (grâce au numéro de séquence).
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::wait_queue::{WaitQueue, WaitNode};
use crate::scheduler::sync::mutex::KMutexGuard;
use crate::scheduler::core::task::{ThreadControlBlock, TaskState};

/// Compteurs d'instrumentation.
pub static CONDVAR_WAITS:     AtomicU64 = AtomicU64::new(0);
pub static CONDVAR_WAKEUPS:   AtomicU64 = AtomicU64::new(0);
pub static CONDVAR_SPURIOUS:  AtomicU64 = AtomicU64::new(0);

pub struct CondVar {
    waiters: WaitQueue,
    /// Numéro de séquence : incrémenté à chaque notify_one/notify_all.
    /// Permet de détecter un signal arrivé AVANT que wait_on() dorme
    /// (évite la perte de signal — race condition classique).
    seq: AtomicU64,
}

unsafe impl Send for CondVar {}
unsafe impl Sync for CondVar {}

impl CondVar {
    pub const fn new() -> Self {
        Self {
            waiters: WaitQueue::new(),
            seq:     AtomicU64::new(0),
        }
    }

    // ─── Notification ────────────────────────────────────────────────────────

    /// Réveille exactement un thread en attente sur cette CondVar.
    ///
    /// # Safety
    /// Appelé avec le mutex associé tenu ET préemption désactivée.
    pub unsafe fn notify_one(&mut self) {
        self.seq.fetch_add(1, Ordering::Release);
        self.waiters.wake_one();
        CONDVAR_WAKEUPS.fetch_add(1, Ordering::Relaxed);
    }

    /// Réveille tous les threads en attente.
    ///
    /// # Safety
    /// Appelé avec le mutex associé tenu ET préemption désactivée.
    pub unsafe fn notify_all(&mut self) {
        self.seq.fetch_add(1, Ordering::Release);
        let n = self.waiters.wake_all();
        CONDVAR_WAKEUPS.fetch_add(n as u64, Ordering::Relaxed);
    }

    // ─── Attente ──────────────────────────────────────────────────────────────

    /// Relâche `guard`, bloque le thread courant sur la CondVar, puis réacquiert
    /// le mutex avant de retourner.
    ///
    /// Équivalent POSIX de `pthread_cond_wait(cond, mutex)` :
    ///  1. Lire le numéro de séquence (snapshot atomique).
    ///  2. Allouer un WaitNode depuis l'EmergencyPool (RÈGLE WAITQ-01).
    ///  3. Insérer dans la liste d'attente.
    ///  4. Relâcher le mutex (Drop sur `guard`).
    ///  5. Marquer le TCB comme Sleeping.
    ///  6. Attendre jusqu'au changement du numéro de séquence (reveil).
    ///  7. Réacquérir le mutex via `lock_blocking()`.
    ///
    /// # Safety
    /// - `tcb` doit pointer vers le TCB valide du thread courant.
    /// - Appelé avec le mutex tenu (fourni via `guard`).
    /// - Préemption désactivée chez l'appelant (ou IrqGuard actif).
    ///
    /// # Retour
    /// Retourne un guard sur le mutex nouvellement réacquis.
    pub unsafe fn wait_on<'a, T>(
        &mut self,
        tcb: *mut ThreadControlBlock,
        guard: KMutexGuard<'a, T>,
    ) -> KMutexGuard<'a, T> {
        CONDVAR_WAITS.fetch_add(1, Ordering::Relaxed);

        // Snapshot du numéro de séquence AVANT l'insertion (évite la perte de signal).
        let seq_before = self.seq.load(Ordering::Acquire);

        // Allouer un WaitNode depuis l'EmergencyPool (RÈGLE WAITQ-01).
        let node_opt: Option<NonNull<WaitNode>> = WaitNode::alloc(tcb, 0);

        // Extraire le mutex et le TID du thread avant de relâcher le guard.
        // SAFETY: Le guard contient une référence au mutex — on récupère le TID
        // via le TCB (tid est dans la CL1, lecture seule ici).
        let tid = if !tcb.is_null() { (*tcb).tid.0 } else { 1 };
        let mutex_ref = guard.mutex;

        if let Some(node) = node_opt {
            // Insérer dans la wait queue AVANT de relâcher le mutex.
            // Cela garantit qu'un notify_one() après le relâchement ne sera pas perdu.
            self.waiters.insert(node);
        }

        // Marquer le TCB comme endormi AVANT de relâcher le mutex.
        if !tcb.is_null() {
            (*tcb).set_state(TaskState::Sleeping);
        }

        // Relâcher le mutex (Drop du guard).
        drop(guard);

        // Attente du signal : boucle active jusqu'au changement de séquence.
        // Dans un système complet, le scheduler retire ce thread de la run queue
        // dès qu'il détecte TaskState::Sleeping lors du prochain pick_next.
        loop {
            core::hint::spin_loop();

            // Vérifier si un signal est arrivé (seq a changé).
            let seq_now = self.seq.load(Ordering::Acquire);
            if seq_now != seq_before {
                break;
            }

            // Vérifier si le thread a été directement réveillé par wake_one/wake_all
            // (TaskState est repassé à Runnable).
            if !tcb.is_null() {
                let state = (*tcb).state();
                if state == TaskState::Runnable || state == TaskState::Running {
                    break;
                }
            }
        }

        // Marquer le thread comme de nouveau prêt s'il est encore endormi.
        if !tcb.is_null() {
            let state = (*tcb).state();
            if state == TaskState::Sleeping {
                (*tcb).set_state(TaskState::Runnable);
                CONDVAR_SPURIOUS.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Réacquérir le mutex.
        mutex_ref.lock_blocking(tid, tcb)
    }

    /// Séquence de notification courante (lecture externe pour tests/debug).
    #[inline(always)]
    pub fn seq(&self) -> u64 { self.seq.load(Ordering::Relaxed) }

    /// Nombre de threads actuellement en attente.
    #[inline(always)]
    pub fn waiters_count(&self) -> usize { self.waiters.count() }
}
