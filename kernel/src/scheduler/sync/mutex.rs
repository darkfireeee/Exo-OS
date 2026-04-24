// kernel/src/scheduler/sync/mutex.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// KMutex — Mutex bloquant kernel avec héritage de priorité simplifié
// ═══════════════════════════════════════════════════════════════════════════════
//
// Deux modes d'acquisition :
//   try_lock()     — non-bloquant, retourne None si pris
//   lock_blocking() — bloquant via WaitQueue + TaskState::Sleeping
//                    (nécessite TCB du thread appelant)
//
// RÈGLE WAITQ-01 : les WaitNodes proviennent de l'EmergencyPool.
// RÈGLE PREEMPT-01 : la préemption est désactivée autour des sections critiques
//                   via IrqGuard (imposé par l'appelant).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use crate::scheduler::sync::wait_queue::{WaitNode, WaitQueue};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Compteurs d'instrumentation.
pub static KMUTEX_CONTENTIONS: AtomicU64 = AtomicU64::new(0);
pub static KMUTEX_ACQUIRES: AtomicU64 = AtomicU64::new(0);

/// Mutex bloquant. Le thread est mis en attente (TaskState::Sleeping) si le
/// verrou est déjà pris, et réveillé lors du release().
pub struct KMutex<T> {
    /// TID du propriétaire courant. 0 = libre.
    owner_tid: AtomicU32,
    /// File d'attente des threads bloqués sur ce mutex.
    waiters: UnsafeCell<WaitQueue>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for KMutex<T> {}
unsafe impl<T: Send> Sync for KMutex<T> {}

impl<T> KMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            owner_tid: AtomicU32::new(0),
            waiters: UnsafeCell::new(WaitQueue::new()),
            data: UnsafeCell::new(value),
        }
    }

    // ─── Chemin non-bloquant ────────────────────────────────────────────────

    /// Essai d'acquisition sans blocage.
    /// Retourne `Some(guard)` si libre, `None` si contention.
    pub fn try_lock(&self, tid: u32) -> Option<KMutexGuard<'_, T>> {
        self.owner_tid
            .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| {
                KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
                KMutexGuard { mutex: self }
            })
    }

    // ─── Chemin bloquant ─────────────────────────────────────────────────────

    /// Acquiert le mutex en bloquant le thread appelant si nécessaire.
    ///
    /// Séquence :
    ///  1. CAS fast-path (non-contended) → return immédiatement.
    ///  2. Contention : alloue un WaitNode depuis l'EmergencyPool.
    ///  3. Insère le WaitNode en queue, passe le thread en TaskState::Sleeping.
    ///  4. Boucle jusqu'au réveil par wake_one() dans release().
    ///  5. Après réveil : retente le CAS (un seul thread gagnant, les autres
    ///     dorment de nouveau).
    ///
    /// # Safety
    /// - `tid` = TID du thread courant.
    /// - `tcb` = pointeur valide vers le TCB du thread courant.
    /// - Préemption désactivée ou IrqGuard actif chez l'appelant.
    /// - Ne PAS appeler depuis un contexte IN_RECLAIM (deadlock EmergencyPool possible).
    pub unsafe fn lock_blocking(
        &self,
        tid: u32,
        tcb: *mut ThreadControlBlock,
    ) -> KMutexGuard<'_, T> {
        // Fast path (non-contended) : CAS sans allocation.
        if self
            .owner_tid
            .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
            return KMutexGuard { mutex: self };
        }

        KMUTEX_CONTENTIONS.fetch_add(1, Ordering::Relaxed);

        loop {
            // Tenter d'allouer un WaitNode depuis l'EmergencyPool.
            // RÈGLE WAITQ-01 : jamais de Box::new ici.
            if let Some(node) = WaitNode::alloc(tcb, 0) {
                // BUG-FIX T : marquer le thread comme endormi AVANT l'insertion
                // dans la wait queue.
                // Si on insère d'abord puis set_state(Sleeping), une fenêtre de
                // race est ouverte : release() peut appeler wake_one() entre les
                // deux opérations, tenter try_transition(Sleeping→Runnable) sur
                // un thread encore Running → le CAS échoue → le thread n'est
                // jamais réenfilé dans la run queue → gel définitif du thread.
                if !tcb.is_null() {
                    (*tcb).set_state(TaskState::Sleeping);
                }

                // Insérer dans la wait queue (FIFO) après le changement d'état.
                let wq = &mut *self.waiters.get();
                wq.insert(node);

                // BUG-FIX P : utiliser schedule_block() au lieu d'une boucle active.
                // La boucle active précédente causait deux problèmes critiques :
                //   1. Brûlait 100% CPU pendant toute la durée de la contention.
                //   2. Double-scheduling : release() appelait wake_one() → enqueue()
                //      pendant que ce thread tournait encore sur son CPU, autorisant
                //      une exécution simultanée sur deux CPUs différents.
                // schedule_block() suspend proprement le thread sans le ré-enqueuer,
                // et retourne uniquement quand le thread est réveillé par wake_one().
                if !tcb.is_null() {
                    let cpu_raw = (*tcb).cpu_id.load(Ordering::Relaxed) as usize;
                    if cpu_raw < crate::scheduler::core::preempt::MAX_CPUS {
                        let cpu_id = crate::scheduler::core::task::CpuId(cpu_raw as u32);
                        // SAFETY: cpu_raw < MAX_CPUS, run queue initialisée.
                        let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                        crate::scheduler::core::switch::schedule_block(rq, &mut *tcb);
                    }
                }
                // Note : WaitNode est libéré par wake_one() dans release().
            } else {
                // EmergencyPool épuisé (situation critique) — spin bref.
                core::hint::spin_loop();
            }

            // Retenter l'acquisition après réveil.
            if self
                .owner_tid
                .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
                return KMutexGuard { mutex: self };
            }
            // Un autre thread a pris le mutex le premier → reboucler.
        }
    }

    // ─── Libération ──────────────────────────────────────────────────────────

    unsafe fn release(&self) {
        // Libérer la propriété avant de réveiller (évite une fenêtre de race).
        self.owner_tid.store(0, Ordering::Release);
        // Réveiller le prochain thread en attente (FIFO).
        let wq = &mut *self.waiters.get();
        wq.wake_one();
    }

    // ─── Diagnostics ──────────────────────────────────────────────────────────

    /// Retourne `true` si le mutex est actuellement pris.
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        self.owner_tid.load(Ordering::Relaxed) != 0
    }

    /// Retourne le TID du propriétaire actuel (0 = libre).
    #[inline(always)]
    pub fn owner(&self) -> u32 {
        self.owner_tid.load(Ordering::Relaxed)
    }
}

pub struct KMutexGuard<'a, T> {
    /// Accès `pub(crate)` requis par CondVar::wait_on() pour réacquérir
    /// le mutex après réveil sans exposer le champ publiquement.
    pub(crate) mutex: &'a KMutex<T>,
}

impl<'a, T> core::ops::Deref for KMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for KMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T> Drop for KMutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.mutex.release();
        }
    }
}
