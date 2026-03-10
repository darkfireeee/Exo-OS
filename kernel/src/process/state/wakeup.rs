// kernel/src/process/state/wakeup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessWakeupHandler impl DmaWakeupHandler — RÈGLE PROC-02 (DOC4)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente le trait DmaWakeupHandler de memory/ pour que le sous-système DMA
// puisse réveiller un thread en attente d'une transaction DMA.
//
// Inversion de dépendance :
//   memory/ définit DmaWakeupHandler (trait).
//   process/ l'implémente ici (ProcessWakeupHandler).
//   process/mod.rs::init() appelle register_with_dma() pour s'enregistrer.

#![allow(dead_code)]

use core::sync::atomic::Ordering;
use crate::memory::dma::core::wakeup_iface::DmaWakeupHandler;
use crate::memory::dma::core::types::{DmaTransactionId, DmaError};
use crate::scheduler::sync::wait_queue::WaitQueue;

// ─────────────────────────────────────────────────────────────────────────────
// File d'attente globale pour les threads en attente de DMA
// ─────────────────────────────────────────────────────────────────────────────

/// File d'attente globale : threads en attente de complétion DMA.
static DMA_WAIT_QUEUE: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// Registre des résultats DMA par TID
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale du registre de complétions DMA en attente.
const DMA_COMPLETION_SLOTS: usize = 256;

/// Slot de complétion DMA.
#[repr(C)]
struct DmaCompletionSlot {
    /// TID cible (0 = slot libre).
    tid:    core::sync::atomic::AtomicU64,
    /// ID de transaction.
    txn_id: core::sync::atomic::AtomicU64,
    /// Résultat (0 = OK, errno négatif = erreur).
    result: core::sync::atomic::AtomicI64,
}

impl DmaCompletionSlot {
    const fn empty() -> Self {
        Self {
            tid:    core::sync::atomic::AtomicU64::new(0),
            txn_id: core::sync::atomic::AtomicU64::new(0),
            result: core::sync::atomic::AtomicI64::new(0),
        }
    }
}

/// Tableau statique des slots de complétion.
static DMA_COMPLETIONS: [DmaCompletionSlot; DMA_COMPLETION_SLOTS] = {
    // const construction over array with non-Copy const fn.
    const EMPTY: DmaCompletionSlot = DmaCompletionSlot::empty();
    [EMPTY; DMA_COMPLETION_SLOTS]
};

/// Enregistre la complétion d'une transaction pour le thread `tid`.
fn store_completion(tid: u64, txn_id: u64, result: i64) {
    // Cherche un slot libre par round-robin simple.
    for slot in &DMA_COMPLETIONS {
        if slot.tid.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            slot.txn_id.store(txn_id, Ordering::Release);
            slot.result.store(result, Ordering::Release);
            return;
        }
    }
    // Table pleine : ignorer (le thread remarquera le timeout).
}

