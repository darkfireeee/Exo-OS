// kernel/src/memory/physical/frame/emergency_pool.rs
//
// EmergencyPool — 64 WaitNodes pré-alloués au BOOT.
// RÈGLE EMERGENCY-01 : Initialisé AVANT tout autre module noyau.
//
// Raison d'être :
//   WaitQueue::wait() peut être appelé depuis un contexte de reclaim mémoire.
//   Si on alloue depuis le heap pendant reclaim → deadlock récursif.
//   Ce pool résout ce problème en fournissant des nœuds pré-alloués en
//   statique .bss, jamais libérés, jamais compactés.
//
// Utilisateurs légitimes : scheduler/sync/wait_queue.rs UNIQUEMENT.
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use crate::memory::core::constants::EMERGENCY_POOL_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// WAIT NODE — nœud de wait queue
// ─────────────────────────────────────────────────────────────────────────────

/// WaitNode — structure allouée depuis l'EmergencyPool pour les wait queues.
/// Doit rester aussi petite que possible (vise 64 bytes = 1 cache line).
#[repr(C, align(64))]
pub struct WaitNode {
    /// Thread ID en attente (0 = nœud libre).
    pub thread_id:  AtomicUsize,
    /// Prochain nœud dans la liste chaînée (null = fin).
    pub next:       AtomicUsize,   // *mut WaitNode comme usize
    /// Résultat de réveil (0 = signalé proprement, autre = erreur/timeout).
    pub wakeup_result: AtomicUsize,
    /// Timestamp d'entrée en attente (nanosecondes monotoniques, debug).
    pub enqueue_ts: AtomicUsize,
    /// Padding pour atteindre exactement 64 bytes.
    _pad: [u8; 64 - 4 * core::mem::size_of::<AtomicUsize>()],
}

const _: () = assert!(
    core::mem::size_of::<WaitNode>() == 64,
    "WaitNode doit faire exactement 64 bytes (1 cache line)"
);
const _: () = assert!(
    core::mem::align_of::<WaitNode>() == 64,
    "WaitNode doit être aligné sur 64 bytes"
);

impl WaitNode {
    /// Constante de valeur NULL pour le champ `next`.
    pub const NULL_PTR: usize = 0;

    /// Crée un WaitNode vide (thread_id = 0 = libre).
    #[inline(always)]
    pub const fn new_free() -> Self {
        WaitNode {
            thread_id:      AtomicUsize::new(0),
            next:           AtomicUsize::new(Self::NULL_PTR),
            wakeup_result:  AtomicUsize::new(0),
            enqueue_ts:     AtomicUsize::new(0),
            _pad:           [0u8; 64 - 4 * core::mem::size_of::<AtomicUsize>()],
        }
    }

