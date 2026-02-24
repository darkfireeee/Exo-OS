// kernel/src/memory/dma/completion/wakeup.rs
//
// Réveil de waiter sur complétion DMA.
//
// Ce module est la couche "haute" de notification : il associe un TID
// à une transaction en cours et déclenche le réveil via le handler enregistré
// par process/ (`DmaWakeupHandler`).
//
// Différence avec `core/wakeup_iface.rs` :
//   - `wakeup_iface.rs` définit le **trait** (Couche 0, interface minimale).
//   - `completion/wakeup.rs` gère la **table de waiters** et orchestre la
//     notification depuis le DmaCompletionManager lors d'une IRQ DMA.
//
// COUCHE 0 — aucune dépendance externe vers process/scheduler.

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicBool, Ordering};

use crate::memory::dma::core::types::{DmaTransactionId, DmaChannelId, DmaError};
use crate::memory::dma::core::wakeup_iface::wake_on_completion;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de la table de waiters (transactions en attente de complétion).
pub const WAKEUP_TABLE_SIZE: usize = 512;

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT D'UN WAITER
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
enum WaiterState {
    Free      = 0,
    Waiting   = 1,
    Completed = 2,
}

/// Un waiter enregistré pour une transaction DMA.
///
/// Alloué statiquement — pas de Box, pas de Vec.
#[repr(C, align(64))]
pub struct WaiterSlot {
    /// Transaction attendue.
    txn_id:     DmaTransactionId,
    /// Canal DMA.
    channel_id: DmaChannelId,
    /// TID du thread/processus qui attend (opaque, interprété par process/).
    waiter_tid: AtomicU64,
    /// État du waiter.
    state:      AtomicU8,
    /// Résultat disponible (true = succès).
    result_ok:  AtomicBool,
    /// Octets transférés (OK) ou code d'erreur (Err).
    result_val: AtomicU64,
    _pad:       [u8; 6],
}

const _: () = assert!(
    core::mem::size_of::<WaiterSlot>() <= 64,
    "WaiterSlot doit tenir dans 64 bytes"
);

impl WaiterSlot {
    const fn new() -> Self {
        WaiterSlot {
            txn_id:     DmaTransactionId::INVALID,
            channel_id: DmaChannelId(u32::MAX),
            waiter_tid: AtomicU64::new(0),
            state:      AtomicU8::new(WaiterState::Free as u8),
            result_ok:  AtomicBool::new(false),
            result_val: AtomicU64::new(0),
            _pad:       [0u8; 6],
        }
    }

    fn is_free(&self) -> bool {
        self.state.load(Ordering::Acquire) == WaiterState::Free as u8
    }

    fn is_waiting_for(&self, txn_id: DmaTransactionId) -> bool {
        self.state.load(Ordering::Acquire) == WaiterState::Waiting as u8
            && self.txn_id.0 == txn_id.0
    }

    /// Marque le slot comme attendant une transaction.
    ///
    /// # Safety
    /// Doit être appelé uniquement sur un slot `Free`.
    unsafe fn mark_waiting(
        &self,
        txn_id:     DmaTransactionId,
        channel_id: DmaChannelId,
        tid:        u64,
    ) {
        let ptr = self as *const _ as *mut WaiterSlot;
        (*ptr).txn_id     = txn_id;
        (*ptr).channel_id = channel_id;
        self.waiter_tid.store(tid, Ordering::Relaxed);
        self.result_ok.store(false, Ordering::Relaxed);
        self.result_val.store(0, Ordering::Relaxed);
        self.state.store(WaiterState::Waiting as u8, Ordering::Release);
    }

    /// Complète le slot avec un résultat.
    fn complete(&self, result: Result<usize, DmaError>) {
        match result {
            Ok(bytes) => {
                self.result_ok.store(true, Ordering::Relaxed);
                self.result_val.store(bytes as u64, Ordering::Relaxed);
            }
            Err(err) => {
                self.result_ok.store(false, Ordering::Relaxed);
                self.result_val.store(err as u64, Ordering::Relaxed);
            }
        }
        self.state.store(WaiterState::Completed as u8, Ordering::Release);
    }

    /// Libère le slot (retour à l'état Free).
    fn release(&self) {
        self.state.store(WaiterState::Free as u8, Ordering::Release);
    }

