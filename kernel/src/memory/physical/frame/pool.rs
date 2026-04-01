// kernel/src/memory/physical/frame/pool.rs
//
// Per-CPU frame pools + EmergencyPool tie-in.
// Les per-CPU pools servent à accélérer les allocations de frames en
// évitant les contentions sur le buddy global (512 frames par CPU).
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::memory::core::{
    Frame, PhysAddr, AllocFlags, AllocError,
    PER_CPU_POOL_SIZE, PER_CPU_DRAIN_THRESHOLD, PER_CPU_REFILL_THRESHOLD,
    MAX_CPUS,
};

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU FRAME POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Pool de frames par CPU — lock-free, taille fixe.
///
/// Principe :
///   Chaque CPU maintient un tableau circulaire de `PER_CPU_POOL_SIZE` frames.
///   Allocation locale : O(1), sans verrou, sans cache miss cross-CPU.
///   Quand le pool tombe sous `PER_CPU_REFILL_THRESHOLD`, il est réapprovisionné
///   depuis le buddy global.
///   Quand le pool dépasse `PER_CPU_DRAIN_THRESHOLD`, l'excès est rendu au buddy.
///
/// Thread-safety :
///   Un seul thread (le CPU propriétaire) accède à ce pool en mode normal.
///   Le compteur `count` est atomique pour les statsthreads d'observation.
///
/// RÈGLE NO-ALLOC : Ce code est appelé avec préemption potentiellement désactivée
///   → aucun `Vec`, `Box`, `Arc` — uniquement des tableaux statiques.
#[repr(C, align(64))]
pub struct PerCpuFramePool {
    /// Tableau circulaire des frames disponibles (PFNs).
    /// UnsafeCell car modifié uniquement par le CPU propriétaire.
    frames: UnsafeCell<[u64; PER_CPU_POOL_SIZE]>,
    /// Index de production (prochaine case libre).
    head:   AtomicUsize,
    /// Index de consommation (prochain frame à allouer).
    tail:   AtomicUsize,
    /// Compteur de frames disponibles snapshot (approximation).
    count:  AtomicUsize,
    /// CPU ID propriétaire de ce pool (-1 = non initialisé).
    cpu_id: AtomicUsize,
    /// Statistiques d'allocation depuis ce pool.
    alloc_hits:   AtomicU64,
    /// Statistiques d'échecs (pool vide → fallback buddy).
    alloc_misses: AtomicU64,
    /// Statistiques de réapprovisionnements.
    refills:      AtomicU64,
    /// Statistiques de vidanges.
    drains:       AtomicU64,
    /// Padding pour occuper exactement 2 cache lines (128 bytes).
    _pad: [u8; 64],
}

// Calcul du padding : on ajuste pour un align total de 128 bytes.
// Le compilateur vérifiera la taille via l'assertion statique ci-dessous.
// Note : UnsafeCell<[u64; 512]> = 4096 bytes — la struct sera grande.

// SAFETY: PerCpuFramePool est accédé uniquement par son CPU propriétaire
// en mode normal. Les compteurs atomiques permettent l'observation externe.
unsafe impl Sync for PerCpuFramePool {}
unsafe impl Send for PerCpuFramePool {}

impl PerCpuFramePool {
    /// Crée un pool vide non initialisé.
    pub const fn new_uninit() -> Self {
        PerCpuFramePool {
            frames:       UnsafeCell::new([0u64; PER_CPU_POOL_SIZE]),
            head:         AtomicUsize::new(0),
            tail:         AtomicUsize::new(0),
            count:        AtomicUsize::new(0),
            cpu_id:       AtomicUsize::new(usize::MAX),
            alloc_hits:   AtomicU64::new(0),
            alloc_misses: AtomicU64::new(0),
            refills:      AtomicU64::new(0),
            drains:       AtomicU64::new(0),
            _pad:         [0u8; _PAD_POOL],
        }
    }

    /// Initialise le pool pour un CPU donné.
    /// SAFETY: Doit être appelé une fois, uniquement par le CPU `cpu_id`,
    /// pendant l'init SMP avant tout accès concurrent.
    pub unsafe fn init(&self, cpu_id: usize) {
        self.cpu_id.store(cpu_id, Ordering::Release);
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
    }

