// ipc/stats/counters.rs — Compteurs statistiques IPC globaux pour Exo-OS
//
// Ce module fournit l'instrumentation centralisée pour l'ensemble du sous-système
// IPC. Chaque événement significatif (envoi, réception, drop, erreur, latence...)
// est enregistré via `IPC_STATS.record(StatEvent)`.
//
// Architecture :
//   - `StatEvent` : enum exhaustif de tous les événements IPC mesurables
//   - `IpcStats` : tableau de compteurs atomiques indexé par StatEvent
//   - `IPC_STATS` : instance statique globale (zéro allocation)
//   - `IpcStatsSnapshot` : instantané immutable pour inspection
//
// RÈGLE STATS-01 : tous les compteurs sont AtomicU64 — jamais de mutex.
// RÈGLE STATS-02 : record() est inlinée et O(1).
// RÈGLE STATS-03 : les canaux channel/ peuvent importer {IPC_STATS, StatEvent}
//                  directement depuis ce fichier.

use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// StatEvent — tous les événements mesurables du sous-système IPC
// ---------------------------------------------------------------------------

/// Identificateur d'événement IPC statistique.
///
/// Chaque variant correspond à un compteur dédié dans `IpcStats`.
/// L'ordre est stable — ne pas réorganiser sans mettre à jour STAT_COUNT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum StatEvent {
    // --- Messages ---
    /// Message envoyé avec succès
    MessageSent = 0,
    /// Message reçu avec succès
    MessageReceived = 1,
    /// Message abandonné (overflow/queue pleine)
    MessageDropped = 2,
    /// Message dont la livraison a échoué (endpoint mort, etc.)
    MessageFailed = 3,

    // --- Canaux ---
    /// Canal créé
    ChannelCreated = 4,
    /// Canal fermé
    ChannelClosed = 5,
    /// Canal détruit
    ChannelDestroyed = 6,
    /// Envoi bloquant (channel full, wait)
    ChannelSendBlocked = 7,
    /// Réception bloquante (channel empty, wait)
    ChannelRecvBlocked = 8,
    /// Timeout lors d'une opération sur canal
    ChannelTimeout = 9,

    // --- Mémoire partagée ---
    /// Allocation SHM réussie
    ShmAllocated = 10,
    /// Libération SHM
    ShmFreed = 11,
    /// Échec d'allocation SHM
    ShmAllocFailed = 12,
    /// Mapping SHM dans un espace processus
    ShmMapped = 13,
    /// Unmapping SHM
    ShmUnmapped = 14,

    // --- Futex ---
    /// Appel futex_wait()
    FutexWait = 15,
    /// Réveil futex_wake()
    FutexWake = 16,
    /// Timeout futex_wait()
    FutexTimeout = 17,
    /// Requeue futex
    FutexRequeue = 18,

    // --- Ring buffers ---
    /// Push dans un ring réussi
    RingPush = 19,
    /// Pop d'un ring réussi
    RingPop = 20,
    /// Ring plein (push échoué)
    RingFull = 21,
    /// Ring vide (pop échoué)
    RingEmpty = 22,

    // --- Endpoints ---
    /// Endpoint créé
    EndpointCreated = 23,
    /// Endpoint détruit
    EndpointDestroyed = 24,
    /// Connexion entre endpoints
    EndpointConnected = 25,
    /// Déconnexion entre endpoints
    EndpointDisconnected = 26,

    // --- Capabilities ---
    /// Vérification de capability réussie
    CapCheck = 27,
    /// Vérification de capability échouée (access denied)
    CapDenied = 28,
    /// Capability créée
    CapCreated = 29,
    /// Capability révoquée
    CapRevoked = 30,

    // --- RPC ---
    /// Appel RPC émis
    RpcCall = 31,
    /// Appel RPC complété
    RpcReturn = 32,
    /// Appel RPC timeout
    RpcTimeout = 33,
    /// Appel RPC erreur de protocole
    RpcProtocolError = 34,

    // --- Synchronisation ---
    /// Wake sur wait_queue IPC
    WaitQueueWake = 35,
    /// Timeout wait_queue
    WaitQueueTimeout = 36,
    /// Event set
    EventSet = 37,
    /// Event wait
    EventWait = 38,
    /// Barrière franchie
    BarrierCrossed = 39,
    /// Rendez-vous complété
    RendezvousMet = 40,

    // --- Transfert ---
    /// Transfert de descripteur entre processus
    DescriptorTransfer = 41,
    /// Transfert zero-copy
    ZeroCopyTransfer = 42,

    // --- Erreurs génériques ---
    /// Erreur générique IPC
    GenericError = 43,
    /// Paramètre invalide
    InvalidParam = 44,
}

/// Nombre total d'événements statistiques
pub const STAT_COUNT: usize = 45;

// ---------------------------------------------------------------------------
// IpcStats — table centrale de compteurs atomiques
// ---------------------------------------------------------------------------

/// Table de compteurs statistiques IPC.
///
/// Indexée par `StatEvent as usize`. Tous les compteurs sont AtomicU64.
/// `record()` est inlinée et se réduit à un `fetch_add` x86 LOCK.
pub struct IpcStats {
    counters: [AtomicU64; STAT_COUNT],
}

