// ipc/sync/rendezvous.rs — Point de rendez-vous IPC généralisé pour Exo-OS
//
// Ce module implémente un rendez-vous N-voies (N-way rendezvous) :
// N threads doivent tous arriver au point de rendez-vous avant que l'un
// d'eux puisse continuer. Contrairement à la barrière cyclique, le rendez-vous
// est à usage unique par défaut, mais peut être réarmé explicitement.
//
// Variante disponible : RendezvousExchange<T: Copy> permettant à deux threads
// d'échanger simultanément une valeur (rendezvous symétrique bilatéral).
//
// RÈGLE RNDV-01 : pas d'allocation. Tables statiques uniquement.
// RÈGLE RNDV-02 : exchange atomique via CAS à deux phases (offer/take).
// RÈGLE RNDV-03 : spin-wait borné. Timeout → désinscription propre.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, AtomicUsize, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::IpcError;

// ---------------------------------------------------------------------------
// Rendez-vous N-voies
// ---------------------------------------------------------------------------

/// Nombre maximum de participants par rendez-vous
pub const MAX_RENDEZVOUS_PARTIES: usize = 64;

/// Nombre maximum de rendez-vous dans la table globale
pub const MAX_RENDEZVOUS_ENTRIES: usize = 64;

/// État du rendez-vous
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RendezvousState {
    /// En attente de tous les participants
    Waiting = 0,
    /// Tous les participants sont arrivés — libération en cours
    Releasing = 1,
    /// Détruit / invalidé
    Destroyed = 2,
}

impl RendezvousState {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Releasing,
            2 => Self::Destroyed,
            _ => Self::Waiting,
        }
    }
}

/// Rendez-vous IPC N-voies à usage unique (peut être réarmé).
#[repr(C, align(64))]
pub struct IpcRendezvous {
    pub id: u32,
    /// Nombre de participants requis
    parties: AtomicU32,
    /// Nombre de participants arrivés
    arrived: AtomicU32,
    /// État interne
    state: AtomicU32,
    /// Génération (anti-spurious wakeup)
    generation: AtomicU32,
    /// Actif (non détruit)
    pub active: AtomicBool,
    _pad: [u8; 3],
    /// Statistiques
    pub total_meetings: AtomicU64,
    pub total_timeouts: AtomicU64,
}

unsafe impl Sync for IpcRendezvous {}
unsafe impl Send for IpcRendezvous {}

impl IpcRendezvous {
    pub const fn new(id: u32, parties: u32) -> Self {
        Self {
            id,
            parties: AtomicU32::new(parties),
            arrived: AtomicU32::new(0),
            state: AtomicU32::new(RendezvousState::Waiting as u32),
            generation: AtomicU32::new(0),
            active: AtomicBool::new(true),
            _pad: [0u8; 3],
            total_meetings: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
        }
    }

    pub fn parties(&self) -> u32 {
        self.parties.load(Ordering::Relaxed)
    }

    pub fn arrived_count(&self) -> u32 {
        self.arrived.load(Ordering::Relaxed)
    }