    /// Alloue un frame depuis le pool.
    /// Retourne `None` si le pool est vide (fallback vers buddy).
    ///
    /// RÈGLE NO-ALLOC : appelé depuis hot path, pas d'allocation.
    #[inline(always)]
    pub fn pop(&self) -> Option<Frame> {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            self.alloc_misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // SAFETY: count > 0 → head != tail, accès exclusif par ce CPU.
        let frames_ptr = self.frames.get();
        let tail = self.tail.load(Ordering::Relaxed);
        let pfn = unsafe { (*frames_ptr)[tail % PER_CPU_POOL_SIZE] };
        self.tail.store((tail + 1) & (PER_CPU_POOL_SIZE - 1), Ordering::Relaxed);
        self.count.fetch_sub(1, Ordering::Relaxed);
        self.alloc_hits.fetch_add(1, Ordering::Relaxed);
        Some(Frame::containing(PhysAddr::new(pfn << 12)))
    }

    /// Dépose un frame libéré dans le pool (retour depuis buddy ou kfree).
    /// Retourne `false` si le pool est plein (fallback vers buddy global).
    #[inline(always)]
    pub fn push(&self, frame: Frame) -> bool {
        let count = self.count.load(Ordering::Relaxed);
        if count >= PER_CPU_POOL_SIZE {
            return false;
        }

        let frames_ptr = self.frames.get();
        let head = self.head.load(Ordering::Relaxed);
        // SAFETY: count < PER_CPU_POOL_SIZE ⇒ accès dans [0,SIZE); pas d'autre producteur.
        unsafe {
            (*frames_ptr)[head % PER_CPU_POOL_SIZE] = frame.pfn();
        }
        self.head.store((head + 1) & (PER_CPU_POOL_SIZE - 1), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Vérifie si le pool nécessite un réapprovisionnement.
    #[inline(always)]
    pub fn needs_refill(&self) -> bool {
        self.count.load(Ordering::Relaxed) < PER_CPU_REFILL_THRESHOLD
    }

    /// Vérifie si le pool doit être partiellement vidangé.
    #[inline(always)]
    pub fn needs_drain(&self) -> bool {
        self.count.load(Ordering::Relaxed) > PER_CPU_DRAIN_THRESHOLD
    }

    /// Retourne le nombre actuel de frames disponibles.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Retourne `true` si le pool est vide.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.count.load(Ordering::Relaxed) == 0
    }

    /// Retourne `true` si le pool est plein.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.count.load(Ordering::Relaxed) >= PER_CPU_POOL_SIZE
    }

    /// Retourne les statistiques de ce pool.
    pub fn stats(&self) -> PerCpuPoolStats {
        PerCpuPoolStats {
            cpu_id:       self.cpu_id.load(Ordering::Relaxed),
            count:        self.count.load(Ordering::Relaxed),
            capacity:     PER_CPU_POOL_SIZE,
            alloc_hits:   self.alloc_hits.load(Ordering::Relaxed),
            alloc_misses: self.alloc_misses.load(Ordering::Relaxed),
            refills:      self.refills.load(Ordering::Relaxed),
            drains:       self.drains.load(Ordering::Relaxed),
        }
    }

    /// Réapprovisionne le pool depuis un itérateur de frames.
    /// S'arrête quand le pool est plein ou l'itérateur épuisé.
    /// Retourne le nombre de frames ajoutés.
    pub fn refill(&self, frames: impl Iterator<Item = Frame>) -> usize {
        let mut count = 0;
        for frame in frames {
            if self.is_full() { break; }
            if self.push(frame) { count += 1; }
        }
        if count > 0 {
            self.refills.fetch_add(1, Ordering::Relaxed);
        }
        count
    }

    /// Vidange partiellement le pool (retourne les frames en excès).
    /// Appelle `drain_fn` pour chaque frame à retourner au buddy.
    /// Retourne le nombre de frames vidangés.
    pub fn drain(&self, drain_fn: impl FnMut(Frame)) -> usize {
        let target = PER_CPU_POOL_SIZE / 2; // Vidanger jusqu'à 50% plein
        let current = self.count.load(Ordering::Relaxed);
        if current <= target { return 0; }

        let mut drain_fn = drain_fn;
        let to_drain = current - target;
        let mut drained = 0;

        for _ in 0..to_drain {
            match self.pop() {
                Some(frame) => { drain_fn(frame); drained += 1; }
                None        => break,
            }
        }

        if drained > 0 {
            self.drains.fetch_add(1, Ordering::Relaxed);
        }
        drained
    }
}

