// ipc/sync/barrier.rs — Barrière de synchronisation IPC pour Exo-OS
//
// Ce module implémente une barrière cyclique (reusable barrier) pour N threads.
// Chaque participant appelle `arrive_and_wait()` ; tous sont bloqués jusqu'à
// ce que les N participants soient arrivés, puis tous sont libérés simultanément.
//
// La barrière est cyclique : après libération, le compteur est remis à zéro
// automatiquement. Un numéro de phase (generation) protège contre les réveils
// spurieux et les races d'un cycle sur l'autre.
//
// RÈGLE BARRIER-01 : pas d'allocation dynamique, table statique globale.
// RÈGLE BARRIER-02 : spin-wait borné par un timeout configurable.
// RÈGLE BARRIER-03 : génération AtomicU32 garantit la correction cyclique.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// Barrière IPC principale
// ---------------------------------------------------------------------------

/// Nombre maximum de participants par barrière
pub const MAX_BARRIER_PARTIES: usize = 256;

/// Nombre maximum de barrières dans la table globale
pub const MAX_IPC_BARRIERS: usize = 64;

/// États de phase de la barrière
mod phase_state {
    pub const WAITING: u32 = 0;
    #[allow(dead_code)]
    pub const RELEASING: u32 = 1;
}

/// Résultat d'un appel à `arrive_and_wait()`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierResult {
    /// Ce thread était le dernier arrivé (thread leader)
    Leader,
    /// Ce thread a été libéré normalement
    Follower,
}

/// Barrière de synchronisation IPC cyclique (reusable).
///
/// Supporte jusqu'à `MAX_BARRIER_PARTIES` participants.  
/// Réutilisable après chaque cycle grâce au numéro de génération.
#[repr(C, align(64))]
pub struct IpcBarrier {
    /// Identifiant opaque
    pub id: u32,
    /// Nombre de participants requis pour franchir la barrière
    parties: AtomicU32,
    /// Compteur d'arrivées pour le cycle courant
    arrived: AtomicU32,
    /// Numéro de génération (phase courante)
    generation: AtomicU32,
    /// Phase de la barrière
    phase: AtomicU32,
    /// Nombre de cycles complétés
    pub cycles: AtomicU64,
    /// Nombre total d'arrivées
    pub total_arrives: AtomicU64,
    /// Nombre de timeouts
    pub total_timeouts: AtomicU64,
    /// Barrière active (non détruite)
    pub active: AtomicBool,
    _pad: [u8; 15],
}

// SAFETY: tous les champs sont atomiques ou primitifs
unsafe impl Sync for IpcBarrier {}
unsafe impl Send for IpcBarrier {}

