// kernel/src/scheduler/sync/wait_queue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Wait Queue — file d'attente de threads (Exo-OS · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE WAITQ-01 : Les WaitNode sont alloués EXCLUSIVEMENT depuis l'EmergencyPool
//   (jamais depuis l'allocateur heap — risque de deadlock pendant la réclamation).
//
// Fonctionnement :
//   • Un thread qui doit attendre appelle `wait_queue_wait()` avec un WaitNode
//     pré-alloué depuis l'EmergencyPool.
//   • Il est inséré dans la liste de la WaitQueue, son état passe à BLOCKED.
//   • `wait_queue_wake_one()` / `wait_queue_wake_all()` réveillent les threads.
//   • Le WaitNode est libéré vers l'EmergencyPool APRÈS le réveil.
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::core::task::{ThreadControlBlock, TaskState};
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::task::CpuId;
use super::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// EmergencyPool FFI (Couche 0 — memory/)
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    /// Alloue un WaitNode depuis l'EmergencyPool. Retourne null si épuisé.
    fn emergency_pool_alloc_wait_node() -> *mut WaitNode;
    /// Libère un WaitNode vers l'EmergencyPool.
    fn emergency_pool_free_wait_node(node: *mut WaitNode);
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitNode — nœud d'une liste chaînée intrusive
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct WaitNode {
    pub tcb:  *mut ThreadControlBlock,
    pub next: *mut WaitNode,
    pub prev: *mut WaitNode,
    /// Drapeaux : bit 0 = EXCLUSIVE (réveiller un seul thread).
    pub flags: u32,
    _pad: u32,
}

impl WaitNode {
    pub const EXCLUSIVE: u32 = 1 << 0;

    /// Alloue un nœud depuis l'EmergencyPool.
    ///
    /// # Safety
    /// RÈGLE WAITQ-01 : appel OBLIGATOIRE via cette fonction — jamais `Box::new`.
    pub unsafe fn alloc(tcb: *mut ThreadControlBlock, flags: u32) -> Option<NonNull<WaitNode>> {
        let ptr = emergency_pool_alloc_wait_node();
        let node = NonNull::new(ptr)?;
        let n = &mut *node.as_ptr();
        n.tcb   = tcb;
        n.next  = core::ptr::null_mut();
        n.prev  = core::ptr::null_mut();
        n.flags = flags;
        Some(node)
    }