    /// Vérifie si ce nœud est libre (non utilisé par une wait queue).
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.thread_id.load(Ordering::Acquire) == 0
    }

    /// Marque le nœud comme alloué pour le thread `tid`.
    /// Retourne false si le nœud était déjà alloué (échec CAS).
    #[inline(always)]
    pub fn try_acquire(&self, tid: usize) -> bool {
        debug_assert_ne!(tid, 0, "thread_id 0 est réservé pour 'libre'");
        self.thread_id
            .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Libère le nœud (remet thread_id à 0).
    /// SAFETY: L'appelant doit garantir que plus aucune wait queue
    ///         ne référence ce nœud.
    #[inline(always)]
    pub unsafe fn release(&self) {
        self.next.store(Self::NULL_PTR, Ordering::Release);
        self.wakeup_result.store(0, Ordering::Release);
        self.enqueue_ts.store(0, Ordering::Release);
        // Relâcher thread_id en dernier pour que is_free() soit cohérent
        self.thread_id.store(0, Ordering::Release);
    }

    /// Signale le thread en attente (réveille, code résultat = 0 = OK).
    #[inline(always)]
    pub fn signal_ok(&self) {
        self.wakeup_result.store(0, Ordering::Release);
    }

    /// Signale le thread avec un code d'erreur.
    #[inline(always)]
    pub fn signal_err(&self, code: usize) {
        self.wakeup_result.store(code, Ordering::Release);
    }

    /// Retourne le code de résultat du réveil.
    #[inline(always)]
    pub fn result(&self) -> usize {
        self.wakeup_result.load(Ordering::Acquire)
    }

    /// Retourne l'adresse de ce nœud comme usize (pour les linked-lists lock-free).
    #[inline(always)]
    pub fn as_usize_ptr(&self) -> usize {
        self as *const WaitNode as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EMERGENCY POOL — pool statique de WaitNodes
// ─────────────────────────────────────────────────────────────────────────────

/// Le pool d'urgence global — alloué dans .bss, initialisé au boot.
///
/// RÈGLE EMERGENCY-01 : `init()` DOIT être appelé AVANT tout autre module.
/// Si un appel à `acquire()` se produit avant `init()`, il panique en debug
/// et retourne None en release (anti-crash).
pub struct EmergencyPool {
    /// Tableau statique de WaitNodes — en .bss, jamais désalloué.
    nodes: UnsafeCell<[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE]>,
    /// Indicateur d'initialisation.
    initialized: AtomicBool,
    /// Compteur d'allocations actuelles (telémétrie).
    alloc_count: AtomicUsize,
    /// Pic d'allocations simultanées (high watermark — telémétrie).
    peak_alloc:  AtomicUsize,
    /// Compteur d'échecs d'allocation (pool épuisé).
    exhausted_count: AtomicUsize,
}

// SAFETY: EmergencyPool est thread-safe via ses AtomicBool/AtomicUsize internes.
// L'accès aux WaitNodes est protégé par le protocole CAS de try_acquire/release.
unsafe impl Sync for EmergencyPool {}
unsafe impl Send for EmergencyPool {}

impl EmergencyPool {
    /// Crée une instance non initialisée du pool.
    /// DOIT être placée dans une statique (UnsafeCell nécessite Sync).
    pub const fn new_uninit() -> Self {
        EmergencyPool {
            nodes:           UnsafeCell::new(
                // SAFETY: MaybeUninit::uninit() est safe en context const.
                [const { MaybeUninit::uninit() }; EMERGENCY_POOL_SIZE]
            ),
            initialized:     AtomicBool::new(false),
            alloc_count:     AtomicUsize::new(0),
            peak_alloc:      AtomicUsize::new(0),
            exhausted_count: AtomicUsize::new(0),
        }
    }

    /// Initialise le pool — DOIT être appelé au boot en premier.
    ///
    /// # Safety
    /// Doit être appelé une seule fois, depuis un seul CPU (avant l'init SMP),
    /// avant toute utilisation du pool. L'appel répété est no-op sécurisé.
    pub unsafe fn init(&self) {
        if self.initialized.load(Ordering::Acquire) {
            return; // Déjà initialisé — idempotent
        }

        let nodes_ptr = self.nodes.get();
        // SAFETY: Accès exclusif garanti par le protocole d'initialisation single-CPU.
        // Les MaybeUninit sont écrits une seule fois ici avant tout accès concurrent.
        for i in 0..EMERGENCY_POOL_SIZE {
            (*nodes_ptr)[i].write(WaitNode::new_free());
        }

        // Barrière de publication : tous les writes aux nodes sont visibles
        // avant que initialized passe à true.
        self.initialized.store(true, Ordering::Release);
    }

    /// Alloue un WaitNode libre du pool pour le thread `thread_id`.
    ///
    /// Retourne `None` si le pool est épuisé (toujours O(n) au pire mais
    /// n=64, linéaire acceptable pour un path non-hot).
    ///
    /// # Panics
    /// En mode debug, panique si le pool n'est pas initialisé.
    pub fn acquire(&self, thread_id: usize) -> Option<&WaitNode> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "EmergencyPool::acquire() appelé avant init() — RÈGLE EMERGENCY-01 violée"
        );

        if !self.initialized.load(Ordering::Acquire) {
            return None;
        }

        let nodes_ptr = self.nodes.get();
        let nodes: &[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE] =
            // SAFETY: Après init(), tous les WaitNode sont initialisés (barrière Release).
            unsafe { &*nodes_ptr };

        for node_uninit in nodes.iter() {
            // SAFETY: Tous les nœuds ont été initialisés dans init().
            let node: &WaitNode = unsafe { node_uninit.assume_init_ref() };
            if node.try_acquire(thread_id) {
                // Mise à jour du compteur et du peak atomiquement
                let prev = self.alloc_count.fetch_add(1, Ordering::Relaxed);
                let new_count = prev + 1;
                // Mettre à jour le peak si nécessaire (best-effort, pas de garantie exacte)
                let mut peak = self.peak_alloc.load(Ordering::Relaxed);
                while new_count > peak {
                    match self.peak_alloc.compare_exchange_weak(
                        peak, new_count, Ordering::Relaxed, Ordering::Relaxed
                    ) {
                        Ok(_) => break,
                        Err(p) => peak = p,
                    }
                }
                return Some(node);
            }
        }

        // Pool épuisé
        self.exhausted_count.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Libère un WaitNode précédemment acquis.
    ///
    /// # Safety
    /// Le nœud DOIT avoir été obtenu via `acquire()` depuis ce pool.
    /// Plus aucune wait queue ne doit référencer ce nœud au moment de l'appel.
    pub unsafe fn release(&self, node: &WaitNode) {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "EmergencyPool::release() appelé avant init()"
        );
        // Vérifier que le nœud appartient bien à ce pool
        debug_assert!(
            self.owns(node),
            "EmergencyPool::release() : nœud ne vient pas de ce pool"
        );

        node.release();
        self.alloc_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Vérifie si un `WaitNode` appartient à ce pool.
    #[inline]
    pub fn owns(&self, node: &WaitNode) -> bool {
        let pool_start = self.nodes.get() as usize;
        let pool_end   = pool_start + core::mem::size_of::<[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE]>();
        let node_addr  = node as *const WaitNode as usize;
        node_addr >= pool_start && node_addr < pool_end
    }

    /// Retourne les statistiques du pool (telémétrie).
    #[inline]
    pub fn stats(&self) -> EmergencyPoolStats {
        EmergencyPoolStats {
            capacity:        EMERGENCY_POOL_SIZE,
            allocated:       self.alloc_count.load(Ordering::Relaxed),
            peak_allocated:  self.peak_alloc.load(Ordering::Relaxed),
            exhausted_count: self.exhausted_count.load(Ordering::Relaxed),
            initialized:     self.initialized.load(Ordering::Relaxed),
        }
    }

    /// Retourne le nombre de nœuds actuellement alloués.
    #[inline(always)]
    pub fn allocated_count(&self) -> usize {
        self.alloc_count.load(Ordering::Relaxed)
    }

    /// Retourne `true` si le pool est initialisé.
    #[inline(always)]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Retourne `true` si le pool est épuisé (plus aucun nœud libre).
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.alloc_count.load(Ordering::Relaxed) >= EMERGENCY_POOL_SIZE
    }

    /// Compte les nœuds libres disponibles (O(n) — utiliser uniquement pour diagnostics).
    pub fn free_count(&self) -> usize {
        if !self.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let nodes_ptr = self.nodes.get();
        // SAFETY: Après init(), tous les nœuds sont initialisés.
        let nodes: &[MaybeUninit<WaitNode>; EMERGENCY_POOL_SIZE] =
            // SAFETY: nodes_ptr valide post-init(); accès immutable.
            unsafe { &*nodes_ptr };
        nodes.iter()
            .map(|n| unsafe { n.assume_init_ref() })
            .filter(|n| n.is_free())
            .count()
    }
}