    fn result(&self) -> Result<usize, DmaError> {
        if self.result_ok.load(Ordering::Acquire) {
            Ok(self.result_val.load(Ordering::Relaxed) as usize)
        } else {
            Err(match self.result_val.load(Ordering::Relaxed) {
                0  => DmaError::NoChannel,
                1  => DmaError::OutOfMemory,
                2  => DmaError::InvalidParams,
                3  => DmaError::Timeout,
                4  => DmaError::HardwareError,
                5  => DmaError::IommuFault,
                6  => DmaError::NotInitialized,
                7  => DmaError::AlreadySubmitted,
                8  => DmaError::Cancelled,
                9  => DmaError::MisalignedBuffer,
                10 => DmaError::WrongZone,
                _  => DmaError::NotSupported,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE WAITERS
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale des waiters DMA.
///
/// Alloue un slot par transaction en attente. Quand l'IRQ DMA arrive,
/// `notify_completion` retrouve le slot, appelle le WakeupHandler,
/// puis libère le slot.
pub struct DmaWakeupTable {
    slots:            [WaiterSlot; WAKEUP_TABLE_SIZE],
    /// Nombre de slots actuellement occupés.
    pub active_count: core::sync::atomic::AtomicU32,
    /// Wakeups délivrés avec succès.
    pub delivered:    AtomicU64,
    /// Notifications orphelines (transaction inconnue).
    pub orphans:      AtomicU64,
    /// Timeouts détectés lors du nettoyage.
    pub timeouts:     AtomicU64,
}

// SAFETY: Les slots sont accédés via CAS sur state (Waiting → Completed).
unsafe impl Sync for DmaWakeupTable {}
unsafe impl Send for DmaWakeupTable {}

impl DmaWakeupTable {
    const fn new() -> Self {
        const SLOT: WaiterSlot = WaiterSlot::new();
        DmaWakeupTable {
            slots:        [SLOT; WAKEUP_TABLE_SIZE],
            active_count: core::sync::atomic::AtomicU32::new(0),
            delivered:    AtomicU64::new(0),
            orphans:      AtomicU64::new(0),
            timeouts:     AtomicU64::new(0),
        }
    }

    /// Enregistre un waiter pour `txn_id`.
    ///
    /// Le thread identifié par `tid` sera réveillé par `DmaWakeupHandler`
    /// lors de la complétion de la transaction.
    ///
    /// Retourne `false` si la table est pleine.
    pub fn register(
        &self,
        txn_id:     DmaTransactionId,
        channel_id: DmaChannelId,
        tid:        u64,
    ) -> bool {
        for slot in self.slots.iter() {
            if slot.is_free() {
                // Tenter une CAS Free → Waiting pour éviter les courses.
                if slot.state.compare_exchange(
                    WaiterState::Free as u8,
                    WaiterState::Waiting as u8,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ).is_ok() {
                    // SAFETY: On vient de passer le CAS, on est l'unique propriétaire.
                    unsafe { slot.mark_waiting(txn_id, channel_id, tid); }
                    self.active_count.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        false
    }

    /// Notifie la complétion d'une transaction et réveille le waiter.
    ///
    /// Appelé depuis le handler d'interruption DMA.
    /// Recherche le slot correspondant à `txn_id`, appelle `wake_on_completion`,
    /// puis libère le slot.
    ///
    /// Retourne `true` si un waiter a été trouvé et notifié.
    pub fn notify_completion(
        &self,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    ) -> bool {
        for slot in self.slots.iter() {
            if slot.is_waiting_for(txn_id) {
                let tid = slot.waiter_tid.load(Ordering::Acquire);
                slot.complete(result);

                // Appel vers le WakeupHandler enregistré par process/.
                wake_on_completion(tid, txn_id, result);

                self.delivered.fetch_add(1, Ordering::Relaxed);
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                slot.release();
                return true;
            }
        }
        self.orphans.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Nombre de waiters actifs.
    pub fn active(&self) -> u32 {
        self.active_count.load(Ordering::Relaxed)
    }
}

/// Table de wakeup DMA globale.
pub static DMA_WAKEUP_TABLE: DmaWakeupTable = DmaWakeupTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre un waiter pour une transaction DMA.
#[inline]
pub fn register_wakeup(
    txn_id:     DmaTransactionId,
    channel_id: DmaChannelId,
    tid:        u64,
) -> bool {
    DMA_WAKEUP_TABLE.register(txn_id, channel_id, tid)
}

/// Notifie la complétion d'une transaction (appelé par IRQ handler).
#[inline]
pub fn notify_completion(
    txn_id: DmaTransactionId,
    result: Result<usize, DmaError>,
) -> bool {
    DMA_WAKEUP_TABLE.notify_completion(txn_id, result)
}
