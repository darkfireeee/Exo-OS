// kernel/src/memory/dma/completion/handler.rs
//
// Gestionnaire de complétion DMA — enregistre les transactions en attente
// et déclenche le mécanisme de réveil via DmaWakeupHandler.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicBool, Ordering};

use crate::memory::dma::core::types::{DmaTransactionId, DmaError};
use crate::memory::dma::core::wakeup_iface::wake_on_completion;

// ─────────────────────────────────────────────────────────────────────────────
// SLOT D'ATTENTE
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de transactions en attente simultanément.
pub const MAX_PENDING_COMPLETIONS: usize = 512;

/// État d'un slot de completion.
#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
enum SlotState {
    Free       = 0,
    Waiting    = 1,
    Completed  = 2,
    Consumed   = 3,
}

/// Un slot de complétion pour une transaction DMA.
#[repr(C, align(64))]
struct CompletionSlot {
    /// Identifiant de la transaction associée.
    txn_id:        DmaTransactionId,
    /// ID du canal DMA.
    channel_id:    u32,
    /// TID du thread qui attend ce résultat.
    waiter_tid:    u64,
    /// État du slot.
    state:         AtomicU8,
    /// Résultat de la transaction (0 = pas encore).
    result_ok:     AtomicBool,
    /// Octets transférés (succès) ou code d'erreur (erreur).
    result_value:  AtomicU64,
    /// Timestamp de complétion (TSC).
    complete_tsc:  AtomicU64,
}

impl CompletionSlot {
    #[allow(dead_code)]
    const fn new() -> Self {
        CompletionSlot {
            txn_id:       DmaTransactionId::INVALID,
            channel_id:   u32::MAX,
            waiter_tid:   0,
            state:        AtomicU8::new(SlotState::Free as u8),
            result_ok:    AtomicBool::new(false),
            result_value: AtomicU64::new(0),
            complete_tsc: AtomicU64::new(0),
        }
    }

    fn is_free(&self) -> bool { self.state.load(Ordering::Acquire) == SlotState::Free as u8 }
    fn is_waiting(&self) -> bool { self.state.load(Ordering::Acquire) == SlotState::Waiting as u8 }
    fn is_completed(&self) -> bool { self.state.load(Ordering::Acquire) == SlotState::Completed as u8 }

    fn mark_waiting(&self, txn_id: DmaTransactionId, channel_id: u32, tid: u64) {
        // SAFETY: Le slot est free (garantie par alloc).
        unsafe {
            let ptr = self as *const _ as *mut CompletionSlot;
            (*ptr).txn_id     = txn_id;
            (*ptr).channel_id = channel_id;
            (*ptr).waiter_tid = tid;
        }
        self.state.store(SlotState::Waiting as u8, Ordering::Release);
    }

    fn mark_completed(&self, result: Result<usize, DmaError>) {
        match result {
            Ok(bytes) => {
                self.result_ok.store(true, Ordering::Relaxed);
                self.result_value.store(bytes as u64, Ordering::Release);
            }
            Err(e) => {
                self.result_ok.store(false, Ordering::Relaxed);
                self.result_value.store(e as u64, Ordering::Release);
            }
        }
        self.state.store(SlotState::Completed as u8, Ordering::Release);
    }

    fn consume(&self) -> Result<usize, DmaError> {
        self.state.store(SlotState::Consumed as u8, Ordering::Release);
        if self.result_ok.load(Ordering::Relaxed) {
            Ok(self.result_value.load(Ordering::Acquire) as usize)
        } else {
            let code = self.result_value.load(Ordering::Acquire) as u8;
            Err(match code {
                0 => DmaError::NoChannel,
                1 => DmaError::OutOfMemory,
                2 => DmaError::InvalidParams,
                3 => DmaError::Timeout,
                4 => DmaError::HardwareError,
                5 => DmaError::IommuFault,
                _ => DmaError::HardwareError,
            })
        }
    }