/// Statistiques de l'EmergencyPool pour la télémétrie/debug.
#[derive(Copy, Clone, Debug)]
pub struct EmergencyPoolStats {
    pub capacity:        usize,
    pub allocated:       usize,
    pub peak_allocated:  usize,
    pub exhausted_count: usize,
    pub initialized:     bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// INSTANCE GLOBALE
// ─────────────────────────────────────────────────────────────────────────────

/// Pool d'urgence global — DOIT être initialisé en premier au boot.
///
/// RÈGLE EMERGENCY-01 : Appeler `EMERGENCY_POOL.init()` AVANT tout autre module.
/// Voir le commentaire de module pour le raisonnement anti-deadlock.
pub static EMERGENCY_POOL: EmergencyPool = EmergencyPool::new_uninit();

/// Initialise le pool d'urgence ET le SchedNodePool.
/// SAFETY: Comme `EmergencyPool::init()` — voir sa documentation.
///
/// Doit être le PREMIER appel d'initialisation du kernel dans `kernel_init()`.
pub unsafe fn init() {
    EMERGENCY_POOL.init();
    // Initialise le SchedNodePool (bitmap externe → tous blocs libres).
    // RÈGLE WAITQ-01 : disponible avant toute création de wait queue.
    init_sched_pool();
}

/// Alloue un WaitNode depuis le pool global.
/// Retourne `None` si le pool est épuisé (critic situation, kernel warning).
#[inline]
pub fn acquire(thread_id: usize) -> Option<&'static WaitNode> {
    EMERGENCY_POOL.acquire(thread_id)
}

/// Libère un WaitNode vers le pool global.
/// SAFETY: Voir `EmergencyPool::release()`.
#[inline]
pub unsafe fn release(node: &'static WaitNode) {
    EMERGENCY_POOL.release(node)
}

