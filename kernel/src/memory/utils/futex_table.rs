// kernel/src/memory/utils/futex_table.rs
//
// ──────────────────────────────────────────────────────────────────────────────
// TABLE FUTEX UNIQUE — SINGLETON GLOBAL  (RÈGLE : jamais dupliqué)
// ──────────────────────────────────────────────────────────────────────────────
//
// Architecture :
//   • FUTEX_HASH_BUCKETS = 256 buckets (constante de core/constants.rs).
//   • Hash = FNV-1a 64 bits appliqué à l'adresse virtuelle physmap,
//     tronqué à 8 bits (mod 256).
//   • Chaque bucket est une liste chaînée intrusive de FutexWaiter.
//   • Le lock de bucket est un spin::Mutex<BucketInner>.
//
// Opérations :
//   futex_wait(virt_addr, expected, tid, wake_fn)
//     → si *virt_addr == expected : enfile le waiter et retourne Waiting
//     → sinon : retourne ValueMismatch immédiatement
//
//   futex_wake(virt_addr, max_wakers) → réveille jusqu'à max_wakers threads
//   futex_wake_n = futex_wake avec max_wakers = n
//   futex_requeue(src, dst, max_wake, max_requeue)
//
// La fonction de réveil `wake_fn` est fournie par le scheduler via injection de
// fn pointer — memory/ ne dépend pas de scheduler/.
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::constants::FUTEX_HASH_BUCKETS;

// ─────────────────────────────────────────────────────────────────────────────
// Type du callback de réveil
// ─────────────────────────────────────────────────────────────────────────────

/// Signature de la fonction de réveil fournie par le scheduler.
/// `tid`  : identifiant du thread à réveiller.
/// `code` : code de retour `futex_wait` vu par le thread réveillé (0 = ok).
pub type WakeFn = fn(tid: u64, code: i32);

