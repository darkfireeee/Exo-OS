// kernel/src/ipc/core/types.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// TYPES FONDAMENTAUX IPC — MessageId, ChannelId, EndpointId, Cookie
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE IPC-COUCHE : ipc/ se situe en Couche 2a.
//   DÉPEND DE : memory/ (Couche 0) + scheduler/ (Couche 1) + security/capability/
//   N'IMPORTE JAMAIS : fs/, process/ (Couches supérieures)
//
// RÈGLE UNSAFE : tout bloc unsafe précédé de // SAFETY:
// RÈGLE NO-ALLOC : ce fichier — zéro Vec/Box/Arc (types de base uniquement)
// ═══════════════════════════════════════════════════════════════════════════════

use core::fmt;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, Ordering};

// Re-export ProcessId depuis scheduler (ProcessId est défini là-bas comme ProcessId(pub u32)).
pub use crate::scheduler::ProcessId;

// ─────────────────────────────────────────────────────────────────────────────
// Identifiants opaques — Newtype wrappers autour de u64
// Les valeurs 0 sont réservées comme "invalide / non initialisé".
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un message IPC.
/// Généré de manière monotone, jam ais réutilisé (rollover = panique en debug).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct MessageId(pub NonZeroU64);

impl MessageId {
    /// Crée un MessageId depuis une valeur brute non-nulle.
    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    /// Crée un MessageId depuis une valeur brute sans vérification.
    ///
    /// # Safety
    /// `v` ne doit pas être nul.
    #[inline(always)]
    pub unsafe fn new_unchecked(v: u64) -> Self {
        Self(NonZeroU64::new_unchecked(v))
    }

    /// Retourne la valeur sous-jacente.
    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgId({})", self.0.get())
    }
}

/// Identifiant unique d'un canal IPC (bidirectionnel ou unidirectionnel).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ChannelId(pub NonZeroU64);

impl ChannelId {
    /// Sentinel « pendant » — valeur invalide utilisée comme valeur initiale
    /// avant qu'un `ChannelId` réel soit alloué.
    pub const DANGLING: Self = Self(unsafe { NonZeroU64::new_unchecked(u64::MAX) });

    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChanId({})", self.0.get())
    }
}

/// Identifiant unique d'un endpoint (point de communication nommé).
/// Un endpoint peut accepter des connexions entrantes.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct EndpointId(pub NonZeroU64);

impl EndpointId {
    #[inline(always)]
    pub fn new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(Self)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0.get()
    }

    /// Sentinel « invalide » — valeur sentinelle utilisée à la place de
    /// `EndpointId(0)` qui est impossible (`NonZeroU64`).
    pub const INVALID: Self = Self(unsafe { NonZeroU64::new_unchecked(u64::MAX) });

    /// Convertit en ObjectId pour le système de capabilities.
    /// Les 32 bits hauts encodent le type (0x01 = endpoint IPC).
    #[inline(always)]
    pub fn to_object_id(self) -> u64 {
        (0x01u64 << 32) | (self.0.get() & 0xFFFF_FFFF)
    }
}

impl fmt::Debug for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EpId({})", self.0.get())
    }
}

/// Cookie opaque — valeur 64 bits arbitraire attachée à un message ou connexion.
/// Permet de corréler des requêtes/réponses sans état côté serveur.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct Cookie(pub u64);

impl Cookie {
    pub const ZERO: Self = Self(0);

    #[inline(always)]
    pub fn new(v: u64) -> Self {
        Self(v)
    }

    #[inline(always)]
    pub fn get(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Générateurs monotones thread-safe
// ─────────────────────────────────────────────────────────────────────────────

/// Générateur d'identifiants monotones pour les messages.
static MSG_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue un MessageId unique, monotone.
/// En cas de rollover (u64 max), le système est en panne — panique en debug.
#[inline]
pub fn alloc_message_id() -> MessageId {
    let v = MSG_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "MessageId counter rollover — system exhausted");
    // SAFETY: v est issu d'un fetch_add depuis 1 — jamais nul jusqu'au rollover.
    MessageId(unsafe { NonZeroU64::new_unchecked(v) })
}

/// Générateur d'identifiants monotones pour les canaux.
static CHAN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue un ChannelId unique, monotone.
#[inline]
pub fn alloc_channel_id() -> ChannelId {
    let v = CHAN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "ChannelId counter rollover");
    // SAFETY: v >= 1 par construction.
    ChannelId(unsafe { NonZeroU64::new_unchecked(v) })
}

