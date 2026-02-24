// kernel/src/ipc/core/constants.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CONSTANTES GLOBALES IPC
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Toutes les constantes dimensionnantes de l'IPC sont centralisées ici.
// Toute modification doit être accompagnée d'une mise à jour du commentaire
// d'impact et d'une validation du budget mémoire du pool SHM.
// ═══════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// Contraintes messages
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un message inline (données copiées dans le ring).
/// 4080 = 4096 - 16 (header IPC) → un message tient dans une page 4 KiB.
pub const MAX_MSG_SIZE: usize = 4_080;

/// Taille de l'en-tête de message (MessageHeader dans le ring).
/// 16 bytes : msg_id(8) + flags(4) + len(2) + pad(2).
pub const MSG_HEADER_SIZE: usize = 16;

/// Taille totale d'un slot de ring = header + payload.
pub const RING_SLOT_SIZE: usize = MSG_HEADER_SIZE + MAX_MSG_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Dimensionnement des rings
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de slots dans un ring standard (SPSC/MPMC).
/// Puissance de 2 — masque mod = RING_SIZE - 1.
pub const RING_SIZE: usize = 4_096;

/// Masque de modulo pour un ring de RING_SIZE slots.
/// N/A si RING_SIZE n'est pas une puissance de 2 (vérifié statiquement).
pub const RING_MASK: usize = RING_SIZE - 1;

const _RING_SIZE_IS_POW2: () = assert!(
    RING_SIZE.is_power_of_two(),
    "RING_SIZE doit être une puissance de 2"
);

/// Taille minimale d'un ring batch (fusion ring).
pub const FUSION_RING_SIZE: usize = 256;

/// Seuil de batching pour le Fusion Ring (nombre de msgs avant flush forcé).
pub const FUSION_BATCH_THRESHOLD: usize = 16;

/// Délai maximal de rétention dans le Fusion Ring (en tick scheduler = 1ms à HZ=1000).
pub const FUSION_MAX_DELAY_TICKS: u64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Shared Memory
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de pages pré-allouées dans le pool SHM.
/// 256 pages × 4096 bytes = 1 MiB réservé au boot.
pub const SHM_POOL_PAGES: usize = 256;

/// Taille d'une page physique (identique à memory/core/constants.rs PAGE_SIZE).
pub const PAGE_SIZE: usize = 4_096;

/// Taille du pool SHM en bytes.
pub const SHM_POOL_BYTES: usize = SHM_POOL_PAGES * PAGE_SIZE;

/// Nombre maximal de régions SHM actives simultanément.
pub const SHM_MAX_REGIONS: usize = 1_024;

/// Alignement obligatoire des allocations SHM (NO_COW flag).
pub const SHM_ALIGN: usize = PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Endpoints et canaux
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal d'endpoints dans le registre global.
pub const MAX_ENDPOINTS: usize = 8_192;

/// Longueur maximale d'un nom d'endpoint (ASCII, null-terminé dans le registre).
pub const MAX_ENDPOINT_NAME_LEN: usize = 64;

/// File d'attente de connexions entrantes par endpoint.
pub const ENDPOINT_BACKLOG: usize = 32;

/// Nombre maximal de canaux ouverts simultanément (toutes directions).
pub const MAX_CHANNELS: usize = 65_536;

/// Nombre maximal de propriétaires concurrents d'un même endpoint.
pub const MAX_ENDPOINT_OWNERS: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// RPC
// ─────────────────────────────────────────────────────────────────────────────

/// Timeout RPC par défaut en nanosecondes (5 ms).
pub const RPC_DEFAULT_TIMEOUT_NS: u64 = 5_000_000;

/// Timeout RPC maximal (100 ms).
pub const RPC_MAX_TIMEOUT_NS: u64 = 100_000_000;

/// Nombre maximal de requêtes RPC en vol simultanément (par client).
pub const RPC_MAX_INFLIGHT: usize = 64;

/// Nombre maximal de tentatives de retry RPC (après timeout).
pub const RPC_MAX_RETRIES: usize = 3;

/// Magic RPC dans l'en-tête de protocole.
/// 0xEA045250 = 'E','A',0x04,'R','P' — marqueur de trame RPC.
pub const RPC_MAGIC: u32 = 0xEA04_5250;

/// Version du protocole RPC binaire.
pub const RPC_PROTOCOL_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Synchronisation
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur magique pour les futex IPC (distingue des futex mémoire normaux).
pub const IPC_FUTEX_MAGIC: u32 = 0x1FCF_07E0;

/// Timeout maximal d'une attente sur WaitQueue IPC (en nanosecondes) = 1 seconde.
pub const IPC_WAIT_MAX_NS: u64 = 1_000_000_000;

/// Nombre maximal de waiters simultanés sur un même rendezvous.
pub const RENDEZVOUS_MAX_WAITERS: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// Numéros de séquence
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de bits de la fenêtre glissante de séquence (détection d'ordonnancement).
pub const SEQ_WINDOW_BITS: u32 = 7; // fenêtre = 128 messages

/// Fenêtre de tolérance séquence : au-delà, le message est considéré hors-ordre.
pub const SEQ_WINDOW_SIZE: u64 = 1 << SEQ_WINDOW_BITS;

// ─────────────────────────────────────────────────────────────────────────────
// Performance / latence cibles
// ─────────────────────────────────────────────────────────────────────────────

/// Latence cible pour un fast IPC inline (sans syscall) : < 200 ns.
pub const TARGET_FAST_IPC_NS: u64 = 200;

/// Latence cible pour un message synchrone full path : < 2 µs.
pub const TARGET_SYNC_IPC_NS: u64 = 2_000;

/// Débit cible de msgs/s par canal SPSC : > 10 million msgs/s.
pub const TARGET_SPSC_THROUGHPUT: u64 = 10_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de version et limites globales
// ─────────────────────────────────────────────────────────────────────────────

/// Version du sous-système IPC (version interne kernel).
pub const IPC_VERSION: u32 = 1;

/// Nombre maximal de canaux simultanément ouverts.
/// Alias explicite de MAX_CHANNELS pour les re-exports publics.
pub const IPC_MAX_CHANNELS: usize = MAX_CHANNELS;

/// Nombre maximal d'endpoints dans le registre global.
/// Alias explicite de MAX_ENDPOINTS.
pub const IPC_MAX_ENDPOINTS: usize = MAX_ENDPOINTS;

/// Nombre maximal de processus pouvant détenir des ressources IPC.
pub const IPC_MAX_PROCESSES: usize = 65_536;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de messages (header)
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur magique dans l'en-tête de frame de message (MsgFrameHeader).
/// 0x4D534748 = 'M','S','G','H'
pub const MSG_HEADER_MAGIC: u32 = 0x4D53_4748;

// ─────────────────────────────────────────────────────────────────────────────
// Timeouts canal synchrone
// ─────────────────────────────────────────────────────────────────────────────

/// Timeout par défaut d'un canal synchrone (spin-wait) en nanosecondes = 5 ms.
pub const SYNC_CHANNEL_TIMEOUT_NS: u64 = 5_000_000;