    /// Arrive au rendez-vous et attend.
    ///
    /// Retourne `Ok(true)` pour le thread leader (dernier arrivé),
    /// `Ok(false)` pour les autres.
    pub fn meet(&self, spin_max: u64) -> Result<bool, IpcError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(IpcError::Closed);
        }

        let gen = self.generation.load(Ordering::Acquire);
        let parties = self.parties.load(Ordering::Relaxed);
        let my_arrival = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;

        if my_arrival == parties {
            // Ce thread est le leader
            self.state.store(RendezvousState::Releasing as u32, Ordering::Release);
            self.total_meetings.fetch_add(1, Ordering::Relaxed);
            // Avancer la génération pour libérer les autres
            self.generation.fetch_add(1, Ordering::Release);
            return Ok(true);
        }

        // Attendre que le leader libère
        let limit = if spin_max == 0 { u64::MAX } else { spin_max };
        let mut spins = 0u64;

        loop {
            core::hint::spin_loop();
            spins += 1;

            if !self.active.load(Ordering::Relaxed) {
                return Err(IpcError::Closed);
            }

            if self.generation.load(Ordering::Acquire) != gen {
                return Ok(false);
            }

            if spins >= limit {
                // Timeout : décrémenter pour ne pas bloquer le rendez-vous
                self.arrived.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
        }
    }

    /// Réarme le rendez-vous pour un prochain cycle.
    pub fn rearm(&self) {
        self.arrived.store(0, Ordering::Release);
        self.state.store(RendezvousState::Waiting as u32, Ordering::Release);
    }

    /// Détruit le rendez-vous.
    pub fn destroy(&self) {
        self.active.store(false, Ordering::Release);
        self.state.store(RendezvousState::Destroyed as u32, Ordering::Release);
        // Libérer les éventuels waiters
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> IpcRendezvousStats {
        IpcRendezvousStats {
            id: self.id,
            parties: self.parties.load(Ordering::Relaxed),
            arrived: self.arrived.load(Ordering::Relaxed),
            total_meetings: self.total_meetings.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct IpcRendezvousStats {
    pub id: u32,
    pub parties: u32,
    pub arrived: u32,
    pub total_meetings: u64,
    pub total_timeouts: u64,
}

// ---------------------------------------------------------------------------
// Échange symétrique 2-voies : RendezvousExchange<T>
// ---------------------------------------------------------------------------

/// Taille maximale d'une valeur échangeable
pub const MAX_EXCHANGE_SIZE: usize = 512;

/// État de l'échange bilatéral
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExchangeState {
    /// Personne n'attend
    Empty = 0,
    /// Thread A a déposé sa valeur et attend
    OfferPending = 1,
    /// Thread B a récupéré, dépôt en cours
    Completing = 2,
}

/// Slot d'échange inline (pas de pointeur, données copiées)
#[repr(C, align(64))]
pub struct ExchangeSlot {
    state: AtomicU32,
    /// Identité du thread offrant (pour exclusion)
    offerer_thread: AtomicU32,
    /// Taille des données (pour validation)
    data_size: AtomicU32,
    /// Buffer d'offre (données du thread A)
    offer_buf: [u8; MAX_EXCHANGE_SIZE],
    /// Buffer de réponse (données du thread B)
    reply_buf: [u8; MAX_EXCHANGE_SIZE],
    /// Résultat disponible (thread A attend reply_buf)
    result_ready: AtomicBool,
    _pad: [u8; 3],
    /// Statistiques
    pub exchanges: AtomicU64,
    pub timeouts: AtomicU64,
}

unsafe impl Sync for ExchangeSlot {}
unsafe impl Send for ExchangeSlot {}

impl ExchangeSlot {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(ExchangeState::Empty as u32),
            offerer_thread: AtomicU32::new(0),
            data_size: AtomicU32::new(0),
            offer_buf: [0u8; MAX_EXCHANGE_SIZE],
            reply_buf: [0u8; MAX_EXCHANGE_SIZE],
            result_ready: AtomicBool::new(false),
            _pad: [0u8; 3],
            exchanges: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
        }
    }

    /// Échange de données entre deux threads.
    ///
    /// Thread A appelle `exchange(my_data, out_buf)` → bloque jusqu'à ce que
    /// Thread B appelle aussi `exchange(...)`.
    ///
    /// Les deux threads ressortent avec les données de l'autre.
    ///
    /// `data` doit avoir la même taille `size` pour les deux threads.
    pub fn exchange(
        &self,
        thread_id: u32,
        data: &[u8],
        out: &mut [u8],
        spin_max: u64,
    ) -> Result<(), IpcError> {
        if data.len() > MAX_EXCHANGE_SIZE || out.len() < data.len() {
            return Err(IpcError::Invalid);
        }

        let size = data.len() as u32;

        // Tenter de devenir l'offrant (CAS Empty → OfferPending)
        let became_offerer = self.state.compare_exchange(
            ExchangeState::Empty as u32,
            ExchangeState::OfferPending as u32,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ).is_ok();

        if became_offerer {
            // --- Thread A : déposer l'offre ---
            self.offerer_thread.store(thread_id, Ordering::Relaxed);
            self.data_size.store(size, Ordering::Relaxed);

            // Copier mes données dans offer_buf
            // SAFETY: offer_buf est un tableau inline, data.len() <= MAX_EXCHANGE_SIZE
            unsafe {
                let dst = self.offer_buf.as_ptr() as *mut u8;
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            }

            self.result_ready.store(false, Ordering::Release);

            // Attendre que Thread B complète l'échange
            let limit = if spin_max == 0 { u64::MAX } else { spin_max };
            let mut spins = 0u64;
            loop {
                core::hint::spin_loop();
                spins += 1;
                if self.result_ready.load(Ordering::Acquire) {
                    // Lire la réponse de B dans reply_buf
                    // SAFETY: B a écrit reply_buf avant de mettre result_ready
                    unsafe {
                        let src = self.reply_buf.as_ptr();
                        core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), data.len());
                    }
                    self.exchanges.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                if spins >= limit {
                    // Annuler l'offre
                    self.state.store(ExchangeState::Empty as u32, Ordering::Release);
                    self.timeouts.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Timeout);
                }
            }
        } else {
            // --- Thread B : tenter de prendre l'offre ---
            let limit = if spin_max == 0 { u64::MAX } else { spin_max };
            let mut spins = 0u64;
            loop {
                core::hint::spin_loop();
                spins += 1;

                let st = self.state.load(Ordering::Acquire);
                if st == ExchangeState::OfferPending as u32 {
                    // CAS OfferPending → Completing
                    if self.state.compare_exchange(
                        ExchangeState::OfferPending as u32,
                        ExchangeState::Completing as u32,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ).is_ok() {
                        // Lire l'offre de A
                        // SAFETY: A a initialisé offer_buf sous OfferPending
                        let offer_size = self.data_size.load(Ordering::Relaxed) as usize;
                        let recv_len = offer_size.min(out.len());
                        // SAFETY: offer_buf initialisé par A sous OfferPending (Acquire/Release sur state).
                        unsafe {
                            let src = self.offer_buf.as_ptr();
                            core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), recv_len);
                        }

                        // Écrire notre réponse dans reply_buf
                        let write_len = data.len().min(MAX_EXCHANGE_SIZE);
                        // SAFETY: reply_buf exclusif en état Completing — seul B écrit ici.
                        unsafe {
                            let dst = self.reply_buf.as_ptr() as *mut u8;
                            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, write_len);
                        }

                        // Signaler à A que l'échange est terminé
                        self.result_ready.store(true, Ordering::Release);
                        // Revenir à Empty pour le prochain échange
                        self.state.store(ExchangeState::Empty as u32, Ordering::Release);
                        return Ok(());
                    }
                }

                if spins >= limit {
                    self.timeouts.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Timeout);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tables globales