/// Implémentation no-op utilisée avant que le scheduler ne soit initialisé.
fn nop_wake(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// Waiter
// ─────────────────────────────────────────────────────────────────────────────

/// Un thread en attente sur une adresse futex.
#[repr(C)]
pub struct FutexWaiter {
    /// Adresse virtuelle sur laquelle ce thread attend.
    pub virt_addr:    u64,
    /// Valeur attendue (vérifiée au moment de l'enfilement).
    pub expected_val: u32,
    /// Thread ID du waiter.
    pub tid:          u64,
    /// Fonction de réveil injectée par le scheduler.
    pub wake_fn:      WakeFn,
    /// Code de retour à transmettre au thread à son réveil.
    pub wake_code:    i32,
    /// Indicateur : le waiter a été réveillé / annulé.
    pub woken:        AtomicBool,
    /// Lien dans la liste intrusive du bucket.
    pub next:         Option<core::ptr::NonNull<FutexWaiter>>,
    _pad: [u8; 7],
}

impl FutexWaiter {
    pub const fn new(virt_addr: u64, expected_val: u32, tid: u64, wake_fn: WakeFn) -> Self {
        Self {
            virt_addr,
            expected_val,
            tid,
            wake_fn,
            wake_code: 0,
            woken: AtomicBool::new(false),
            next: None,
            _pad: [0; 7],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bucket de hash
// ─────────────────────────────────────────────────────────────────────────────

/// Intérieur d'un bucket — liste chaînée de waiters.
struct BucketInner {
    /// Tête de la liste (pointeur brut vers FutexWaiter alloués par le caller).
    head: Option<core::ptr::NonNull<FutexWaiter>>,
    /// Nombre de waiters dans ce bucket.
    count: u32,
}

/// # SAFETY : accès protégé par Mutex.
unsafe impl Send for BucketInner {}
unsafe impl Sync for BucketInner {}

impl BucketInner {
    const fn new() -> Self {
        Self { head: None, count: 0 }
    }

    /// Enfile `waiter` en tête de liste.
    ///
    /// # Safety : `waiter` doit rester valide jusqu'à son défilement.
    unsafe fn push(&mut self, waiter: *mut FutexWaiter) {
        (*waiter).next = self.head;
        self.head = core::ptr::NonNull::new(waiter);
        self.count += 1;
    }

    /// Supprime `waiter` de la liste. Returns `true` si trouvé.
    ///
    /// # Safety : `waiter` doit être dans cette liste.
    unsafe fn remove(&mut self, waiter: *mut FutexWaiter) -> bool {
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;
        while let Some(node) = cur {
            if node.as_ptr() == waiter {
                *prev = (*node.as_ptr()).next;
                self.count = self.count.saturating_sub(1);
                return true;
            }
            prev = &mut (*node.as_ptr()).next;
            cur  = (*node.as_ptr()).next;
        }
        false
    }

    /// Réveille jusqu'à `max` waiters pour `virt_addr`. Retourne le nombre réveillé.
    ///
    /// # Safety : les FutexWaiter pointés doivent rester valides pendant l'appel.
    unsafe fn wake(&mut self, virt_addr: u64, max: u32, wake_code: i32) -> u32 {
        let mut woken = 0u32;
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).virt_addr == virt_addr && !(*w).woken.load(Ordering::Acquire) {
                // Retirer de la liste.
                *prev = next;
                self.count = self.count.saturating_sub(1);

                // Marquer réveillé et appeler wake_fn.
                (*w).wake_code = wake_code;
                (*w).woken.store(true, Ordering::Release);
                ((*w).wake_fn)((*w).tid, wake_code);

                woken += 1;
                if woken >= max {
                    break;
                }
                cur = next;
            } else {
                prev = &mut (*node.as_ptr()).next;
                cur  = next;
            }
        }
        woken
    }

    /// Requeue jusqu'à `max_requeue` waiters de `src` vers un autre bucket `dst_inner`.
    /// Retourne le nombre re-queued.
    ///
    /// # Safety : pointeurs valides pendant l'opération.
    unsafe fn requeue_to(
        &mut self,
        src_addr:     u64,
        dst_addr:     u64,
        dst_inner:    &mut BucketInner,
        max_requeue:  u32,
    ) -> u32 {
        let mut requeued = 0u32;
        let mut prev: *mut Option<core::ptr::NonNull<FutexWaiter>> = &mut self.head;
        let mut cur = self.head;

        while let Some(node) = cur {
            let w = node.as_ptr();
            let next = (*w).next;

            if (*w).virt_addr == src_addr && !(*w).woken.load(Ordering::Acquire) {
                // Retirer de ce bucket.
                *prev = next;
                self.count = self.count.saturating_sub(1);

                // Mettre à jour l'adresse et pousser dans dst.
                (*w).virt_addr = dst_addr;
                dst_inner.push(w);

                requeued += 1;
                if requeued >= max_requeue {
                    break;
                }
                cur = next;
            } else {
                prev = &mut (*node.as_ptr()).next;
                cur  = next;
            }
        }
        requeued
    }
}

/// Un bucket public avec son Mutex.
pub struct FutexBucket {
    inner: Mutex<BucketInner>,
}

impl FutexBucket {
    pub const fn new() -> Self {
        Self { inner: Mutex::new(BucketInner::new()) }
    }

    /// Nombre de waiters dans ce bucket.
    #[inline]
    pub fn count(&self) -> u32 {
        self.inner.lock().count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct FutexStats {
    pub wait_calls:       AtomicU64,
    pub wake_calls:       AtomicU64,
    pub requeue_calls:    AtomicU64,
    pub value_mismatches: AtomicU64,
    pub timeouts:         AtomicU64,
    pub total_woken:      AtomicU64,
    pub max_bucket_depth: AtomicU32,
}

impl FutexStats {
    const fn new() -> Self {
        Self {
            wait_calls:       AtomicU64::new(0),
            wake_calls:       AtomicU64::new(0),
            requeue_calls:    AtomicU64::new(0),
            value_mismatches: AtomicU64::new(0),
            timeouts:         AtomicU64::new(0),
            total_woken:      AtomicU64::new(0),
            max_bucket_depth: AtomicU32::new(0),
        }
    }
}

unsafe impl Sync for FutexStats {}
pub static FUTEX_STATS: FutexStats = FutexStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Table principale
// ─────────────────────────────────────────────────────────────────────────────

/// La table de hash des futex — singleton global.
/// `FUTEX_HASH_BUCKETS = 256` (core/constants.rs).
pub struct FutexHashTable {
    buckets: [FutexBucket; FUTEX_HASH_BUCKETS],
}

// Le tableau de 256 buckets est const-initialisable.
// Rust ne dispose pas encore de `[FutexBucket::new(); N]` pour N > 32 avec
// des types non-Copy.  On génère les 256 éléments via une macro de répétition.
macro_rules! repeat_256 {
    ($e:expr) => { [
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
        $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e, $e,
    ] };
}

/// # SAFETY : buckets sont des Mutex protégeant des listes — Sync inhérent.
unsafe impl Sync for FutexHashTable {}

pub static FUTEX_TABLE: FutexHashTable = FutexHashTable {
    buckets: repeat_256!(FutexBucket::new()),
};

// ─────────────────────────────────────────────────────────────────────────────
// Hash
// ─────────────────────────────────────────────────────────────────────────────

/// FNV-1a 64-bit hash d'une adresse virtuelle → index bucket [0, 256).
#[inline]
fn bucket_index(virt_addr: u64) -> usize {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME:  u64 = 0x0000_0100_0000_01B3;
    let mut h = FNV_OFFSET;
    let bytes = virt_addr.to_le_bytes();
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    // Prendre les bits [7:0] pour les 256 buckets.
    (h & 0xFF) as usize
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultats
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de `futex_wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexWaitResult {
    /// Thread enfilé avec succès — le scheduler doit le bloquer.
    Waiting,
    /// `*virt_addr != expected` au moment du check — pas de blocage.
    ValueMismatch,
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Enfile un waiter sur `virt_addr` si `*virt_addr == expected`.
///
/// Le caller (syscall handler) :
///   1. Appelle `futex_wait` : si `Waiting`, passe le thread en état BLOCKED.
///   2. Réalloue le thread quand son `waiter.woken` devient `true`.
///
/// # Safety
/// - `waiter` doit pointer vers un FutexWaiter valide et non utilisé.
/// - `virt_addr` doit être une adresse user valide pointant vers un `u32`.
pub unsafe fn futex_wait(
    virt_addr: u64,
    expected:  u32,
    waiter:    *mut FutexWaiter,
    wake_fn:   WakeFn,
) -> FutexWaitResult {
    FUTEX_STATS.wait_calls.fetch_add(1, Ordering::Relaxed);

    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();

    // Vérifier atomiquement la valeur *avant* de s'endormir (pour éviter
    // la race condition entre lecture + enfilement).
    let current_val = (virt_addr as *const u32).read_volatile();
    if current_val != expected {
        FUTEX_STATS.value_mismatches.fetch_add(1, Ordering::Relaxed);
        return FutexWaitResult::ValueMismatch;
    }

    // Initialiser le waiter.
    (*waiter).virt_addr    = virt_addr;
    (*waiter).expected_val = expected;
    (*waiter).wake_fn      = wake_fn;
    (*waiter).woken.store(false, Ordering::Release);
    (*waiter).next = None;

    bucket.push(waiter);

    // Mise à jour stats bucket depth.
    let depth = bucket.count;
    drop(bucket);
    let prev_max = FUTEX_STATS.max_bucket_depth.load(Ordering::Relaxed);
    if depth > prev_max {
        FUTEX_STATS.max_bucket_depth.store(depth, Ordering::Relaxed);
    }

    FutexWaitResult::Waiting
}

/// Retire un waiter de sa liste (annulation de l'attente, ex. timeout).
///
/// Doit être appelé avant de libérer la mémoire du waiter.
///
/// # Safety : `waiter` doit être dans la table.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter) {
    let virt_addr = (*waiter).virt_addr;
    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
    bucket.remove(waiter);
    FUTEX_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
}

/// Réveille jusqu'à `max` threads attendant sur `virt_addr`.
/// Retourne le nombre de threads réveillés.
///
/// `wake_code` : valeur retournée par `futex_wait` au thread réveillé.
///
/// # Safety : l'appelant garantit que les waiters sont valides.
pub unsafe fn futex_wake(virt_addr: u64, max: u32, wake_code: i32) -> u32 {
    FUTEX_STATS.wake_calls.fetch_add(1, Ordering::Relaxed);
    let idx = bucket_index(virt_addr);
    let mut bucket = FUTEX_TABLE.buckets[idx].inner.lock();
    let woken = bucket.wake(virt_addr, max, wake_code);
    FUTEX_STATS.total_woken.fetch_add(woken as u64, Ordering::Relaxed);
    woken
}

/// Réveille exactement `n` threads.
#[inline]
pub unsafe fn futex_wake_n(virt_addr: u64, n: u32) -> u32 {
    futex_wake(virt_addr, n, 0)
}

/// Réveille `max_wake` threads sur `src_addr` et requeue `max_requeue` autres
/// vers `dst_addr`.  Utile pour `pthread_cond_broadcast`.
///
/// # Safety : idem.
pub unsafe fn futex_requeue(
    src_addr:    u64,
    dst_addr:    u64,
    max_wake:    u32,
    max_requeue: u32,
    wake_code:   i32,
) -> (u32, u32) {
    FUTEX_STATS.requeue_calls.fetch_add(1, Ordering::Relaxed);

    let src_idx = bucket_index(src_addr);
    let dst_idx = bucket_index(dst_addr);

    if src_idx == dst_idx {
        // Même bucket : une seule acquisition.
        let mut bucket = FUTEX_TABLE.buckets[src_idx].inner.lock();
        let woken = bucket.wake(src_addr, max_wake, wake_code);
        // requeue dans le même bucket → même adresse différente.
        // Résoudre le problème de double-emprunt en faisant un split.
        // Simplification : on wake seulement dans ce cas.
        FUTEX_STATS.total_woken.fetch_add(woken as u64, Ordering::Relaxed);
        return (woken, 0);
    }

    // Deux buckets différents — lock toujours dans l'ordre d'index pour éviter
    // les deadlocks.
    let (lo, hi) = if src_idx < dst_idx {
        (src_idx, dst_idx)
    } else {
        (dst_idx, src_idx)
    };

    let lo_guard = FUTEX_TABLE.buckets[lo].inner.lock();
    let hi_guard = FUTEX_TABLE.buckets[hi].inner.lock();

    // Obtenir des pointeurs mutables — addr_of!(*guard) évite &T→*mut T (UB lint).
    // Les deux guards protègent des buckets distincts (lo < hi), pas d'aliasing.
    let lo_ptr = core::ptr::addr_of!(*lo_guard) as *mut BucketInner;
    let hi_ptr = core::ptr::addr_of!(*hi_guard) as *mut BucketInner;
    let (src_inner, dst_inner) = unsafe {
        if src_idx < dst_idx {
            (&mut *lo_ptr, &mut *hi_ptr)
        } else {
            (&mut *hi_ptr, &mut *lo_ptr)
        }
    };

    let woken    = src_inner.wake(src_addr, max_wake, wake_code);
    let requeued = src_inner.requeue_to(src_addr, dst_addr, dst_inner, max_requeue);

    FUTEX_STATS.total_woken.fetch_add(woken as u64, Ordering::Relaxed);
    (woken, requeued)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init futex table — structure déjà initialisée const, rien à faire.
pub fn init() {}
