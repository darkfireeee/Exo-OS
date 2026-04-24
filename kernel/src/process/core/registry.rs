// kernel/src/process/core/registry.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessRegistry — table globale PID → PCB (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure :
//   • Tableau plat de slots (index = PID - 1).
//   • Chaque slot = Option<Box<ProcessControlBlock>> dans un RwLock léger.
//   • Accès non bloquant en lecture via AtomicPtr pour les hot paths.
//   • Écriture protégée par spinlock global de la registry.
//
// Concurrence :
//   Les lecteurs (signal, syscall, IPC) utilisent `find_by_pid()` qui lit
//   le slot via raw pointer atomique — cohérent via la garantie que le PCB
//   ne peut pas être libéré tant qu'il y a des références (refcounting).
// ═══════════════════════════════════════════════════════════════════════════════

use super::pcb::{Credentials, ProcessControlBlock};
use super::pid::Pid;
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// RegistrySlot
// ─────────────────────────────────────────────────────────────────────────────

/// Un slot dans la table de la registry.
/// AtomicPtr permet les lectures lockless (non-blocking reads).
struct RegistrySlot {
    /// Pointeur atomique vers le PCB. null = slot vide.
    pcb_ptr: AtomicPtr<ProcessControlBlock>,
    /// Compteur de références actives sur ce PCB.
    refcount: AtomicU32,
}

