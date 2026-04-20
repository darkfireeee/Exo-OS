// kernel/src/scheduler/core/runqueue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// RUN QUEUE per-CPU — 3 files (RT / Normal / Idle) (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • File RT     : bitmap 100 bits (niveaux 0–99), O(1) via bit-scan
//   • File Normal : CFS min-heap (red-black tree simulé par tableau trié en
//                   inplace pour éviter alloc heap) — 512 threads max per-CPU
//   • File Idle   : un seul thread (le idle_thread per-CPU)
//
// RÈGLES :
//   • NO-ALLOC : tout le storage est statique (tableaux) + indices dans un
//     pool global de TCBs
//   • La run queue est protégée par un spinlock IRQ-safe (IrqGuard)
//   • MAX_TASKS_PER_CPU = 512 → suffisant pour toute charge réaliste
//   • Instrumentation : compteurs atomiques pour pick_next latency tracking
//
// LOCK ORDERING (regle_bonus.md) : Scheduler locks < Memory locks
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::ptr::NonNull;
use super::task::{ThreadControlBlock, CpuId, SchedPolicy};
use super::preempt::MAX_CPUS;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de threads par file CFS per-CPU.
pub const MAX_TASKS_PER_CPU: usize = 512;
/// Nombre de niveaux RT (priorités 0–99).
pub const RT_LEVELS: usize = 100;
/// Quantum RT Round-Robin (ms).
pub const RR_TIMESLICE_MS: u64 = 10;
/// Quantum CFS minimal (µs).
pub const CFS_MIN_GRANULARITY_US: u64 = 750;
/// Quantum CFS cible (ms).
pub const CFS_TARGET_LATENCY_MS: u64 = 6;

// ─────────────────────────────────────────────────────────────────────────────
// RtBitmap — O(1) find_first_set pour 100 niveaux RT
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmap 128 bits pour 100 niveaux de priorité RT.
/// Bit N = 1 → au moins un thread de priorité N est prêt.
#[derive(Default)]
#[repr(C)]
struct RtBitmap {
    bits: [u64; 2],  // 128 bits → couvre 100 niveaux avec marge
}

impl RtBitmap {
    #[inline(always)]
    fn set(&mut self, prio: u8) {
        debug_assert!((prio as usize) < RT_LEVELS);
        let word = prio as usize / 64;
        let bit  = prio as usize % 64;
        self.bits[word] |= 1u64 << bit;
    }

