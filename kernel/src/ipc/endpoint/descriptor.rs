// kernel/src/ipc/endpoint/descriptor.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ENDPOINT DESCRIPTOR — Description complète d'un endpoint IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un EndpointDesc décrit un point de communication nommé :
//   • Identificateur unique (EndpointId)
//   • Liste des propriétaires (ThreadId) pouvant accepter des connexions
//   • File de connexions entrantes (backlog)
//   • État (Listening, Closed, Drain)
//
// RÈGLE : zéro allocation heap ici — structures à taille fixe (Zone NO-ALLOC).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::{ENDPOINT_BACKLOG, MAX_ENDPOINT_NAME_LEN, MAX_ENDPOINT_OWNERS};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::scheduler::core::task::ThreadId;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// EndpointState — machine d'états d'un endpoint
// ─────────────────────────────────────────────────────────────────────────────

/// État d'un endpoint IPC.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum EndpointState {
    /// Endpoint créé, pas encore en écoute.
    Created = 0,
    /// En écoute — accepte des connexions.
    Listening = 1,
    /// Fermé — rejette toutes les connexions.
    Closed = 2,
    /// En cours de fermeture (drain des messages en vol).
    Draining = 3,
    /// Détruit — la structure peut être réutilisée.
    Destroyed = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// PendingConnection — connexion en attente dans le backlog
// ─────────────────────────────────────────────────────────────────────────────

/// Une connexion en attente d'acceptation.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PendingConnection {
    /// Thread demandeur.
    pub requester: ThreadId,
    /// Canal proposé pour la connexion.
    pub channel_id: u64,
    /// Cookie pour corréler la réponse.
    pub cookie: u64,
    /// Timestamp de la demande (ticks).
    pub timestamp_ticks: u64,
}

impl PendingConnection {
    pub const EMPTY: Self = Self {
        requester: ThreadId(0),
        channel_id: 0,
        cookie: 0,
        timestamp_ticks: 0,
    };

    pub fn is_valid(&self) -> bool {
        self.requester.0 != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointName — nom ASCII null-terminé
// ─────────────────────────────────────────────────────────────────────────────

/// Nom d'un endpoint — max 64 bytes ASCII.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct EndpointName {
    bytes: [u8; MAX_ENDPOINT_NAME_LEN],
    len: u8,
}

impl EndpointName {
    pub const EMPTY: Self = Self {
        bytes: [0u8; MAX_ENDPOINT_NAME_LEN],
        len: 0,
    };

    /// Crée un nom depuis un slice ASCII.
    pub fn from_bytes(src: &[u8]) -> Result<Self, IpcError> {
        if src.len() >= MAX_ENDPOINT_NAME_LEN {
            return Err(IpcError::InvalidParam);
        }
        let mut name = Self::EMPTY;
        name.bytes[..src.len()].copy_from_slice(src);
        name.len = src.len() as u8;
        Ok(name)
    }