// ---------------------------------------------------------------------------

struct RendezvousTableEntry {
    rdv: MaybeUninit<IpcRendezvous>,
    occupied: AtomicBool,
}

impl RendezvousTableEntry {
    const fn empty() -> Self {
        Self { rdv: MaybeUninit::uninit(), occupied: AtomicBool::new(false) }
    }
}

struct IpcRendezvousTable {
    slots: [RendezvousTableEntry; MAX_RENDEZVOUS_ENTRIES],
    count: AtomicU32,
}

unsafe impl Sync for IpcRendezvousTable {}

impl IpcRendezvousTable {
    const fn new() -> Self {
        const EMPTY: RendezvousTableEntry = RendezvousTableEntry::empty();
        Self { slots: [EMPTY; MAX_RENDEZVOUS_ENTRIES], count: AtomicU32::new(0) }
    }

    fn alloc(&self, id: u32, parties: u32) -> Option<usize> {
        if parties == 0 || parties as usize > MAX_RENDEZVOUS_PARTIES { return None; }
        for i in 0..MAX_RENDEZVOUS_ENTRIES {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i].occupied.compare_exchange(
                    false, true, Ordering::AcqRel, Ordering::Relaxed,
                ).is_ok() {
                    // SAFETY: CAS AcqRel garantit l'exclusivité du slot; rdv MaybeUninit write-once.
                    unsafe {
                        (self.slots[i].rdv.as_ptr() as *mut IpcRendezvous)
                            .write(IpcRendezvous::new(id, parties));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&IpcRendezvous> {
        if idx >= MAX_RENDEZVOUS_ENTRIES { return None; }
        if !self.slots[idx].occupied.load(Ordering::Acquire) { return None; }
        Some(unsafe { &*self.slots[idx].rdv.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_RENDEZVOUS_ENTRIES { return false; }
        if let Some(r) = self.get(idx) { r.destroy(); }
        if self.slots[idx].occupied.compare_exchange(
            true, false, Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

/// Nombre maximal d'échanges symétrique dans la table
pub const MAX_EXCHANGE_SLOTS: usize = 64;

struct ExchangeTable {
    slots: [MaybeUninit<ExchangeSlot>; MAX_EXCHANGE_SLOTS],
    occupied: [AtomicBool; MAX_EXCHANGE_SLOTS],
    count: AtomicU32,
}

unsafe impl Sync for ExchangeTable {}

impl ExchangeTable {
    const fn new() -> Self {
        const UNINIT: MaybeUninit<ExchangeSlot> = MaybeUninit::uninit();
        const FALSE: AtomicBool = AtomicBool::new(false);
        Self { slots: [UNINIT; MAX_EXCHANGE_SLOTS], occupied: [FALSE; MAX_EXCHANGE_SLOTS], count: AtomicU32::new(0) }
    }

    fn alloc(&self) -> Option<usize> {
        for i in 0..MAX_EXCHANGE_SLOTS {
            if !self.occupied[i].load(Ordering::Relaxed) {
                if self.occupied[i].compare_exchange(
                    false, true, Ordering::AcqRel, Ordering::Relaxed,
                ).is_ok() {
                    // SAFETY: CAS AcqRel garantit l'exclusivité; slots[i] MaybeUninit<ExchangeSlot> write-once.
                    unsafe {
                        (self.slots[i].as_ptr() as *mut ExchangeSlot)
                            .write(ExchangeSlot::new());
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&ExchangeSlot> {
        if idx >= MAX_EXCHANGE_SLOTS { return None; }
        if !self.occupied[idx].load(Ordering::Acquire) { return None; }
        Some(unsafe { &*self.slots[idx].as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_EXCHANGE_SLOTS { return false; }
        if self.occupied[idx].compare_exchange(
            true, false, Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok() {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static IPC_RENDEZVOUS_TABLE: IpcRendezvousTable = IpcRendezvousTable::new();
static IPC_EXCHANGE_TABLE: ExchangeTable = ExchangeTable::new();

// ---------------------------------------------------------------------------
// API publique : rendez-vous N-voies
// ---------------------------------------------------------------------------

/// Crée un rendez-vous pour `parties` participants. Retourne l'index (handle).
pub fn rendezvous_create(id: u32, parties: u32) -> Option<usize> {
    IPC_RENDEZVOUS_TABLE.alloc(id, parties)
}

/// Attend que tous les `parties` soient présents.
/// Retourne `true` si ce thread était le dernier (leader).
pub fn rendezvous_meet(idx: usize, spin_max: u64) -> Result<bool, IpcError> {
    IPC_RENDEZVOUS_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?.meet(spin_max)
}

/// Réarme le rendez-vous pour un prochain cycle.
pub fn rendezvous_rearm(idx: usize) -> Result<(), IpcError> {
    IPC_RENDEZVOUS_TABLE.get(idx).ok_or(IpcError::InvalidHandle).map(|r| r.rearm())
}

/// Détruit le rendez-vous et libère son slot.
pub fn rendezvous_destroy(idx: usize) -> bool {
    IPC_RENDEZVOUS_TABLE.free(idx)
}

/// Récupère le nombre de participants arrivés.
pub fn rendezvous_arrived(idx: usize) -> Option<u32> {
    IPC_RENDEZVOUS_TABLE.get(idx).map(|r| r.arrived_count())
}

/// Statistiques du rendez-vous.
pub fn rendezvous_stats(idx: usize) -> Option<IpcRendezvousStats> {
    IPC_RENDEZVOUS_TABLE.get(idx).map(|r| r.snapshot())
}

// ---------------------------------------------------------------------------
// API publique : échange symétrique
// ---------------------------------------------------------------------------

/// Crée un slot d'échange symétrique.
pub fn exchange_create() -> Option<usize> {
    IPC_EXCHANGE_TABLE.alloc()
}

/// Échange des données entre deux threads.
///
/// Les deux threads appellent cette fonction simultanément.
/// Chacun reçoit les données de l'autre dans `out`.
pub fn exchange_swap(
    idx: usize,
    thread_id: u32,
    data: &[u8],
    out: &mut [u8],
    spin_max: u64,
) -> Result<(), IpcError> {
    IPC_EXCHANGE_TABLE.get(idx).ok_or(IpcError::InvalidHandle)?
        .exchange(thread_id, data, out, spin_max)
}

/// Détruit un slot d'échange.
pub fn exchange_destroy(idx: usize) -> bool {
    IPC_EXCHANGE_TABLE.free(idx)
}