/// Générateur d'identifiants monotones pour les endpoints.
static EP_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Alloue un EndpointId unique, monotone.
#[inline]
pub fn alloc_endpoint_id() -> EndpointId {
    let v = EP_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    debug_assert!(v != 0, "EndpointId counter rollover");
    // SAFETY: v >= 1 par construction.
    EndpointId(unsafe { NonZeroU64::new_unchecked(v) })
}

// ─────────────────────────────────────────────────────────────────────────────
// Drapeaux de message
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux bitmask pour un message IPC.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct MsgFlags(pub u32);

impl MsgFlags {
    /// Message temps-réel (priorité maximale dans les queues).
    pub const RT: Self = Self(1 << 0);
    /// Message de réponse (corrèle à un MessageId existant).
    pub const REPLY: Self = Self(1 << 1);
    /// Zero-copy : le contenu est une référence physique, pas une copie.
    pub const ZEROCOPY: Self = Self(1 << 2);
    /// Broadcast : livrer à tous les récepteurs.
    pub const BROADCAST: Self = Self(1 << 3);
    /// Message d'erreur.
    pub const ERROR: Self = Self(1 << 4);
    /// Synchrone : bloque l'émetteur jusqu'à l'acquittement.
    pub const SYNC: Self = Self(1 << 5);
    /// Non bloquant : retourne immédiatement si queue pleine.
    pub const NOWAIT: Self = Self(1 << 6);