    /// Retourne le slice de bytes du nom.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Retourne la longueur du nom.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Compare deux noms (insensible à la casse non requis).
    pub fn eq_bytes(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EndpointDesc — descripteur principal
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur complet d'un endpoint IPC.
/// Taille fixe — alloué depuis un pool statique (endpoint/registry.rs).
#[repr(C, align(64))]
pub struct EndpointDesc {
    /// Identifiant unique de l'endpoint.
    pub id: EndpointId,
    /// État courant (atomique pour lecture sans lock).
    pub state: AtomicU32,
    /// Nombre de connexions actives.
    pub active_conns: AtomicU32,
    /// Génération (incrémentée à chaque fermeture/réouverture).
    pub generation: AtomicU64,
    /// Propriétaires qui peuvent accepter des connexions (max MAX_ENDPOINT_OWNERS).
    pub owners: [ThreadId; MAX_ENDPOINT_OWNERS],
    /// Nombre de propriétaires actifs.
    pub owner_count: u32,
    /// Drapeaux de configuration (ex : BROADCAST, RPC, STREAMING).
    pub config_flags: u32,
    /// Nom de l'endpoint (ASCII).
    pub name: EndpointName,
    /// Backlog : file des connexions en attente d'acceptation.
    pub backlog: [PendingConnection; ENDPOINT_BACKLOG],
    /// Tête du backlog (index prochain à accepter).
    pub backlog_head: AtomicU32,
    /// Queue du backlog (index prochain slot libre).
    pub backlog_tail: AtomicU32,
    /// Compteur total de connexions acceptées.
    pub total_accepted: AtomicU64,
    /// Compteur total de connexions refusées (backlog plein).
    pub total_refused: AtomicU64,
}

impl EndpointDesc {
    /// Crée un nouveau descripteur d'endpoint.
    pub fn new(id: EndpointId, name: EndpointName, owner: ThreadId) -> Self {
        let mut owners = [ThreadId(0); MAX_ENDPOINT_OWNERS];
        owners[0] = owner;
        Self {
            id,
            state: AtomicU32::new(EndpointState::Created as u32),
            active_conns: AtomicU32::new(0),
            generation: AtomicU64::new(1),
            owners,
            owner_count: 1,
            config_flags: 0,
            name,
            backlog: [PendingConnection::EMPTY; ENDPOINT_BACKLOG],
            backlog_head: AtomicU32::new(0),
            backlog_tail: AtomicU32::new(0),
            total_accepted: AtomicU64::new(0),
            total_refused: AtomicU64::new(0),
        }
    }

    /// Retourne l'état courant.
    #[inline(always)]
    pub fn state(&self) -> EndpointState {
        match self.state.load(Ordering::Acquire) {
            0 => EndpointState::Created,
            1 => EndpointState::Listening,
            2 => EndpointState::Closed,
            3 => EndpointState::Draining,
            4 => EndpointState::Destroyed,
            _ => EndpointState::Closed, // valeur invalide → fermé par sécurité
        }
    }

    /// Transition vers l'état Listening.
    pub fn start_listen(&self) -> Result<(), IpcError> {
        self.state
            .compare_exchange(
                EndpointState::Created as u32,
                EndpointState::Listening as u32,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .map(|_| ())
            .map_err(|_| IpcError::InvalidParam)
    }

    /// Retourne vrai si l'endpoint accepte des connexions.
    #[inline(always)]
    pub fn is_listening(&self) -> bool {
        self.state.load(Ordering::Acquire) == EndpointState::Listening as u32
    }

    /// Enqueue une connexion dans le backlog.
    /// Retourne `Err(WouldBlock)` si le backlog est plein.
    pub fn enqueue_connection(&self, conn: PendingConnection) -> Result<(), IpcError> {
        let tail = self.backlog_tail.load(Ordering::Relaxed);
        let head = self.backlog_head.load(Ordering::Acquire);
        let next = (tail + 1) % ENDPOINT_BACKLOG as u32;
        if next == head {
            self.total_refused.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::WouldBlock);
        }
        // SAFETY: tail ∈ [0,ENDPOINT_BACKLOG), place vérifiée (next != head), producteur unique.
        unsafe {
            let slot = core::ptr::addr_of!(self.backlog[tail as usize]) as *mut PendingConnection;
            core::ptr::write(slot, conn);
        }
        self.backlog_tail.store(next, Ordering::Release);
        Ok(())
    }

    /// Dequeue une connexion du backlog.
    pub fn dequeue_connection(&self) -> Option<PendingConnection> {
        let head = self.backlog_head.load(Ordering::Relaxed);
        let tail = self.backlog_tail.load(Ordering::Acquire);
        if head == tail {
            return None; // backlog vide
        }
        // SAFETY: head est dans [0, ENDPOINT_BACKLOG) et tail != head.
        let conn = unsafe { core::ptr::read(&self.backlog[head as usize]) };
        let next = (head + 1) % ENDPOINT_BACKLOG as u32;
        self.backlog_head.store(next, Ordering::Release);
        self.total_accepted.fetch_add(1, Ordering::Relaxed);
        Some(conn)
    }

    /// Retourne le nombre de connexions en attente dans le backlog.
    #[inline(always)]
    pub fn backlog_len(&self) -> usize {
        let h = self.backlog_head.load(Ordering::Relaxed) as usize;
        let t = self.backlog_tail.load(Ordering::Relaxed) as usize;
        t.wrapping_sub(h) % ENDPOINT_BACKLOG
    }

    /// Vérifie si un ThreadId est propriétaire de cet endpoint.
    pub fn is_owner(&self, tid: ThreadId) -> bool {
        self.owners[..self.owner_count as usize]
            .iter()
            .any(|&o| o == tid)
    }

    /// Ajoute un propriétaire supplémentaire.
    pub fn add_owner(&mut self, tid: ThreadId) -> Result<(), IpcError> {
        if self.owner_count as usize >= MAX_ENDPOINT_OWNERS {
            return Err(IpcError::ResourceExhausted);
        }
        self.owners[self.owner_count as usize] = tid;
        self.owner_count += 1;
        Ok(())
    }
}