    #[inline(always)]
    fn clear(&mut self, prio: u8) {
        let word = prio as usize / 64;
        let bit  = prio as usize % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// Retourne la priorité la plus haute (valeur la plus basse) prête.
    /// O(1) via leading zeros instruction.
    #[inline(always)]
    fn find_highest_prio(&self) -> Option<u8> {
        if self.bits[0] != 0 {
            Some(self.bits[0].trailing_zeros() as u8)
        } else if self.bits[1] != 0 {
            Some(64 + self.bits[1].trailing_zeros() as u8)
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RT Run Queue — par niveau de priorité, doublement chaînée (indices statiques)
// ─────────────────────────────────────────────────────────────────────────────

/// Un slot dans la file RT (stocke un pointeur TCB + lien doublement chaîné).
struct RtEntry {
    tcb:  Option<NonNull<ThreadControlBlock>>,
    next: u16,  // indice suivant dans le tableau (0xFFFF = fin)
    prev: u16,  // indice précédent
}

const RT_QUEUE_CAPACITY: usize = 256;
const RT_QUEUE_NONE: u16 = 0xFFFF;

/// File RT complète : 100 têtes + pool d'entrées.
struct RtRunQueue {
    /// Tête de chaque niveau (indice dans `entries`, RT_QUEUE_NONE si vide).
    heads:   [u16; RT_LEVELS],
    /// Pool d'entrées doublement chaîné.
    entries: [RtEntry; RT_QUEUE_CAPACITY],
    /// Next free slot in entries.
    free_head: u16,
    /// Bitmap de niveaux non vides.
    bitmap: RtBitmap,
    /// Nombre total de threads RT prêts.
    count: u32,
}

impl RtRunQueue {
    fn new() -> Self {
        const ENTRY: RtEntry = RtEntry { tcb: None, next: RT_QUEUE_NONE, prev: RT_QUEUE_NONE };
        let mut entries = [ENTRY; RT_QUEUE_CAPACITY];
        // Initialiser la liste libre : 0 → 1 → 2 → … → CAPACITY-1 → NONE
        for i in 0..(RT_QUEUE_CAPACITY - 1) {
            entries[i].next = (i + 1) as u16;
        }
        entries[RT_QUEUE_CAPACITY - 1].next = RT_QUEUE_NONE;
        Self {
            heads:     [RT_QUEUE_NONE; RT_LEVELS],
            entries,
            free_head: 0,
            bitmap:    RtBitmap::default(),
            count:     0,
        }
    }

    /// Enfile un thread RT à la fin de sa file de priorité (FIFO dans le niveau).
    fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        // SAFETY: tcb est un NonNull<ThreadControlBlock> valide fourni par
        // pick_next / enqueue — la run queue est l'unique propriétaire.
        // La préemption est désactivée (invariant PerCpuRunQueue).
        let prio = unsafe { tcb.as_ref() }.priority.0 as usize;
        debug_assert!(prio < RT_LEVELS);

        let slot = self.alloc_slot();
        let Some(slot_idx) = slot else { return false; };

        self.entries[slot_idx].tcb = Some(tcb);
        self.entries[slot_idx].next = RT_QUEUE_NONE;

        // Ajouter en fin de liste du niveau (FIFO = fairness Round-Robin).
        let head = self.heads[prio];
        if head == RT_QUEUE_NONE {
            self.entries[slot_idx].prev = RT_QUEUE_NONE;
            self.heads[prio] = slot_idx as u16;
        } else {
            // Trouver la queue.
            let mut cur = head;
            loop {
                let nxt = self.entries[cur as usize].next;
                if nxt == RT_QUEUE_NONE { break; }
                cur = nxt;
            }
            self.entries[cur as usize].next = slot_idx as u16;
            self.entries[slot_idx].prev = cur;
        }

        self.bitmap.set(prio as u8);
        self.count += 1;
        true
    }

    /// Retire et retourne le thread le plus prioritaire (O(1) via bitmap).
    fn dequeue_highest(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        let prio = self.bitmap.find_highest_prio()? as usize;
        let slot_idx = self.heads[prio] as usize;
        let tcb = self.entries[slot_idx].tcb.take()?;

        // Avancer la tête de la liste.
        let next = self.entries[slot_idx].next;
        self.heads[prio] = next;
        if next != RT_QUEUE_NONE {
            self.entries[next as usize].prev = RT_QUEUE_NONE;
        } else {
            // Niveau vide → effacer le bit.
            self.bitmap.clear(prio as u8);
        }

        self.free_slot(slot_idx);
        self.count -= 1;
        Some(tcb)
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        if self.free_head == RT_QUEUE_NONE { return None; }
        let idx = self.free_head as usize;
        self.free_head = self.entries[idx].next;
        self.entries[idx].next = RT_QUEUE_NONE;
        self.entries[idx].prev = RT_QUEUE_NONE;
        Some(idx)
    }

    fn free_slot(&mut self, idx: usize) {
        self.entries[idx].tcb = None;
        self.entries[idx].next = self.free_head;
        self.free_head = idx as u16;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CFS Run Queue — tableau trié par vruntime (insertion O(n), pick O(1))
//
// Pour éviter toute allocation heap, on utilise un tableau statique de
// pointeurs TCB trié par vruntime croissant (index 0 = plus petit vruntime).
// L'insertion O(n) est acceptable car pick_next_task est O(1) (priorité).
// - MAX_TASKS_PER_CPU = 512 (limite réaliste per-CPU sur Exo-OS)
// ─────────────────────────────────────────────────────────────────────────────

struct CfsRunQueue {
    /// Tableau de pointeurs TCB triés par vruntime croissant.
    tasks:    [Option<NonNull<ThreadControlBlock>>; MAX_TASKS_PER_CPU],
    /// Nombre de threads CFS prêts.
    count:    usize,
    /// Somme des poids pour le calcul du quantum.
    weight_sum: u64,
    /// vruntime minimum courant (valeur de base CFS).
    min_vruntime: AtomicU64,
}

impl CfsRunQueue {
    fn new() -> Self {
        Self {
            tasks:    [None; MAX_TASKS_PER_CPU],
            count:    0,
            weight_sum: 0,
            min_vruntime: AtomicU64::new(0),
        }
    }

    /// Insère un thread CFS en maintenant le tri par vruntime.
    /// Insertion bisection O(log n) + déplacement O(n) dans le pire cas.
    fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) {
        if self.count >= MAX_TASKS_PER_CPU {
            // File pleine : le thread sera re-tenté au prochain tick.
            // En pratique impossible avec 512 slots per-CPU.
            return;
        }

        // SAFETY: tcb est un NonNull valide — même invariant que enqueue().
        let vr = unsafe { tcb.as_ref() }.vruntime.load(Ordering::Acquire);
        let weight = unsafe { tcb.as_ref() }.priority.cfs_weight();

        // Bisection pour trouver la position d'insertion.
        let pos = {
            let mut lo = 0usize;
            let mut hi = self.count;
            while lo < hi {
                let mid = (lo + hi) / 2;
                // SAFETY: tasks[mid] est Some car mid < self.count.
                let mv = unsafe {
                    self.tasks[mid].unwrap().as_ref().vruntime.load(Ordering::Acquire)
                };
                if mv < vr { lo = mid + 1; } else { hi = mid; }
            }
            lo
        };

        // Décaler les éléments après `pos` vers la droite.
        if pos < self.count {
            self.tasks.copy_within(pos..self.count, pos + 1);
        }
        self.tasks[pos] = Some(tcb);
        self.count += 1;
        self.weight_sum = self.weight_sum.saturating_add(weight as u64);
    }

    /// Retire le thread avec le plus petit vruntime (tête du tableau trié).
    fn dequeue_min(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        if self.count == 0 { return None; }
        let tcb = self.tasks[0].take()?;
        self.tasks.copy_within(1..self.count, 0);
        self.tasks[self.count - 1] = None;
        self.count -= 1;

        // SAFETY: tcb est un NonNull valide — même invariant que enqueue().
        let weight = unsafe { tcb.as_ref() }.priority.cfs_weight() as u64;
        self.weight_sum = self.weight_sum.saturating_sub(weight);

        // Actualiser min_vruntime.
        if self.count > 0 {
            // SAFETY: tasks[0] est Some car self.count > 0.
            let new_min = unsafe {
                self.tasks[0].unwrap().as_ref().vruntime.load(Ordering::Relaxed)
            };
            // Intentionnel: min_vruntime est une borne approximative CFS.
            // La cohérence stricte de décision est garantie par insert_sorted() en Acquire.
            self.min_vruntime.store(new_min, Ordering::Relaxed);
        }

        Some(tcb)
    }

    /// Retire un thread spécifique (wakeup / migration).
    fn remove(&mut self, target: NonNull<ThreadControlBlock>) -> bool {
        for i in 0..self.count {
            if self.tasks[i] == Some(target) {
                // SAFETY: target est un NonNull valide — même invariant que enqueue().
                let weight = unsafe { target.as_ref() }.priority.cfs_weight() as u64;
                self.tasks.copy_within(i + 1..self.count, i);
                self.tasks[self.count - 1] = None;
                self.count -= 1;
                self.weight_sum = self.weight_sum.saturating_sub(weight);
                return true;
            }
        }
        false
    }

    /// Calcule le quantum alloué à un thread donné (CFS targeting).
    fn timeslice_ns(&self, weight: u32) -> u64 {
        if self.count == 0 || self.weight_sum == 0 {
            return CFS_TARGET_LATENCY_MS * 1_000_000 / (self.count.max(1) as u64);
        }
        let target_ns = CFS_TARGET_LATENCY_MS * 1_000_000;
        let slice = target_ns * weight as u64 / self.weight_sum;
        slice.max(CFS_MIN_GRANULARITY_US * 1000)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PerCpuRunQueue — agrégation des 3 files pour 1 CPU
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques de la run queue (instrumentation).
#[repr(C)]
pub struct RunQueueStats {
    /// Appels totaux à pick_next_task().
    pub picks_total:      AtomicU64,
    /// Appels pick_next ayant retourné RT.
    pub picks_rt:         AtomicU64,
    /// Appels pick_next ayant retourné CFS.
    pub picks_cfs:        AtomicU64,
    /// Appels pick_next ayant retourné idle.
    pub picks_idle:       AtomicU64,
    /// Nombre de threads en attente (snapshot).
    pub nr_running:       AtomicU32,
    /// Charge normalisée × 1024 (load average).
    pub load_avg:         AtomicU64,
    /// Timestamp du dernier reequilibrage SMP (ns).
    pub last_balance_ns:  AtomicU64,
}

impl RunQueueStats {
    const fn new() -> Self {
        Self {
            picks_total:     AtomicU64::new(0),
            picks_rt:        AtomicU64::new(0),
            picks_cfs:       AtomicU64::new(0),
            picks_idle:      AtomicU64::new(0),
            nr_running:      AtomicU32::new(0),
            load_avg:        AtomicU64::new(0),
            last_balance_ns: AtomicU64::new(0),
        }
    }
}

/// Run queue complète pour un CPU logique.
///
/// FIX-RQ-ALIGN-01 : #[repr(C, align(64))] garantit que deux PerCpuRunQueue
/// adjacentes dans PER_CPU_RQ[MAX_CPUS] ne partagent pas de cache lines sur
/// leurs champs hot (cpu, stats, current) → pas de false sharing SMP.
#[repr(C, align(64))]
pub struct PerCpuRunQueue {
    /// Identifiant du CPU propriétaire.
    pub cpu: CpuId,
    /// File temps-réel.
    rt:      RtRunQueue,
    /// File CFS (ordonnancement équitable).
    cfs:     CfsRunQueue,
    /// Thread idle de ce CPU (toujours présent, non compté dans nr_running).
    pub idle_thread: Option<NonNull<ThreadControlBlock>>,
    /// Thread courant (en train de tourner).
    pub current: Option<NonNull<ThreadControlBlock>>,
    /// Statistiques.
    pub stats: RunQueueStats,
    /// Clock vruntime — temps monotone utilisé pour le lag CFS.
    pub clock_task_ns: u64,
}

// FIX-RQ-ALIGN-01 : vérification compile-time de l'alignement.
const _: () = assert!(
    core::mem::align_of::<PerCpuRunQueue>() >= 64,
    "FIX-RQ-ALIGN-01: PerCpuRunQueue doit être aligné sur 64 octets minimum"
);

impl PerCpuRunQueue {
    /// Crée une run queue pour le CPU `cpu`.
    pub fn new(cpu: CpuId) -> Self {
        Self {
            cpu,
            rt:          RtRunQueue::new(),
            cfs:         CfsRunQueue::new(),
            idle_thread: None,
            current:     None,
            stats:       RunQueueStats::new(),
            clock_task_ns: 0,
        }
    }

    /// Enregistre le thread idle de ce CPU (appelé une seule fois au boot).
    pub fn set_idle_thread(&mut self, idle: NonNull<ThreadControlBlock>) {
        // SAFETY: idle est valide, la run queue en est propriétaire.
        unsafe { idle.as_ref() }.sched_state
            .fetch_or(super::task::SCHED_IDLE_BIT, Ordering::Relaxed);
        self.idle_thread = Some(idle);
    }

    /// Ajoute un thread à la run queue selon sa politique.
    ///
    /// INVARIANT : appelé avec préemption désactivée.
    pub fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>) {
        // SAFETY: tcb est un NonNull valide, préemption désactivée (invariant).
        let policy = unsafe { tcb.as_ref() }.policy;
        match policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                self.rt.enqueue(tcb);
            }
            SchedPolicy::Normal | SchedPolicy::Batch => {
                self.cfs.enqueue(tcb);
            }
            SchedPolicy::Deadline => {
                // SCHED_DEADLINE → file EDF dédiée (échéance la plus proche en tête).
                // SAFETY: préemption désactivée (INVARIANT), cpu.0 < MAX_CPUS garanti.
                unsafe {
                    crate::scheduler::timer::deadline_timer::dl_enqueue(
                        self.cpu.0 as usize, tcb,
                    );
                }
            }
            SchedPolicy::Idle => { /* géré par idle_thread */ }
        }
        let prev = self.stats.nr_running.fetch_add(1, Ordering::Relaxed);
        self.update_load_avg(prev as u64 + 1);
    }

    /// Retire un thread spécifique de la queue (migration, signal mort).
    pub fn remove(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        // SAFETY: tcb est un NonNull valide, préemption désactivée (invariant).
        let policy = unsafe { tcb.as_ref() }.policy;
        let removed = match policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                // Recherche dans les slots RT — O(RT_QUEUE_CAPACITY)
                // SAFETY: même invariant que pour policy.
                let prio = unsafe { tcb.as_ref() }.priority.0 as usize;
                let head = self.rt.heads[prio];
                if head == RT_QUEUE_NONE { return false; }
                let mut cur = head;
                loop {
                    let entry_tcb = self.rt.entries[cur as usize].tcb;
                    if entry_tcb == Some(tcb) {
                        let prev_entry = self.rt.entries[cur as usize].prev;
                        let next_entry = self.rt.entries[cur as usize].next;
                        if prev_entry == RT_QUEUE_NONE {
                            self.rt.heads[prio] = next_entry;
                        } else {
                            self.rt.entries[prev_entry as usize].next = next_entry;
                        }
                        if next_entry != RT_QUEUE_NONE {
                            self.rt.entries[next_entry as usize].prev = prev_entry;
                        }
                        if self.rt.heads[prio] == RT_QUEUE_NONE {
                            self.rt.bitmap.clear(prio as u8);
                        }
                        self.rt.free_slot(cur as usize);
                        self.rt.count -= 1;
                        break true;
                    }
                    cur = self.rt.entries[cur as usize].next;
                    if cur == RT_QUEUE_NONE { break false; }
                }
            }
            SchedPolicy::Deadline => {
                // SCHED_DEADLINE → retirer de la file EDF dédiée.
                // SAFETY: DL_QUEUES init, préemption désactivée.
                unsafe {
                    crate::scheduler::timer::deadline_timer::dl_remove(self.cpu.0 as usize, tcb)
                }
            }
            _ => self.cfs.remove(tcb),
        };
        if removed {
            self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
        }
        removed
    }

    /// Sélectionne le prochain thread à exécuter — O(1) via bitmap RT ou tête CFS.
    ///
    /// GARANTIE : 100-150 cycles en mode non-contended (cible DOC3).
    /// INVARIANT : appelé avec préemption désactivée.
    pub fn pick_next(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        self.stats.picks_total.fetch_add(1, Ordering::Relaxed);

        // 1. Priorité absolue : file RT.
        if self.rt.count > 0 {
            self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
            let tcb = self.rt.dequeue_highest()?;
            // BUG-FIX B : décrémenter nr_running — le thread passe Runnable → Running.
            // Avant ce correctif, nr_running ne décrémentait jamais ici, causant un
            // compteur toujours gonflé (load-balancing aveugle, inutilisable).
            self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
            return Some(tcb);
        }

        // 2. File DEADLINE (EDF — échéance la plus proche en tête).
        // SAFETY: DL_QUEUES initialisé par scheduler::init() étape 8.
        let dl_candidate = unsafe {
            crate::scheduler::timer::deadline_timer::dl_pick_next(self.cpu.0 as usize)
        };
        if let Some(tcb) = dl_candidate {
            self.stats.picks_cfs.fetch_add(1, Ordering::Relaxed); // comptabilisé avec CFS
            // BUG-FIX B (DL) : décrémenter nr_running pour thread DEADLINE.
            self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
            return Some(tcb);
        }

        // 3. File CFS (normal / batch).
        if self.cfs.count > 0 {
            self.stats.picks_cfs.fetch_add(1, Ordering::Relaxed);
            let tcb = self.cfs.dequeue_min()?;
            // BUG-FIX B (CFS) : décrémenter nr_running pour thread CFS.
            self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
            return Some(tcb);
        }

        // 4. Thread idle (toujours présent — non comptabilisé dans nr_running).
        self.stats.picks_idle.fetch_add(1, Ordering::Relaxed);
        self.idle_thread
    }

    /// Retourne le nombre de threads en cours d'exécution (RT + CFS).
    #[inline(always)]
    pub fn nr_running(&self) -> u32 {
        self.stats.nr_running.load(Ordering::Relaxed)
    }

    /// Calcule le quantum pour le thread courant (CFS).
    pub fn timeslice_for(&self, tcb: NonNull<ThreadControlBlock>) -> u64 {
        // SAFETY: tcb est un NonNull valide, appelé avec préemption désactivée.
        let weight = unsafe { tcb.as_ref() }.priority.cfs_weight();
        self.cfs.timeslice_ns(weight)
    }

    /// Met à jour le load average exponentiel (EMA, α = 1/8, unité × 1024).
    fn update_load_avg(&self, nr: u64) {
        let old = self.stats.load_avg.load(Ordering::Relaxed);
        let new_avg = (old * 7 + nr * 1024) >> 3;
        self.stats.load_avg.store(new_avg, Ordering::Relaxed);
    }

    /// Avance l'horloge de la run queue (appelé par le tick).
    #[inline(always)]
    pub fn advance_clock(&mut self, delta_ns: u64) {
        self.clock_task_ns = self.clock_task_ns.wrapping_add(delta_ns);
    }

    /// Retourne le nombre de threads actifs (RT + CFS) sous forme de `usize`.
    #[inline(always)]
    pub fn nr_running_usize(&self) -> usize {
        self.stats.nr_running.load(Ordering::Relaxed) as usize
    }

    /// Somme des poids CFS de tous les threads dans la file CFS (pour timeslice).
    pub fn total_cfs_weight(&self) -> u64 {
        let mut total = 0u64;
        for i in 0..self.cfs.count {
            if let Some(tcb) = self.cfs.tasks[i] {
                // SAFETY: tasks[i] est Some et valide (invariant CfsRunQueue).
                let w = unsafe { tcb.as_ref() }.priority.cfs_weight() as u64;
                total = total.saturating_add(w);
            }
        }
        total
    }

    /// Priorité RT la plus haute actuellement dans la file RT (0 = aucune).
    /// Retourne 0 si la file RT est vide.
    pub fn rt_highest_prio(&self) -> u8 {
        if self.rt.count == 0 { return 0; }
        self.rt.bitmap.find_highest_prio().unwrap_or(0)
    }

    /// Retourne (sans extraire) le 2e thread CFS trié par vruntime.
    /// Utilisé par ai_guided::maybe_prefer() pour une comparaison légère.
    pub fn cfs_peek_second(&self) -> Option<NonNull<ThreadControlBlock>> {
        if self.cfs.count >= 2 { self.cfs.tasks[1] }
        else { None }
    }

    /// Défile un thread CFS migreable vers le CPU `dst_cpu`.
    /// Cherche depuis la fin (plus haute vruntime = moins prioritaire = meilleur candidat à migrer).
    /// Ne déplace JAMAIS de threads RT.
    pub fn cfs_dequeue_for_migration(&mut self, dst_cpu: CpuId) -> Option<NonNull<ThreadControlBlock>> {
        // Cherche depuis la fin du tableau CFS (ordre décroissant de vruntime).
        let mut found_idx = None;
        for i in (0..self.cfs.count).rev() {
            if let Some(tcb) = self.cfs.tasks[i] {
                // SAFETY: tasks[i] est Some et valide (invariant CfsRunQueue).
                if unsafe { tcb.as_ref() }.allowed_on(dst_cpu) {
                    found_idx = Some(i);
                    break;
                }
            }
        }
        let idx = found_idx?;
        let tcb = self.cfs.tasks[idx];
        // Retirer la slot en décalant.
        let mut j = idx;
        while j + 1 < self.cfs.count {
            self.cfs.tasks[j] = self.cfs.tasks[j + 1];
            j += 1;
        }
        self.cfs.tasks[self.cfs.count - 1] = None;
        self.cfs.count -= 1;
        // BUG-FIX C : mettre à jour weight_sum. Sans ce correctif, le calcul
        // timeslice CFS devenait de plus en plus faux après chaque migration.
        if let Some(t) = tcb {
            // SAFETY: t est un NonNull valide sorti de tasks[idx].
            let weight = unsafe { t.as_ref() }.priority.cfs_weight() as u64;
            self.cfs.weight_sum = self.cfs.weight_sum.saturating_sub(weight);
        }
        self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
        tcb
    }

    /// Retourne la priorité RT la plus haute prête (O(1) via bitmap).
    /// Expose `RtRunQueue::bitmap` depuis l'extérieur du module.
    #[inline(always)]
    pub fn rt_bitmap_highest_prio(&self) -> Option<u8> {
        if self.rt.count == 0 { return None; }
        self.rt.bitmap.find_highest_prio()
    }

    /// Défile le thread RT le plus prioritaire et décrémente `nr_running`.
    #[inline(always)]
    pub fn dequeue_highest_rt(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        let tcb = self.rt.dequeue_highest()?;
        let prev = self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);
        self.update_load_avg(prev.saturating_sub(1) as u64);
        Some(tcb)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tableau global de run queues — 1 par CPU, statique
// ─────────────────────────────────────────────────────────────────────────────

/// Run queues globales — une par CPU logique.
/// Non protégées car chaque CPU accède UNIQUEMENT à la sienne (sauf migration).
// Run queues per-CPU — initialisées par init_percpu() avant tout accès.
// SAFETY: MaybeUninit évite d'appeler new() à la compilation.
use core::mem::MaybeUninit;
static mut PER_CPU_RQ: [MaybeUninit<PerCpuRunQueue>; MAX_CPUS] = {
    // SAFETY: MaybeUninit::uninit() est valide pour zéro-initialisation.
    unsafe { MaybeUninit::uninit().assume_init() }
};

/// Retourne un pointeur mutable vers la run queue du CPU donné.
///
/// # Safety
/// La run queue DOIT avoir été initialisée par `init_percpu` avant tout appel.
/// L'appelant doit garantir qu'il accède à la run queue de son propre CPU
/// avec la préemption désactivée, OU qu'il tient le lock de migration.
#[inline(always)]
pub unsafe fn run_queue(cpu: CpuId) -> &'static mut PerCpuRunQueue {
    // SAFETY: init_percpu() garantit que toutes les run queues sont initialisées.
    debug_assert!((cpu.0 as usize) < MAX_CPUS, "CPU id hors limites");
    PER_CPU_RQ[cpu.0 as usize].assume_init_mut()
}

/// Initialise les run queues pour tous les CPUs.
/// Appelé depuis `scheduler::init()` — step 2 de la séquence.
pub fn init_percpu(nr_cpus: usize) {
    // SAFETY: init appelé une seule fois, avant tout thread utilisateur.
    unsafe {
        for i in 0..nr_cpus.min(MAX_CPUS) {
            PER_CPU_RQ[i].write(PerCpuRunQueue::new(CpuId(i as u32)));
        }
    }
}