/// Récupère et efface la complétion pour le thread `tid` et la transaction `txn_id`.
/// Retourne Some(result) ou None.
pub fn consume_completion(tid: u64, txn_id: u64) -> Option<i64> {
    for slot in &DMA_COMPLETIONS {
        let stored_tid = slot.tid.load(Ordering::Acquire);
        if stored_tid == tid {
            let stored_txn = slot.txn_id.load(Ordering::Acquire);
            if stored_txn == txn_id {
                let result = slot.result.load(Ordering::Acquire);
                slot.tid.store(0, Ordering::Release);
                return Some(result);
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessWakeupHandler
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation de DmaWakeupHandler pour le module process/.
/// Instance statique unique enregistrée auprès de memory/dma/ à l'init.
pub struct ProcessWakeupHandler;

/// Convertit une DmaError en code errno négatif (Linux compatible).
#[inline]
fn dma_error_to_errno(e: DmaError) -> i64 {
    match e {
        DmaError::NoChannel        => -11,   // EAGAIN
        DmaError::OutOfMemory      => -12,   // ENOMEM
        DmaError::InvalidParams    => -22,   // EINVAL
        DmaError::Timeout          => -110,  // ETIMEDOUT
        DmaError::HardwareError    => -5,    // EIO
        DmaError::IommuFault       => -14,   // EFAULT
        DmaError::NotInitialized   => -6,    // ENXIO
        DmaError::AlreadySubmitted => -22,   // EINVAL
        DmaError::Cancelled        => -125,  // ECANCELED
        DmaError::MisalignedBuffer => -22,   // EINVAL
        DmaError::WrongZone        => -22,   // EINVAL
        DmaError::NotSupported     => -95,   // ENOTSUP
    }
}

impl DmaWakeupHandler for ProcessWakeupHandler {
    /// Appelé par le driver DMA quand une transaction se termine.
    /// Réveille le thread `tid` en attente dans DMA_WAIT_QUEUE.
    fn wake_on_completion(
        &self,
        tid:    u64,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    ) {
        // Convertit le résultat en i64 (bytes ou errno négatif)
        let result_i64 = match result {
            Ok(bytes)  => bytes as i64,
            Err(e)     => dma_error_to_errno(e),
        };
        // Stocker le résultat pour que le thread puisse le lire après réveil.
        store_completion(tid, txn_id.0, result_i64);
        // Réveiller tous les threads en attente DMA ;
        // chacun vérifiera si c'est son TID.
        DMA_WAIT_QUEUE.notify_all();
    }

    /// Appelé par le driver DMA en cas d'erreur fatale sur un canal.
    /// Réveille tous les threads en attente sur ce canal.
    fn wake_all_on_error(
        &self,
        channel_id: u32,
        error:      DmaError,
    ) {
        let errno = dma_error_to_errno(error);
        // Marquer toutes les transactions de ce canal comme échouées.
        for slot in &DMA_COMPLETIONS {
            let tid_stored = slot.tid.load(Ordering::Acquire);
            if tid_stored != 0 {
                let txn = slot.txn_id.load(Ordering::Acquire);
                // Les TXN IDs d'un canal possèdent channel_id dans les bits hauts.
                if (txn >> 32) as u32 == channel_id {
                    slot.result.store(errno, Ordering::Release);
                }
            }
        }
        DMA_WAIT_QUEUE.notify_all();
    }
}

/// Instance statique enregistrée auprès de memory/dma/.
pub static PROCESS_WAKEUP_HANDLER: ProcessWakeupHandler = ProcessWakeupHandler;

/// Enregistre PROCESS_WAKEUP_HANDLER auprès du sous-système DMA.
/// Appelé depuis process::init() (RÈGLE PROC-02).
pub fn register_with_dma() {
    // SAFETY: appelé une seule fois depuis process::init() après scheduler::init().
    unsafe {
        crate::memory::dma::register_wakeup_handler(&PROCESS_WAKEUP_HANDLER);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée pour les threads en attente de DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Attend la complétion d'une transaction DMA.
/// Le thread courant se bloque dans DMA_WAIT_QUEUE jusqu'à réveil.
/// Retourne le résultat ou -EINTR si signal reçu.
pub fn wait_for_dma(
    tcb:    &crate::scheduler::core::task::ThreadControlBlock,
    tid:    u64,
    txn_id: u64,
) -> i64 {
    loop {
        // Vérifier si la complétion est déjà arrivée (pas de spurious).
        if let Some(result) = consume_completion(tid, txn_id) {
            return result;
        }
        // Vérifier signal avant de se bloquer.
        if tcb.signal_pending.load(Ordering::Acquire) {
            return -4; // -EINTR
        }
        // SAFETY: tcb pointe vers le TCB courant, pas d'alias &mut actif.
        unsafe { DMA_WAIT_QUEUE.wait_interruptible(tcb as *const _ as *mut _); }
    }
}
