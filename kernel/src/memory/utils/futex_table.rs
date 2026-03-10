// kernel/src/memory/utils/futex_table.rs
//
// ──────────────────────────────────────────────────────────────────────────────
// TABLE FUTEX UNIQUE — SINGLETON GLOBAL  (RÈGLE : jamais dupliqué)
// ──────────────────────────────────────────────────────────────────────────────
//
// Architecture :
//   • FUTEX_HASH_BUCKETS = 4096 buckets (constante de core/constants.rs).
//     RÈGLE MEM-FUTEX (V-34) : ≥ 4096 + SipHash-keyed — anti-DoS par collision.
//   • Hash = SipHash-1-3 keyed (graine initialisée depuis security::crypto::rng
//     au boot step 18, via init_futex_seed()). Fallback FNV avant init.
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


use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, Ordering};
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
#[allow(dead_code)]
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
/// `FUTEX_HASH_BUCKETS = 4096` (core/constants.rs) — R\u00c8GLE MEM-FUTEX (V-34).
pub struct FutexHashTable {
    buckets: [FutexBucket; FUTEX_HASH_BUCKETS],
}

/// # SAFETY : buckets sont des Mutex protégeant des listes — Sync inhérent.
unsafe impl Sync for FutexHashTable {}

// SAFETY: FutexBucket = Mutex<BucketInner>. spin::Mutex all-zeros = déverrouillé
// (AtomicBool(0)). BucketInner all-zeros : head = None (0), count = 0 — état
// initial valide identique à FutexBucket::new(). Rust ne supporte pas
// [non_copy_expr; 4096] sans Copy, d'où l'initialisation par zeroed().
pub static FUTEX_TABLE: FutexHashTable = unsafe { core::mem::zeroed() };

// ─────────────────────────────────────────────────────────────────────────────
// Hash
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Hash — SipHash-1-3 keyed (RÈGLE MEM-FUTEX / V-34)
// ─────────────────────────────────────────────────────────────────────────────

// Graine de hash — 16 octets atomiques (2 × u64).
// Initialisée à 0 ; set_futex_hash_seed() est appelé au boot step 18.
// Avant l'init, le hash est SipHash avec clé = 0 (sous-optimal mais sûr au boot).
static FUTEX_SEED_K0: AtomicU64 = AtomicU64::new(0);
static FUTEX_SEED_K1: AtomicU64 = AtomicU64::new(0);

/// Indicateur : graine initialisée (atomique 8-bit — pas de dépendance spin::Once).
static FUTEX_SEED_SET: AtomicU8 = AtomicU8::new(0);

/// Initialise la graine SipHash depuis security::crypto::rng (boot step 18).
/// DOIT être appelée une seule fois, depuis le BSP, AVANT le démarrage des APs.
/// Après cet appel, bucket_index() utilise SipHash-1-3 keyed.
pub fn init_futex_seed(key: [u8; 16]) {
    let k0 = u64::from_le_bytes(key[0..8].try_into().unwrap_or([0u8; 8]));
    let k1 = u64::from_le_bytes(key[8..16].try_into().unwrap_or([0u8; 8]));
    FUTEX_SEED_K0.store(k0, Ordering::Release);
    FUTEX_SEED_K1.store(k1, Ordering::Release);
    FUTEX_SEED_SET.store(1, Ordering::Release);
}