    /// Libère le nœud vers l'EmergencyPool.
    ///
    /// # Safety
    /// Le nœud ne doit plus être dans aucune liste.
    pub unsafe fn free(node: NonNull<WaitNode>) {
        emergency_pool_free_wait_node(node.as_ptr());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaitQueue — liste d'attente protégée par spinlock
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

/// Données mutables de la file d'attente (toujours accédées sous `lock`).
struct WaitQueueData {
    head:  *mut WaitNode,
    count: usize,
}

pub struct WaitQueue {
    data: UnsafeCell<WaitQueueData>,
    lock: SpinLock<()>,
}

// SAFETY: WaitQueue est protégé par un SpinLock. Les mutations passent par
// UnsafeCell::get() sous la protection du lock. Sûr Sync/Send.
unsafe impl Send for WaitQueue {}
unsafe impl Sync for WaitQueue {}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            data: UnsafeCell::new(WaitQueueData {
                head:  core::ptr::null_mut(),
                count: 0,
            }),
            lock: SpinLock::new(()),
        }
    }

    /// Insère un nœud à la fin de la liste (FIFO).
    ///
    /// # Safety
    /// Le `node` doit être valide et non encore dans une liste.
    pub unsafe fn insert(&self, node: NonNull<WaitNode>) {
        let _g = self.lock.lock();
        let d = &mut *self.data.get();
        let n = node.as_ptr();
        (*n).next = core::ptr::null_mut();
        (*n).prev = core::ptr::null_mut();

        if d.head.is_null() {
            d.head = n;
        } else {
            let mut cur = d.head;
            while !(*cur).next.is_null() { cur = (*cur).next; }
            (*cur).next = n;
            (*n).prev   = cur;
        }
        d.count += 1;
    }

    /// Retire un nœud de la liste.
    ///
    /// # Safety
    /// Le `node` doit être dans cette liste.
    pub unsafe fn remove(&self, node: NonNull<WaitNode>) {
        let _g = self.lock.lock();
        let d = &mut *self.data.get();
        let n = node.as_ptr();
        if !(*n).prev.is_null() { (*(*n).prev).next = (*n).next; }
        else                    { d.head = (*n).next; }
        if !(*n).next.is_null() { (*(*n).next).prev = (*n).prev; }
        (*n).next  = core::ptr::null_mut();
        (*n).prev  = core::ptr::null_mut();
        if d.count > 0 { d.count -= 1; }
    }

    /// Réveille le premier thread en attente.
    /// Thread-safe — prend le lock interne.
    pub fn notify_one(&self) -> bool {
        let _g = self.lock.lock();
        // SAFETY: sous le lock, accès exclusif à data.
        unsafe {
            let d = &mut *self.data.get();
            if d.head.is_null() { return false; }
            let node = d.head;
            d.head = (*node).next;
            if !d.head.is_null() { (*d.head).prev = core::ptr::null_mut(); }
            if d.count > 0 { d.count -= 1; }

            let tcb = (*node).tcb;
            if !tcb.is_null() {
                // BUG-FIX K : vérifier que la transition CAS réussit.
                // Si le thread n'est pas en état Sleeping (ex. déjà Runnable
                // à cause d'un timeout ou signal concurrent), ne PAS l'enfiler
                // dans la run queue : il y est peut-être déjà → double-scheduling.
                let transitioned = (*tcb).try_transition(TaskState::Sleeping, TaskState::Runnable);
                if transitioned {
                    // BUG-FIX L : valider les bornes de cpu_id avant run_queue().
                    // En release mode, debug_assert est no-op → UB si cpu hors limites.
                    let cpu_raw = (*tcb).cpu.load(Ordering::Relaxed) as usize;
                    if cpu_raw < crate::scheduler::core::preempt::MAX_CPUS {
                        let cpu_id = CpuId(cpu_raw as u32);
                        let rq = run_queue(cpu_id);
                        rq.enqueue(NonNull::new_unchecked(tcb));
                    }
                }
            }
            emergency_pool_free_wait_node(node);
            WAITQ_WAKEUPS.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    /// Réveille TOUS les threads en attente.
    /// Thread-safe — prend le lock interne.
    pub fn notify_all(&self) -> usize {
        let mut woken = 0usize;
        while self.notify_one() { woken += 1; }
        woken
    }

    pub fn is_empty(&self) -> bool {
        let _g = self.lock.lock();
        // SAFETY: sous le lock.
        unsafe { (*self.data.get()).head.is_null() }
    }

    pub fn count(&self) -> usize {
        let _g = self.lock.lock();
        // SAFETY: sous le lock.
        unsafe { (*self.data.get()).count }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Compatibilité amont — wrappers &mut self délégant vers &self
    // ─────────────────────────────────────────────────────────────────────

    /// Alias de `notify_one` pour compatibilité.
    #[inline] pub unsafe fn wake_one(&self) -> bool { self.notify_one() }
    /// Alias de `notify_all` pour compatibilité.
    #[inline] pub unsafe fn wake_all(&self) -> usize { self.notify_all() }

    // ─────────────────────────────────────────────────────────────────────
    // Blocage interruptible
    // ─────────────────────────────────────────────────────────────────────

    /// Bloque le thread courant jusqu'au prochain `notify_one`/`notify_all`.
    /// Retourne `true` si réveillé, `false` si interrompu par signal (-EINTR).
    ///
    /// RÈGLE WAITQ-01 : WaitNode alloué depuis l'EmergencyPool.
    ///
    /// # Safety
    /// `tcb` doit pointer vers le TCB du thread courant.
    /// Appelé avec le thread dans un état cohérent (Running).
    pub unsafe fn wait_interruptible(&self, tcb: *mut ThreadControlBlock) -> bool {
        // 1. Allouer un WaitNode depuis l'EmergencyPool.
        let node = match WaitNode::alloc(tcb, 0) {
            Some(n) => n,
            None    => return false, // EmergencyPool épuisé
        };

        // 2. Mettre l'état en Sleeping AVANT d'insérer (évite réveil manqué).
        (*tcb).set_state(TaskState::Sleeping);

        // 3. Insérer dans la file d'attente.
        self.insert(node);

        // 4. Vérification du signal (APRÈS insertion — fenêtre de réveil manqué évitée).
        if (*tcb).signal_pending.load(Ordering::Acquire) {
            self.remove(node);
            WaitNode::free(node);
            (*tcb).set_state(TaskState::Runnable);
            return false; // -EINTR
        }

        // 5. Bloquer — schedule_block ne revient que quand le thread est réveillé.
        // V-38 / PREEMPT-BLOCK : INTERDIT de bloquer sous PreemptGuard actif.
        crate::scheduler::core::preempt::assert_preempt_enabled();
        let cpu_id = CpuId((*tcb).cpu.load(Ordering::Relaxed));
        let rq = run_queue(cpu_id);
        crate::scheduler::core::switch::schedule_block(rq, &mut *tcb);

        // 6. Après réveil : nœud déjà libéré par le waker.
        !(*tcb).signal_pending.load(Ordering::Acquire)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Métriques
// ─────────────────────────────────────────────────────────────────────────────

pub static WAITQ_WAKEUPS: AtomicU64 = AtomicU64::new(0);
pub static WAITQ_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

/// Initialise le sous-système wait_queue.
///
/// Vérifie que l'EmergencyPool est déjà initialisé.
///
/// # Safety
/// Appelé depuis `scheduler::init()`, après `memory::init()`.
pub unsafe fn init() {
    // Vérification : tenter une alloc/free de test depuis l'EmergencyPool.
    let test = emergency_pool_alloc_wait_node();
    if !test.is_null() {
        emergency_pool_free_wait_node(test);
    }
    // Sinon, l'EmergencyPool n'est pas initialisé — erreur fatale
    // gérée par l'appelant (scheduler::init affiche un kernel panic).
}