impl IpcBarrier {
    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            arrived: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            phase: AtomicU32::new(phase_state::WAITING),
            cycles: AtomicU64::new(0),
            total_arrives: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            active: AtomicBool::new(true),
            _pad: [0u8; 15],
        }
    }

    /// Retourne le nombre de participants
    pub fn parties(&self) -> u32 {
        self.parties.load(Ordering::Relaxed)
    }

    /// Retourne le nombre de participants ayant déjà franchi ce cycle
    pub fn arrived_count(&self) -> u32 {
        self.arrived.load(Ordering::Relaxed)
    }

    /// Retourne la génération (phase) courante
    pub fn current_generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // arrive_and_wait()
    // -----------------------------------------------------------------------

    /// Arrive à la barrière et attend que tous les `parties` participants
    /// soient arrivés.
    ///
    /// - `spin_max` : nombre de spins avant timeout (0 = infini)
    ///
    /// # Retour
    /// - `Ok(BarrierResult::Leader)` — ce thread était le dernier (libère les autres)
    /// - `Ok(BarrierResult::Follower)` — réveil normal
    /// - `Err(IpcError::Timeout)` — timeout expiré
    /// - `Err(IpcError::Closed)` — barrière détruite pendant l'attente
    pub fn arrive_and_wait(&self, spin_max: u64) -> Result<BarrierResult, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        self.total_arrives.fetch_add(1, Ordering::Relaxed);

        // Sauvegarder la génération courante AVANT d'incrémenter `arrived`
        let gen = self.generation.load(Ordering::Acquire);
        let parties = self.parties.load(Ordering::Relaxed);

        // Incrémenter le compteur d'arrivées
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;

        if my_arrival == parties {
            // Ce thread est le LEADER : libérer tous les waiters
            self.cycles.fetch_add(1, Ordering::Relaxed);
            // Remettre le compteur à zéro
            self.arrived.store(0, Ordering::Relaxed);
            // Passer à la génération suivante — réveille tous les waiters
            self.generation.fetch_add(1, Ordering::Release);
            return Ok(BarrierResult::Leader);
        }

        // Ce thread doit attendre que la génération change
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;

            if !self.active.load(Ordering::Relaxed) {
                return Err(IpcError::Closed);
            }

            let current_gen = self.generation.load(Ordering::Acquire);
            if current_gen != gen {
                // La génération a changé : la barrière a été franchie
                return Ok(BarrierResult::Follower);
            }

            if spins >= limit {
                // Timeout : décrémenter le compteur pour ne pas bloquer
                // les autres participants (meilleur effort)
                self.arrived.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Arrivée sans attente (non-blocking)
    // -----------------------------------------------------------------------

    /// Arrive à la barrière sans attendre.
    /// Retourne `true` si ce thread a complété le cycle (était le dernier).
    pub fn arrive(&self) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        let parties = self.parties.load(Ordering::Relaxed);
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;
        if my_arrival == parties {
            self.arrived.store(0, Ordering::Relaxed);
            self.cycles.fetch_add(1, Ordering::Relaxed);
            self.generation.fetch_add(1, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Attend que la génération avance (sans incrémenter `arrived`).
    ///
    /// Utile pour `arrive()` non-bloquant suivi d'un wait séparé.
    pub fn wait_for_phase(&self, expected_gen: u32, spin_max: u64) -> Result<(), IpcError> {
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;
        loop {
            core::hint::spin_loop();
            spins += 1;
            if !self.active.load(Ordering::Relaxed) {
                return Err(IpcError::Closed);
            }
            if self.generation.load(Ordering::Acquire) != expected_gen {
                return Ok(());
            }
            if spins >= limit {
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reset et destruction
    // -----------------------------------------------------------------------

    /// Réinitialise la barrière (force reset d'urgence).
    /// Réveille tous les waiters via changement de génération.
    pub fn reset(&self) {
        self.arrived.store(0, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Marque la barrière comme détruite. Les waiters actuels retourneront Closed.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        // Libérer les waiters bloqués en avançant la génération
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Récupère les statistiques
    pub fn snapshot(&self) -> IpcBarrierStats {
        IpcBarrierStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: self.arrived.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
            cycles: self.cycles.load(Ordering::Relaxed),
            total_arrives: self.total_arrives.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcBarrierStats {
    pub id: u32,
    pub parties: u32,
    pub arrived: u32,
    pub generation: u32,
    pub cycles: u64,
    pub total_arrives: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// Table globale de barrières IPC
// ---------------------------------------------------------------------------

struct BarrierSlot {
    barrier: MaybeUninit<IpcBarrier>,
    occupied: AtomicBool,
}

impl BarrierSlot {
    const fn empty() -> Self {
        Self {
            barrier: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct IpcBarrierTable {
    slots: [BarrierSlot; MAX_IPC_BARRIERS],
    count: AtomicU32,
}

// SAFETY : accès via CAS + MaybeUninit
unsafe impl Sync for IpcBarrierTable {}

impl IpcBarrierTable {
    const fn new() -> Self {
        const EMPTY: BarrierSlot = BarrierSlot::empty();
        Self {
            slots: [EMPTY; MAX_IPC_BARRIERS],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32, parties: u32) -> Option<usize> {
        if parties == 0 || parties as usize > MAX_BARRIER_PARTIES {
            return None;
        }
        for i in 0..MAX_IPC_BARRIERS {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i].occupied.compare_exchange(
                    false,
                    true,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ).is_ok() {
                    let ptr = self.slots[i].barrier.as_ptr() as *mut IpcBarrier;
                    // SAFETY: CAS AcqRel garantit l'exclusivité du slot; MaybeUninit<IpcBarrier> write-once.
                    unsafe { ptr.write(IpcBarrier::new(id, parties)) }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcBarrier> {
        if idx >= MAX_IPC_BARRIERS { return None; }
        if !self.slots[idx].occupied.load(Ordering::Acquire) { return None; }
        // SAFETY: occupied=true guarantees initialization
        Some(unsafe { &*self.slots[idx].barrier.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_IPC_BARRIERS { return false; }
        if let Some(b) = self.get(idx) {
            b.destroy();
        }
        if self.slots[idx].occupied.compare_exchange(
            true,
            false,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ).is_ok() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

static IPC_BARRIER_TABLE: IpcBarrierTable = IpcBarrierTable::new();

// ---------------------------------------------------------------------------
// API publique
// ---------------------------------------------------------------------------

/// Crée une barrière pour `parties` participants. Retourne l'index (handle).
pub fn barrier_create(id: u32, parties: u32) -> Option<usize> {
    IPC_BARRIER_TABLE.alloc(id, parties)
}

/// Arrive et attend que tous les participants soient là.
pub fn barrier_arrive_and_wait(idx: usize, spin_max: u64) -> Result<BarrierResult, IpcError> {
    IPC_BARRIER_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.arrive_and_wait(spin_max)
}

/// Arrive sans attendre. Retourne true si ce thread complète le cycle.
pub fn barrier_arrive(idx: usize) -> Result<bool, IpcError> {
    Ok(IPC_BARRIER_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.arrive())
}

/// Attend que la génération expectedgen avance.
pub fn barrier_wait_phase(idx: usize, expected_gen: u32, spin_max: u64) -> Result<(), IpcError> {
    IPC_BARRIER_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.wait_for_phase(expected_gen, spin_max)
}

/// Retourne la génération courante (pour un arrive non-bloquant).
pub fn barrier_generation(idx: usize) -> Option<u32> {
    IPC_BARRIER_TABLE.get(idx).map(|b| b.current_generation())
}

/// Réinitialise d'urgence la barrière.
pub fn barrier_reset(idx: usize) -> Result<(), IpcError> {
    IPC_BARRIER_TABLE.get(idx).ok_or(IpcError::InvalidHandle).map(|b| b.reset())
}

/// Détruit la barrière et libère son slot.
pub fn barrier_destroy(idx: usize) -> bool {
    IPC_BARRIER_TABLE.free(idx)
}

/// Nombre de barrières actives.
pub fn barrier_count() -> u32 {
    IPC_BARRIER_TABLE.count()
}

/// Snapshot de statistiques.
pub fn barrier_stats(idx: usize) -> Option<IpcBarrierStats> {
    IPC_BARRIER_TABLE.get(idx).map(|b| b.snapshot())
}