    fn reset(&self) {
        self.state.store(SlotState::Free as u8, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GESTIONNAIRE DE COMPLÉTION
// ─────────────────────────────────────────────────────────────────────────────

pub struct DmaCompletionManager {
    slots: [CompletionSlot; MAX_PENDING_COMPLETIONS],
    // Statistiques.
    total_registered:  AtomicU64,
    total_completed:   AtomicU64,
    total_errors:      AtomicU64,
}

// SAFETY: Les slots sont accédés via des atomiques — pas de lock global requis.
unsafe impl Sync for DmaCompletionManager {}
unsafe impl Send for DmaCompletionManager {}

impl DmaCompletionManager {
    const fn new() -> Self {
        DmaCompletionManager {
            // On ne peut pas faire [CompletionSlot::new(); 512] sans Copy.
            // On utilise MaybeUninit.
            slots: unsafe { core::mem::MaybeUninit::zeroed().assume_init() },
            total_registered: AtomicU64::new(0),
            total_completed:  AtomicU64::new(0),
            total_errors:     AtomicU64::new(0),
        }
    }

    /// Enregistre une transaction en attente.
    /// Doit être appelé après soumission au canal.
    pub fn register(&self, txn_id: DmaTransactionId, channel_id: u32, waiter_tid: u64) -> bool {
        for slot in self.slots.iter() {
            if slot.is_free() {
                // CAS pour éviter une race avec un concurrent.
                if slot.state.compare_exchange(
                    SlotState::Free as u8,
                    SlotState::Waiting as u8,
                    Ordering::AcqRel, Ordering::Relaxed
                ).is_ok() {
                    slot.mark_waiting(txn_id, channel_id, waiter_tid);
                    self.total_registered.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        false
    }

    /// Signale la complétion d'une transaction (appelé depuis le handler d'interruption DMA).
    /// Réveille le waiter si le handler est enregistré.
    pub fn complete(&self, txn_id: DmaTransactionId, result: Result<usize, DmaError>) {
        for slot in self.slots.iter() {
            if slot.is_waiting() {
                // On accède au txn_id en unsafe (champ non atomique).
                let slot_txn = unsafe {
                    *(slot as *const CompletionSlot as *const DmaTransactionId)
                };
                if slot_txn == txn_id {
                    let waiter = unsafe {
                        *((slot as *const CompletionSlot as *const u8)
                            .add(core::mem::offset_of!(CompletionSlot, waiter_tid)) as *const u64)
                    };
                    slot.mark_completed(result);
                    match result {
                        Ok(_)  => self.total_completed.fetch_add(1, Ordering::Relaxed),
                        Err(_) => self.total_errors.fetch_add(1, Ordering::Relaxed),
                    };
                    // Réveille le waiter via l'interface de réveil.
                    wake_on_completion(waiter, txn_id, result);
                    return;
                }
            }
        }
    }

    /// Poll non-bloquant — vérifie si une transaction est terminée.
    /// Retourne le résultat et libère le slot.
    pub fn poll(&self, txn_id: DmaTransactionId) -> Option<Result<usize, DmaError>> {
        for slot in self.slots.iter() {
            if slot.is_completed() {
                let slot_txn = unsafe {
                    *(slot as *const CompletionSlot as *const DmaTransactionId)
                };
                if slot_txn == txn_id {
                    let res = slot.consume();
                    slot.reset();
                    return Some(res);
                }
            }
        }
        None
    }

    /// Alias de `poll` utilisé par le polling actif.
    #[inline]
    pub fn query(&self, txn_id: DmaTransactionId) -> Option<Result<usize, DmaError>> {
        self.poll(txn_id)
    }

    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.total_registered.load(Ordering::Relaxed),
            self.total_completed.load(Ordering::Relaxed),
            self.total_errors.load(Ordering::Relaxed),
        )
    }
}

pub static DMA_COMPLETION: DmaCompletionManager = DmaCompletionManager::new();