/// Retourne les statistiques du pool global.
#[inline]
pub fn stats() -> EmergencyPoolStats {
    EMERGENCY_POOL.stats()
}

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS STATIQUES
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    EMERGENCY_POOL_SIZE >= 64,
    "EMERGENCY_POOL_SIZE < 64 — risque de deadlock dans wait_queue"
);
const _: () = assert!(
    core::mem::size_of::<WaitNode>() == 64,
    "WaitNode doit faire 64 bytes"
);
const _: () = assert!(
    core::mem::align_of::<WaitNode>() == 64,
    "WaitNode doit être aligné sur 64 bytes"
);

// ═════════════════════════════════════════════════════════════════════════════
// SCHED NODE POOL — pool de blocs bruts pour scheduler/sync/wait_queue.rs
// ═════════════════════════════════════════════════════════════════════════════
//
// Problème architectural :
//   • EmergencyPool::WaitNode  utilise (thread_id: AtomicUsize) au byte 0
//     pour gérer son propre état libre/occupé via CAS.
//   • scheduler::sync::wait_queue::WaitNode  place (tcb: *mut TCB) au byte 0.
//   • Ces deux définitions sont INCOMPATIBLES : écrire le TCB corrompt
//     le champ de suivi de l'EmergencyPool.
//
// Solution :
//   SchedNodePool gère 64 blocs de 64 bytes alignés 64 via un BITSET EXTERNE
//   (AtomicU64) — le pool ne touche JAMAIS au contenu des blocs.
//   Le scheduler possède la totalité du bloc et y écrit sa propre structure.
//
// CAS lock-free O(1) pour alloc/free via trailing_zeros sur le bitset.
// Initialisation : tous les bits à 1 = tous les blocs libres.
//
// RÈGLE WAITQ-01 : les blocs sont pré-alloués en .bss, zéro allocation heap.
// ═════════════════════════════════════════════════════════════════════════════

/// Nombre de blocs dans le SchedNodePool (doit être ≤ 64 pour le bitset u64).
const SCHED_POOL_SIZE: usize = 64;

/// Un bloc de mémoire opaque de 64 bytes aligné 64 — utilisé par le scheduler
/// pour y construire son propre WaitNode (#[repr(C)]).
#[repr(C, align(64))]
struct RawBlock64 {
    data: [u8; 64],
}

const _: () = assert!(
    core::mem::size_of::<RawBlock64>() == 64,
    "RawBlock64 doit faire exactement 64 bytes"
);
const _: () = assert!(
    core::mem::align_of::<RawBlock64>() == 64,
    "RawBlock64 doit être aligné 64 bytes"
);

/// Pool de blocs bruts pour scheduler/sync/wait_queue.rs.
///
/// Utilise un bitmap AtomicU64 pour le suivi (bit N=1 ↔ bloc N libre).
/// Dépendance zéro vers le scheduler ou les types du scheduler.
struct SchedNodePool {
    /// 64 blocs de 64 bytes alignés — en .bss, jamais désalloués.
    blocks:        UnsafeCell<[RawBlock64; SCHED_POOL_SIZE]>,
    /// Bitset libre : bit N = 1 → bloc N disponible.
    /// Initialisé à 0xFFFF_FFFF_FFFF_FFFF (tous libres).
    free_bits:     AtomicU64,
    initialized:   AtomicBool,
    /// Compteur d'allocations actives (diagnostic).
    alloc_count:   AtomicUsize,
    /// Compteur de fois où le pool était épuisé (diagnostic).
    exhausted:     AtomicUsize,
}

unsafe impl Sync for SchedNodePool {}
unsafe impl Send for SchedNodePool {}

impl SchedNodePool {
    const fn new_uninit() -> Self {
        SchedNodePool {
            // SAFETY: RawBlock64 = [u8; 64], tous-zéros valide + pas de drop.
            blocks:      UnsafeCell::new(unsafe { core::mem::zeroed() }),
            free_bits:   AtomicU64::new(0),   // sera mis à !0 dans init()
            initialized: AtomicBool::new(false),
            alloc_count: AtomicUsize::new(0),
            exhausted:   AtomicUsize::new(0),
        }
    }