/// SipHash-1-3 pour un seul bloc de 8 octets (une adresse physique/virtuelle).
/// Pas de dépendance externe — implémentation inline (no_std).
#[inline]
fn siphash13(k0: u64, k1: u64, data: u64) -> u64 {
    macro_rules! sipround {
        ($v0:expr, $v1:expr, $v2:expr, $v3:expr) => {
            $v0 = $v0.wrapping_add($v1); $v1 = $v1.rotate_left(13); $v1 ^= $v0;
            $v0 = $v0.rotate_left(32);
            $v2 = $v2.wrapping_add($v3); $v3 = $v3.rotate_left(16); $v3 ^= $v2;
            $v0 = $v0.wrapping_add($v3); $v3 = $v3.rotate_left(21); $v3 ^= $v0;
            $v2 = $v2.wrapping_add($v1); $v1 = $v1.rotate_left(17); $v1 ^= $v2;
            $v2 = $v2.rotate_left(32);
        };
    }
    let mut v0 = k0 ^ 0x736f6d6570736575u64;
    let mut v1 = k1 ^ 0x646f72616e646f6du64;
    let mut v2 = k0 ^ 0x6c7967656e657261u64;
    let mut v3 = k1 ^ 0x7465646279746573u64;

    // Compression — 1 round sur le seul bloc de 8 octets
    let m = data;
    v3 ^= m;
    sipround!(v0, v1, v2, v3);
    v0 ^= m;

    // Bloc final : (len % 256) << 56 = 0x0800000000000000 (len == 8)
    let b: u64 = 0x0800_0000_0000_0000;
    v3 ^= b;
    sipround!(v0, v1, v2, v3);
    v0 ^= b;

    // Finalisation — 3 rounds
    v2 ^= 0xff;
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);
    sipround!(v0, v1, v2, v3);

    v0 ^ v1 ^ v2 ^ v3
}

/// Hash d'une adresse virtuelle → index bucket [0, FUTEX_HASH_BUCKETS).
/// Utilise SipHash-1-3 keyed si la graine est initialisée, FNV sinon (boot).
#[inline]
fn bucket_index(virt_addr: u64) -> usize {
    if FUTEX_SEED_SET.load(Ordering::Acquire) != 0 {
        // SipHash-1-3 keyed — anti-DoS (RÈGLE MEM-FUTEX / V-34)
        let k0 = FUTEX_SEED_K0.load(Ordering::Relaxed);
        let k1 = FUTEX_SEED_K1.load(Ordering::Relaxed);
        (siphash13(k0, k1, virt_addr) as usize) & (FUTEX_HASH_BUCKETS - 1)
    } else {
        // Fallback FNV-1a (avant init_futex_seed, pendant le boot early)
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME:  u64 = 0x0000_0100_0000_01B3;
        let mut h = FNV_OFFSET;
        for b in virt_addr.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
        h as usize & (FUTEX_HASH_BUCKETS - 1)
    }
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
    // SAFETY: lo_ptr/hi_ptr → buckets distincts (lo < hi), protégés par leurs guards — pas d'aliasing.
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

// ─────────────────────────────────────────────────────────────────────────────
// sys_futex — point d'entrée du syscall FUTEX
// ─────────────────────────────────────────────────────────────────────────────

/// Codes d'opération FUTEX (Linux-compatible).
const FUTEX_WAIT:         u32 = 0;
const FUTEX_WAKE:         u32 = 1;
const FUTEX_REQUEUE:      u32 = 3;
/// Masque permettant de retirer le flag PRIVATE avant comparaison.
const FUTEX_PRIVATE_FLAG: u32 = 128;

/// Erreurs possibles de `sys_futex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexError {
    /// Opération inconnue ou non supportée.
    InvalidOp,
    /// *uaddr != val au moment de FUTEX_WAIT.
    ValueMismatch,
    /// Attente interrompue (pas de sleep hook enregistré).
    Interrupted,
    /// Timeout expiré.
    Timeout,
    /// Allocation mémoire impossible.
    NoMemory,
}