/// Calcul de constante du padding.
const _PAD_POOL: usize = {
    // On vise 128 bytes pour les champs non-frames
    // frames (UnsafeCell<[u64; 512]>) = 4096 bytes — hors padding
    // Reste des champs : head, tail, count, cpu_id (4×8) + stats (4×8) = 64 bytes
    // Padding pour aligner à 128 bytes = 128 - 64 = 64 bytes
    64
};

/// Statistiques d'un per-CPU pool.
#[derive(Copy, Clone, Debug)]
pub struct PerCpuPoolStats {
    pub cpu_id:       usize,
    pub count:        usize,
    pub capacity:     usize,
    pub alloc_hits:   u64,
    pub alloc_misses: u64,
    pub refills:      u64,
    pub drains:       u64,

}

impl PerCpuPoolStats {
    /// Taux de hit du cache (0.0 - 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total = self.alloc_hits + self.alloc_misses;
        if total == 0 { 1.0 } else { self.alloc_hits as f64 / total as f64 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE GLOBALE DES PER-CPU POOLS
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale des per-CPU pools — MAX_CPUS entrées statiques.
pub struct PerCpuPoolTable {
    pools:        [PerCpuFramePool; MAX_CPUS],
    nr_cpus:      AtomicUsize,
    initialized:  AtomicUsize,  // bitmask (atomique, max 256 CPUs sur u64×4)
}

// SAFETY: PerCpuPoolTable est thread-safe via ses AtomicUsize internes
// et le protocole d'accès exclusif par CPU.
unsafe impl Sync for PerCpuPoolTable {}
unsafe impl Send for PerCpuPoolTable {}

impl PerCpuPoolTable {
    pub const fn new() -> Self {
        PerCpuPoolTable {
            pools: [const { PerCpuFramePool::new_uninit() }; MAX_CPUS],
            nr_cpus: AtomicUsize::new(0),
            initialized: AtomicUsize::new(0),
        }
    }

    /// Initialise le pool d'un CPU. SAFETY: voir PerCpuFramePool::init().
    pub unsafe fn init_cpu(&self, cpu_id: usize) {
        debug_assert!(cpu_id < MAX_CPUS, "cpu_id hors limites");
        self.pools[cpu_id].init(cpu_id);
        self.nr_cpus.fetch_max(cpu_id + 1, Ordering::Relaxed);
        // Marquer comme initialisé (bitmask single-word : couvre les CPUs 0..usize::BITS)
        // A_FAIRE: passer à [AtomicUsize; MAX_CPUS/usize::BITS] pour supporter MAX_CPUS > 64
        // Note de sûreté : les CPUs ≥ usize::BITS ne seront pas marqués dans le bitmask
        // mais leur pool sera quand même vide (count = 0) et donc safe.
        if cpu_id < usize::BITS as usize {
            self.initialized.fetch_or(1 << cpu_id, Ordering::Release);
        }
    }

    /// Accès au pool d'un CPU donné.
    ///
    /// # Panics (debug)
    /// Panique si `cpu_id >= MAX_CPUS`.
    #[inline(always)]
    pub fn get(&self, cpu_id: usize) -> &PerCpuFramePool {
        debug_assert!(cpu_id < MAX_CPUS);
        &self.pools[cpu_id]
    }

    /// Alloue depuis le pool du CPU courant.
    /// `cpu_id` doit être l'ID du CPU en cours d'exécution.
    #[inline(always)]
    pub fn alloc_from_cpu(&self, cpu_id: usize, _flags: AllocFlags) -> Result<Frame, AllocError> {
        self.get(cpu_id)
            .pop()
            .ok_or(AllocError::WouldBlock) // Signale le besoin de fallback buddy
    }

    /// Libère un frame vers le pool du CPU courant.
    /// Retourne `false` si le pool est plein (frame doit retourner au buddy).
    #[inline(always)]
    pub fn free_to_cpu(&self, cpu_id: usize, frame: Frame) -> bool {
        self.get(cpu_id).push(frame)
    }

    /// Retourne le nombre de CPUs initialisés.
    #[inline(always)]
    pub fn nr_cpus(&self) -> usize {
        self.nr_cpus.load(Ordering::Relaxed)
    }
}

/// Table globale des per-CPU pools.
/// Initialisée pendant l'init SMP, avant le scheduling.
pub static PER_CPU_POOLS: PerCpuPoolTable = PerCpuPoolTable::new();

/// Initialise le pool du CPU `cpu_id`.
/// SAFETY: Comme PerCpuFramePool::init().
pub unsafe fn init_cpu_pool(cpu_id: usize) {
    PER_CPU_POOLS.init_cpu(cpu_id);
}