// SAFETY: AtomicU64 est Sync
unsafe impl Sync for IpcStats {}
unsafe impl Send for IpcStats {}

impl IpcStats {
    /// Constructeur const — utilisable en static
    pub const fn new() -> Self {
        // Macro locale : générer STAT_COUNT fois AtomicU64::new(0)
        // On ne peut pas utiliser [expr; N] avec AtomicU64 non-Copy,
        // donc on énumère explicitement.
        Self {
            counters: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ],
        }
    }

    /// Enregistre un événement IPC (O(1), inlineable).
    #[inline(always)]
    pub fn record(&self, event: StatEvent) {
        // SAFETY: event as usize < STAT_COUNT garanti par le repr(usize) borné
        debug_assert!((event as usize) < STAT_COUNT);
        self.counters[event as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre N occurrences d'un événement.
    #[inline(always)]
    pub fn record_n(&self, event: StatEvent, n: u64) {
        if n == 0 { return; }
        self.counters[event as usize].fetch_add(n, Ordering::Relaxed);
    }

    /// Lit la valeur courante d'un compteur.
    #[inline]
    pub fn get(&self, event: StatEvent) -> u64 {
        self.counters[event as usize].load(Ordering::Relaxed)
    }

    /// Remet un compteur à zéro.
    #[inline]
    pub fn reset(&self, event: StatEvent) {
        self.counters[event as usize].store(0, Ordering::Relaxed);
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset_all(&self) {
        for c in self.counters.iter() {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Produit un instantané immutable (copie des valeurs courantes).
    pub fn snapshot(&self) -> IpcStatsSnapshot {
        let mut values = [0u64; STAT_COUNT];
        for (i, c) in self.counters.iter().enumerate() {
            values[i] = c.load(Ordering::Relaxed);
        }
        IpcStatsSnapshot { values }
    }
}

// ---------------------------------------------------------------------------
// Instance statique globale
// ---------------------------------------------------------------------------

/// Instance globale des compteurs statistiques IPC.
///
/// Importable depuis n'importe quel fichier du module IPC :
/// ```rust
/// use crate::ipc::stats::counters::{IPC_STATS, StatEvent};
/// IPC_STATS.record(StatEvent::MessageSent);
/// ```
pub static IPC_STATS: IpcStats = IpcStats::new();

// ---------------------------------------------------------------------------
// IpcStatsSnapshot — instantané immutable
// ---------------------------------------------------------------------------

/// Copie instantanée de tous les compteurs IPC.
///
/// Obtenu via `IPC_STATS.snapshot()`. Les valeurs ne varient plus après
/// la capture.
#[derive(Debug, Clone)]
pub struct IpcStatsSnapshot {
    values: [u64; STAT_COUNT],
}

impl IpcStatsSnapshot {
    /// Récupère la valeur d'un compteur dans le snapshot.
    pub fn get(&self, event: StatEvent) -> u64 {
        self.values[event as usize]
    }

    /// Messages envoyés
    pub fn messages_sent(&self) -> u64 { self.get(StatEvent::MessageSent) }
    /// Messages reçus
    pub fn messages_received(&self) -> u64 { self.get(StatEvent::MessageReceived) }
    /// Messages abandonnés
    pub fn messages_dropped(&self) -> u64 { self.get(StatEvent::MessageDropped) }

    /// Allocations SHM
    pub fn shm_allocated(&self) -> u64 { self.get(StatEvent::ShmAllocated) }
    /// Libérations SHM
    pub fn shm_freed(&self) -> u64 { self.get(StatEvent::ShmFreed) }

    /// Appels RPC
    pub fn rpc_calls(&self) -> u64 { self.get(StatEvent::RpcCall) }
    /// Timeouts RPC
    pub fn rpc_timeouts(&self) -> u64 { self.get(StatEvent::RpcTimeout) }

    /// Futex waits
    pub fn futex_waits(&self) -> u64 { self.get(StatEvent::FutexWait) }
    /// Futex wakes
    pub fn futex_wakes(&self) -> u64 { self.get(StatEvent::FutexWake) }

    /// Taux de drop messages (0-100 si sent > 0)
    pub fn message_drop_rate_pct(&self) -> u64 {
        let sent = self.messages_sent();
        if sent == 0 { return 0; }
        (self.messages_dropped() * 100) / sent
    }

    /// Affiche un résumé formaté dans un buffer &mut str de capacité `buf`.
    /// Retourne le nombre de bytes écrits.
    pub fn format_summary(&self, buf: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct BufWriter<'a> {
            buf: &'a mut [u8],
            written: usize,
        }
        impl<'a> core::fmt::Write for BufWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let remaining = &mut self.buf[self.written..];
                let len = s.len().min(remaining.len());
                remaining[..len].copy_from_slice(&s.as_bytes()[..len]);
                self.written += len;
                Ok(())
            }
        }
        let mut w = BufWriter { buf, written: 0 };
        let _ = write!(
            w,
            "IPC_STATS: sent={} recv={} drop={} shm_alloc={} rpc={} futex_wait={} futex_wake={}",
            self.messages_sent(),
            self.messages_received(),
            self.messages_dropped(),
            self.shm_allocated(),
            self.rpc_calls(),
            self.futex_waits(),
            self.futex_wakes(),
        );
        w.written
    }
}