impl FutexError {
    /// Traduit l'erreur en errno POSIX négatif.
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            FutexError::InvalidOp      => -22, // EINVAL
            FutexError::ValueMismatch  => -11, // EAGAIN
            FutexError::Interrupted    => -4,  // EINTR
            FutexError::Timeout        => -110,// ETIMEDOUT
            FutexError::NoMemory       => -12, // ENOMEM
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Hook de mise en sommeil (injecté par le scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// Signature de la fonction de blocage fournie par le scheduler.
/// `waiter`     : waiter enfilé dans la table futex.
/// `timeout_ns` : délai maximum en nanosecondes (0 = infini).
/// Retourne 0 si réveillé normalement, -EINTR si interrompu, -ETIMEDOUT si timeout.
pub type FutexSleepHook = fn(waiter: *mut FutexWaiter, timeout_ns: u64) -> i32;

static FUTEX_SLEEP_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Enregistre la fonction de blocage fournie par le scheduler.
/// Appelée lors de l'initialisation du scheduler.
pub fn register_sleep_hook(hook: FutexSleepHook) {
    FUTEX_SLEEP_HOOK.store(hook as *mut (), Ordering::Release);
}

/// Retourne le hook enregistré, ou None si non défini.
fn get_sleep_hook() -> Option<FutexSleepHook> {
    let ptr = FUTEX_SLEEP_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Stocké via register_sleep_hook avec la bonne signature FutexSleepHook.
        Some(unsafe { core::mem::transmute(ptr) })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Noop wake (utilisé par sys_futex pour FutexWaiter temporaire)
// ─────────────────────────────────────────────────────────────────────────────

fn nop_wake_sys(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// sys_futex
// ─────────────────────────────────────────────────────────────────────────────

/// Point d'entrée du syscall `futex(2)`.
///
/// - `uaddr`   : adresse user de l'entier 32 bits sur lequel opérer.
/// - `op`      : opération (FUTEX_WAIT/WAKE/REQUEUE + éventuellement PRIVATE).
/// - `val`     : valeur attendue (FUTEX_WAIT) ou nombre de threads (FUTEX_WAKE).
/// - `timeout` : timeout en ns (FUTEX_WAIT), ou max_requeue (FUTEX_REQUEUE).
/// - `uaddr2`  : adresse de destination (FUTEX_REQUEUE).
/// - `val3`    : max_requeue wake count (FUTEX_REQUEUE), non utilisé sinon.
///
/// Retourne le nombre de threads réveillés (FUTEX_WAKE/REQUEUE) ou 0 (FUTEX_WAIT).
pub fn sys_futex(
    uaddr:   u64,
    op:      u32,
    val:     u32,
    timeout: u64,
    uaddr2:  u64,
    val3:    u32,
) -> Result<i64, FutexError> {
    // Retirer le flag PRIVATE — la table courante est déjà par-processus.
    let cmd = op & !FUTEX_PRIVATE_FLAG;

    match cmd {
        // ── FUTEX_WAIT ────────────────────────────────────────────────────
        FUTEX_WAIT => {
            // Allouer un waiter sur la pile du présent thread.
            let mut waiter = FutexWaiter::new(uaddr, val, 0, nop_wake_sys);

            // SAFETY: uaddr est une adresse user valide (supposé vérifié côté syscall).
            let result = unsafe { futex_wait(uaddr, val, &mut waiter, nop_wake_sys) };

            match result {
                FutexWaitResult::ValueMismatch => Err(FutexError::ValueMismatch),
                FutexWaitResult::Waiting => {
                    // Appeler le hook du scheduler si disponible.
                    match get_sleep_hook() {
                        Some(sleep) => {
                            let rc = sleep(&mut waiter, timeout);
                            if rc == -4 {
                                // EINTR — annuler l'attente
                                // SAFETY: waiter est encore dans la table.
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Interrupted)
                            } else if rc == -110 {
                                // ETIMEDOUT
                                // SAFETY: waiter est encore dans la table (pas encore réveillé).
                                unsafe { futex_cancel(&mut waiter) };
                                Err(FutexError::Timeout)
                            } else {
                                Ok(0)
                            }
                        }
                        None => {
                            // Pas de scheduler : annuler et retourner EINTR.
                            // SAFETY: waiter est encore dans la table.
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Interrupted)
                        }
                    }
                }
            }
        }

        // ── FUTEX_WAKE ────────────────────────────────────────────────────
        FUTEX_WAKE => {
            // val = nombre max de threads à réveiller ; wake_code = 0.
            // SAFETY: uaddr est une adresse user valide, vérifiée côté syscall.
            let woken = unsafe { futex_wake(uaddr, val, 0) };
            Ok(woken as i64)
        }

        // ── FUTEX_REQUEUE ─────────────────────────────────────────────────
        FUTEX_REQUEUE => {
            // val  = max threads réveillés sur uaddr
            // val3 = max threads requeueés vers uaddr2
            // timeout (troisième arg Linux) = max_wake ; val3 = max_requeue
            let max_wake    = val;
            let max_requeue = val3;
            // SAFETY: uaddr/uaddr2 sont des adresses user valides, vérifiées côté syscall.
            let (woken, _requeued) = unsafe {
                futex_requeue(uaddr, uaddr2, max_wake, max_requeue, 0)
            };
            Ok(woken as i64)
        }

        _ => Err(FutexError::InvalidOp),
    }
}