impl RegistrySlot {
    const fn empty() -> Self {
        Self {
            pcb_ptr: AtomicPtr::new(core::ptr::null_mut()),
            refcount: AtomicU32::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessRegistry
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur de registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    /// Slot plein (pas de place).
    Full,
    /// PID non trouvé.
    NotFound,
    /// PID déjà enregistré.
    AlreadyExists,
}

/// Table globale PID → PCB (initialisation dynamique).
pub struct ProcessRegistry {
    /// Slots de la registry (alloués à l'init).
    slots: *mut RegistrySlot,
    /// Capacité de la table (nombre de PIDs supportés).
    capacity: usize,
    /// Spinlock pour les écritures (insert/remove).
    write_lock: SpinLock<()>,
    /// Nombre de processus enregistrés.
    count: AtomicUsize,
    /// Nombre total d'insertions depuis le boot.
    inserts: AtomicUsize,
    /// Nombre total de suppressions.
    removes: AtomicUsize,
    /// Nombre de lookups (instrumentation debugging).
    lookups: AtomicUsize,
}

// SAFETY: slots pointés sont accédés de manière exclusive ou via atomiques.
unsafe impl Sync for ProcessRegistry {}
unsafe impl Send for ProcessRegistry {}

// Registry globale — initialement invalide (ptr null, capacity 0).
pub static PROCESS_REGISTRY: ProcessRegistry = ProcessRegistry {
    slots: core::ptr::null_mut(),
    capacity: 0,
    write_lock: SpinLock::new(()),
    count: AtomicUsize::new(0),
    inserts: AtomicUsize::new(0),
    removes: AtomicUsize::new(0),
    lookups: AtomicUsize::new(0),
};

impl ProcessRegistry {
    /// Initialise la registry avec `capacity` slots.
    ///
    /// # Safety
    /// Appelé une seule fois depuis le BSP durant l'init.
    pub unsafe fn init(&self, capacity: usize) {
        use alloc::alloc::{alloc_zeroed, Layout};
        let layout = Layout::array::<RegistrySlot>(capacity).expect("layout RegistrySlot valide");
        let ptr = alloc_zeroed(layout) as *mut RegistrySlot;
        assert!(!ptr.is_null(), "ProcessRegistry::init : allocation échouée");
        // Initialiser chaque slot à l'état vide.
        for i in 0..capacity {
            // SAFETY: ptr + i est dans le tableau alloué.
            core::ptr::write(ptr.add(i), RegistrySlot::empty());
        }
        // Mise à jour via raw pointer (la static mut est à adresse fixe).
        let self_mut = self as *const Self as *mut Self;
        (*self_mut).slots = ptr;
        (*self_mut).capacity = capacity;
    }

    /// Enregistre un PCB dans la table (appelé lors de la création d'un processus).
    pub fn insert(&self, pcb: Box<ProcessControlBlock>) -> Result<(), RegistryError> {
        let pid = pcb.pid;
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return Err(RegistryError::Full);
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity, slots initialisé.
        let slot = unsafe { &*self.slots.add(slot_idx) };

        let _guard = self.write_lock.lock();
        if !slot.pcb_ptr.load(Ordering::Relaxed).is_null() {
            return Err(RegistryError::AlreadyExists);
        }
        // Convertit Box en raw pointer, stocke dans le slot.
        let raw = Box::into_raw(pcb);
        slot.pcb_ptr.store(raw, Ordering::Release);
        slot.refcount.store(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.inserts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Retire le PCB de la table et retourne la Box pour libération.
    pub fn remove(&self, pid: Pid) -> Result<Box<ProcessControlBlock>, RegistryError> {
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return Err(RegistryError::NotFound);
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity.
        let slot = unsafe { &*self.slots.add(slot_idx) };

        let _guard = self.write_lock.lock();
        let raw = slot.pcb_ptr.swap(core::ptr::null_mut(), Ordering::AcqRel);
        if raw.is_null() {
            return Err(RegistryError::NotFound);
        }
        self.count.fetch_sub(1, Ordering::Relaxed);
        self.removes.fetch_add(1, Ordering::Relaxed);
        // SAFETY: raw a été mis dans la table via Box::into_raw dans insert().
        Ok(unsafe { Box::from_raw(raw) })
    }

    /// Recherche lockless d'un PCB par PID.
    /// Retourne un pointeur brut validé ou null.
    /// Le pointeur reste valide tant que le PCB est dans la table.
    pub fn find_by_pid(&self, pid: Pid) -> Option<&ProcessControlBlock> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let idx = pid.0 as usize;
        if idx == 0 || idx > self.capacity {
            return None;
        }
        let slot_idx = idx - 1;
        // SAFETY: slot_idx < capacity, accès read-only via AtomicPtr.
        let slot = unsafe { &*self.slots.add(slot_idx) };
        let raw = slot.pcb_ptr.load(Ordering::Acquire);
        if raw.is_null() {
            return None;
        }
        // SAFETY: raw non null = PCB encore dans la table (non libéré).
        Some(unsafe { &*raw })
    }

    /// Nombre de processus courants.
    #[inline(always)]
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Itère sur tous les PCBs actifs (lecture seule).
    /// Fermeture appelée pour chaque PCB non-null.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&ProcessControlBlock),
    {
        for i in 0..self.capacity {
            // SAFETY: i < capacity.
            let slot = unsafe { &*self.slots.add(i) };
            let raw = slot.pcb_ptr.load(Ordering::Acquire);
            if !raw.is_null() {
                // SAFETY: raw non null = PCB valide en mémoire.
                f(unsafe { &*raw });
            }
        }
    }

    /// Accède aux credentials d'un processus et applique une fermeture mutatrice.
    ///
    /// Retourne `Some(R)` si le processus existe, `None` sinon.
    /// La fermeture reçoit `&mut Credentials` pour modification atomique.
    ///
    /// # Example
    /// ```rust
    /// PROCESS_REGISTRY.with_creds_mut_by_pid(pid, |c| { c.euid = 0; });
    /// ```
    pub fn with_creds_mut_by_pid<F, R>(&self, pid: Pid, f: F) -> Option<R>
    where
        F: FnOnce(&mut Credentials) -> R,
    {
        let pcb = self.find_by_pid(pid)?;
        // SAFETY: SpinLock<Credentials> — verrouillage court (quelques instructions),
        // pas d'allocation ni d'appel système à l'intérieur.
        Some(f(&mut *pcb.creds.lock()))
    }

    /// Statistiques de la registry pour le système de monitoring.
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            current_count: self.count.load(Ordering::Relaxed),
            total_inserts: self.inserts.load(Ordering::Relaxed),
            total_removes: self.removes.load(Ordering::Relaxed),
            total_lookups: self.lookups.load(Ordering::Relaxed),
            capacity: self.capacity,
        }
    }
}

/// Statistiques exportées par la registry.
#[derive(Debug, Clone, Copy)]
pub struct RegistryStats {
    pub current_count: usize,
    pub total_inserts: usize,
    pub total_removes: usize,
    pub total_lookups: usize,
    pub capacity: usize,
}

/// Initialise la registry globale.
///
/// # Safety
/// Une seule fois depuis le BSP.
pub unsafe fn init(max_pids: usize) {
    PROCESS_REGISTRY.init(max_pids);
}