    #[inline(always)]
    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    #[inline(always)]
    pub fn set(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    #[inline(always)]
    pub fn clear(self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }

    /// Retourne la valeur brute du bitmask (compatibilité bitflags).
    #[inline(always)]
    pub fn bits(self) -> u32 {
        self.0
    }

    /// Construit depuis une valeur brute en tronquant les bits inconnus.
    #[inline(always)]
    pub fn from_bits_truncate(v: u32) -> Self {
        Self(v & 0x7F)
    }

    /// Positionne un flag en place (compatibilité bitflags `.insert()`).
    #[inline(always)]
    pub fn insert(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    /// Retire un flag en place.
    #[inline(always)]
    pub fn remove(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultats et erreurs IPC
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs IPC — exhaustive, pas de _ catch-all.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum IpcError {
    /// Opération non bloquante sur une file pleine.
    WouldBlock = 1,
    /// Endpoint introuvable dans le registre.
    EndpointNotFound = 2,
    /// Canal fermé par l'autre extrémité.
    ChannelClosed = 3,
    /// Permission refusée par le système de capabilities.
    PermissionDenied = 4,
    /// Message trop grand (dépasse MAX_MSG_SIZE).
    MessageTooLarge = 5,
    /// Timeout expiré.
    Timeout = 6,
    /// Ressource IPC épuisée (pool SHM, CapToken, etc.).
    ResourceExhausted = 7,
    /// Connexion refusée (endpoint non en écoute).
    ConnRefused = 8,
    /// Déjà connecté.
    AlreadyConnected = 9,
    /// Paramètre non valide.
    InvalidParam = 10,
    /// Échec handshake (version, magic mismatch).
    HandshakeFailed = 11,
    /// Opération interrompue par un signal.
    Interrupted = 12,
    /// Erreur interne inattendue du kernel.
    InternalError = 13,
    /// Pool SHM saturé.
    ShmPoolFull = 14,
    /// Séquence hors ordre.
    OutOfOrder = 15,
    /// Handle IPC invalide ou expiré.
    InvalidHandle = 16,
    /// Canal / connexion fermé(e) — alias expressif pour ChannelClosed.
    Closed = 17,
    /// Erreur interne kernel — alias expressif pour InternalError.
    Internal = 18,
    /// Argument / données invalide(s) — alias expressif pour InvalidParam.
    Invalid = 19,
    /// File/ring pleine.
    Full = 20,
    /// Boucle de routage détectée.
    Loop = 21,
    /// Ressource introuvable.
    NotFound = 22,
    /// Endpoint nul (EndpointId 0) passé à une opération.
    NullEndpoint = 23,
    /// Endpoint invalide (corrompu, désalloué, mauvais type).
    InvalidEndpoint = 24,
    /// Opération à réessayer (backoff exponentiel).
    Retry = 25,
    /// Argument invalide — alias pour InvalidParam avec sémantique plus forte.
    InvalidArgument = 26,
    /// Ressources IPC épuisées (table pleine, pool vide, etc.).
    OutOfResources = 27,
    /// File/ring pleine lors d'un push (non bloquant).
    QueueFull = 28,
    /// File/ring vide lors d'un pop (non bloquant).
    QueueEmpty = 29,
    /// Erreur de protocole (magic/version mismatch, séquence invalide).
    ProtocolError = 30,
    /// Échec de mapping mémoire IPC (SHM, zero-copy page fault).
    MappingFailed = 31,
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WouldBlock => write!(f, "IPC: opération non bloquante sur file pleine"),
            Self::EndpointNotFound => write!(f, "IPC: endpoint introuvable"),
            Self::ChannelClosed => write!(f, "IPC: canal fermé"),
            Self::PermissionDenied => write!(f, "IPC: accès refusé (capability)"),
            Self::MessageTooLarge => write!(f, "IPC: message trop grand"),
            Self::Timeout => write!(f, "IPC: timeout expiré"),
            Self::ResourceExhausted => write!(f, "IPC: ressource épuisée"),
            Self::ConnRefused => write!(f, "IPC: connexion refusée"),
            Self::AlreadyConnected => write!(f, "IPC: déjà connecté"),
            Self::InvalidParam => write!(f, "IPC: paramètre invalide"),
            Self::HandshakeFailed => write!(f, "IPC: échec handshake"),
            Self::Interrupted => write!(f, "IPC: interrompu par signal"),
            Self::InternalError => write!(f, "IPC: erreur interne"),
            Self::ShmPoolFull => write!(f, "IPC: pool SHM saturé"),
            Self::OutOfOrder => write!(f, "IPC: séquence hors ordre"),
            Self::InvalidHandle => write!(f, "IPC: handle invalide"),
            Self::Closed => write!(f, "IPC: connexion fermée"),
            Self::Internal => write!(f, "IPC: erreur interne kernel"),
            Self::Invalid => write!(f, "IPC: données invalides"),
            Self::Full => write!(f, "IPC: file pleine"),
            Self::Loop => write!(f, "IPC: boucle de routage détectée"),
            Self::NotFound => write!(f, "IPC: ressource introuvable"),
            Self::NullEndpoint => write!(f, "IPC: endpoint nul"),
            Self::InvalidEndpoint => write!(f, "IPC: endpoint invalide"),
            Self::Retry => write!(f, "IPC: réessayer l'opération"),
            Self::InvalidArgument => write!(f, "IPC: argument invalide"),
            Self::OutOfResources => write!(f, "IPC: ressources épuisées"),
            Self::QueueFull => write!(f, "IPC: file pleine (push refusé)"),
            Self::QueueEmpty => write!(f, "IPC: file vide (pop échoué)"),
            Self::ProtocolError => write!(f, "IPC: erreur de protocole (magic/version)"),
            Self::MappingFailed => write!(f, "IPC: échec de mapping mémoire"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageFlags — drapeaux bitmask pour les messages dans message/
// ─────────────────────────────────────────────────────────────────────────────

/// Drapeaux de message IPC (utilisés par message/builder.rs, serializer.rs, etc.)
/// Représenté sur 16 bits pour tenir dans MsgFrameHeader.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct MessageFlags(pub u16);

impl MessageFlags {
    /// Aucun flag positionné.
    pub const NONE: Self = Self(0);
    /// Message temps-réel.
    pub const RT: Self = Self(1 << 0);
    /// Message de réponse.
    pub const REPLY: Self = Self(1 << 1);
    /// Zero-copy.
    pub const ZEROCOPY: Self = Self(1 << 2);
    /// Broadcast.
    pub const BROADCAST: Self = Self(1 << 3);
    /// Erreur.
    pub const ERROR: Self = Self(1 << 4);
    /// Synchrone.
    pub const SYNC: Self = Self(1 << 5);
    /// Non bloquant.
    pub const NOWAIT: Self = Self(1 << 6);

    /// Retourne la valeur brute.
    #[inline(always)]
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Construit depuis une valeur brute en tronquant les bits inconnus.
    #[inline(always)]
    pub fn from_bits_truncate(v: u16) -> Self {
        Self(v & 0x7F)
    }

    #[inline(always)]
    pub fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) == flag.0
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageType — type sémantique d'un message
// ─────────────────────────────────────────────────────────────────────────────

/// Type sémantique d'un message IPC.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum MessageType {
    #[default]
    /// Message de données brutes.
    Data = 0,
    /// Message de contrôle du canal.
    Control = 1,
    /// Notification de signal.
    Signal = 2,
    /// Réponse RPC.
    RpcReply = 3,
}

impl MessageType {
    /// Crée un MessageType depuis un octet brut.
    /// Valeurs inconnues → `Data` (politique dégradée, pas de panique).
    #[inline(always)]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Data,
            1 => Self::Control,
            2 => Self::Signal,
            3 => Self::RpcReply,
            _ => Self::Data,
        }
    }

    /// Retourne la valeur brute.
    #[inline(always)]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IpcCapError — variante IPC de CapError (v6 — accès via security::access_control)
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur de capability spécifique à IPC.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum IpcCapError {
    /// Token révoqué ou génération incorrecte.
    Revoked = 1,
    /// Objet capability introuvable.
    ObjectNotFound = 2,
    /// Droits insuffisants pour cette opération.
    InsufficientRights = 3,
    /// Délégation interdite (flag DELEGATE absent).
    DelegationDenied = 4,
}

impl From<IpcCapError> for IpcError {
    fn from(e: IpcCapError) -> Self {
        match e {
            IpcCapError::Revoked => IpcError::PermissionDenied,
            IpcCapError::ObjectNotFound => IpcError::EndpointNotFound,
            IpcCapError::InsufficientRights => IpcError::PermissionDenied,
            IpcCapError::DelegationDenied => IpcError::PermissionDenied,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Spectre v1 mitigation — array_index_nospec (RÈGLE IPC-08)
// ─────────────────────────────────────────────────────────────────────────────

/// Mitige Spectre v1 lors des accès aux buffers indexés.
///
/// Retourne `index` si `index < size`, 0 sinon — en utilisant un masque
/// calculé sans branchement sur le bit de signe du résultat `index - size`.
///
/// Le CPU ne peut pas spéculer au-delà des bornes du buffer car le masque
/// force l'index hors-bornes à 0 avant l'accès mémoire.
///
/// UTILISATION OBLIGATOIRE (IPC-08) sur tout accès de type :
///   `buffer[unsafe_index_from_user_or_ring]`
///
/// # Exemple
/// ```no_run
/// let safe_idx = array_index_nospec(user_idx, RING_SIZE);
/// let cell = &buffer[safe_idx];
/// ```
#[inline(always)]
pub fn array_index_nospec(index: usize, size: usize) -> usize {
    // Technique Linux kernel : arithmetic right-shift du signe de (index - size).
    // Si index < size  : index.wrapping_sub(size) → valeur négative (signed) → MSB=1
    //                    → shift donne 0xFFFF...FF → mask est all-ones → index inchangé.
    // Si index >= size : index.wrapping_sub(size) → ≥ 0 (signed) → MSB=0
    //                    → shift donne 0x0000...00 → mask est all-zeros → résultat = 0.
    let mask = (index.wrapping_sub(size) as isize >> (isize::BITS - 1)) as usize;
    index & mask
}