    /// Initialise le pool (idempotent).
    ///
    /// # Safety
    /// Appelé depuis un seul CPU avant tout accès concurrent.
    unsafe fn init(&self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }
        // Marquer tous les blocs comme libres (64 blocs → 64 bits à 1)
        self.free_bits.store(u64::MAX, Ordering::Release);
        self.initialized.store(true, Ordering::Release);
    }

    /// Alloue un bloc de 64 bytes, retourne null si épuisé.
    ///
    /// CAS lock-free via trailing_zeros sur le bitset — O(1) armortized.
    fn alloc(&self) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }
        loop {
            let bits = self.free_bits.load(Ordering::Acquire);
            if bits == 0 {
                self.exhausted.fetch_add(1, Ordering::Relaxed);
                return core::ptr::null_mut();
            }
            let idx = bits.trailing_zeros() as usize;
            let new_bits = bits & !(1u64 << idx);
            match self.free_bits.compare_exchange_weak(
                bits, new_bits, Ordering::AcqRel, Ordering::Acquire
            ) {
                Ok(_) => {
                    self.alloc_count.fetch_add(1, Ordering::Relaxed);
                    // SAFETY: idx < SCHED_POOL_SIZE, blocks initialisé.
                    let blocks_ptr = self.blocks.get();
                    return unsafe { (*blocks_ptr)[idx].data.as_mut_ptr() };
                }
                Err(_) => continue,  // retry CAS
            }
        }
    }

    /// Libère un bloc précédemment alloué via `alloc()`.
    ///
    /// # Safety
    /// `ptr` doit être un pointeur retourné par `alloc()` de ce pool.
    unsafe fn free(&self, ptr: *mut u8) {
        if ptr.is_null() { return; }
        let blocks_base = unsafe { (*self.blocks.get()).as_ptr() as usize };
        let ptr_addr    = ptr as usize;
        // Vérification de borne (défensif)
        let pool_size   = core::mem::size_of::<[RawBlock64; SCHED_POOL_SIZE]>();
        if ptr_addr < blocks_base || ptr_addr >= blocks_base + pool_size {
            debug_assert!(false, "SchedNodePool::free() — pointeur hors bornes");
            return;
        }
        let offset = ptr_addr - blocks_base;
        let idx    = offset / 64;
        // Remettre le bit à 1 (bloc libre) de façon atomique
        self.free_bits.fetch_or(1u64 << idx, Ordering::Release);
        self.alloc_count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Instance globale du SchedNodePool.
static SCHED_NODE_POOL: SchedNodePool = SchedNodePool::new_uninit();

/// Initialise le SchedNodePool (appelé depuis memory::init() après EmergencyPool).
///
/// # Safety
/// Appelé une seule fois depuis le BSP avant tout usage du scheduler.
pub unsafe fn init_sched_pool() {
    SCHED_NODE_POOL.init();
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORTS — scheduler/sync/wait_queue.rs interface
// ─────────────────────────────────────────────────────────────────────────────
//
// Ces fonctions `#[no_mangle] extern "C"` sont le pont FFI entre
// scheduler/sync/wait_queue.rs (RÈGLE WAITQ-01 DOC3) et ce module.
//
// scheduler/ est Couche 1 et ne peut pas importer directement memory/.
// La séparation est préservée : scheduler/ reçoit un pointeur opaque brut
// qu'il cast vers sa propre définition de WaitNode.
//
// SYNCHRONISATION : si la taille du scheduler::WaitNode change au-delà de
// 64 bytes, SCHED_POOL_SIZE ou la taille de RawBlock64 devront être augmentés.
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue un bloc de 64 bytes aligné de l'EmergencyPool pour le scheduler.
///
/// Retourne `null` si le pool est épuisé.
///
/// ## Utilisateur
/// `scheduler::sync::wait_queue::WaitNode::alloc()` — RÈGLE WAITQ-01.
///
/// ## Layout garanti
/// Le bloc retourné fait exactement 64 bytes, aligné sur 64 bytes.
/// Le scheduler y écrit sa propre struct WaitNode (#[repr(C)], ≤ 64 bytes).
///
/// # Safety
/// Appelé depuis code Ring 0, préemption désactivée recommandée.
/// Le pointeur retourné est valide jusqu'à `emergency_pool_free_wait_node()`.
#[no_mangle]
pub unsafe extern "C" fn emergency_pool_alloc_wait_node() -> *mut u8 {
    SCHED_NODE_POOL.alloc()
}

/// Libère un bloc précédemment alloué par `emergency_pool_alloc_wait_node()`.
///
/// ## Utilisateur
/// `scheduler::sync::wait_queue::WaitNode::free()` — RÈGLE WAITQ-01.
///
/// # Safety
/// `node` doit être un pointeur retourné par `emergency_pool_alloc_wait_node()`.
/// Le bloc ne doit plus être dans aucune liste de wait queue.
#[no_mangle]
pub unsafe extern "C" fn emergency_pool_free_wait_node(node: *mut u8) {
    SCHED_NODE_POOL.free(node);
}
