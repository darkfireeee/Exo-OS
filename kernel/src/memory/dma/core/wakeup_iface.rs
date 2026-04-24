// kernel/src/memory/dma/core/wakeup_iface.rs
//
// Interface de réveil DMA — RÈGLE ARCHITECTURALE CRITIQUE.
//
// Le sous-système DMA (Couche 0) doit pouvoir réveiller des processus
// au terme d'un transfert SANS dépendre de process/ ou scheduler/.
// On résout cela par inversion de dépendance :
//   — DMA définit le trait `DmaWakeupHandler` ici (Couche 0).
//   — process/ implémente ce trait et l'enregistre via `register_wakeup_handler()`.
//
// COUCHE 0 — aucune dépendance vers scheduler/ipc/fs/process.

use crate::memory::dma::core::types::{DmaError, DmaTransactionId};
use core::sync::atomic::{AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT DE RÉVEIL
// ─────────────────────────────────────────────────────────────────────────────

/// Trait que le sous-système process/ doit implémenter pour recevoir les
/// notifications de fin de transfert DMA.
///
/// Ce trait est intentionnellement minimal pour limiter le couplage.
pub trait DmaWakeupHandler: Send + Sync {
    /// Réveille le waiter associé à `tid` lorsqu'une transaction DMA se termine.
    ///
    /// `tid` — identifiant du thread/processus à réveiller.
    /// `txn_id` — transaction terminée.
    /// `result` — `Ok(bytes_transferred)` ou `Err(DmaError)`.
    fn wake_on_completion(
        &self,
        tid: u64,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    );

    /// Notifie qu'une erreur fatale s'est produite sur un canal.
    /// Tous les waiters sur ce canal doivent être débloqués avec une erreur.
    fn wake_all_on_error(&self, channel_id: u32, error: DmaError);
}

// ─────────────────────────────────────────────────────────────────────────────
// HANDLER PAR DÉFAUT (NO-OP)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler no-op utilisé avant l'enregistrement depuis process/.
/// Log via le compteur d'appels perdus pour détecter les notifications orphelines.
struct NopWakeupHandler {
    lost_wakeups: AtomicUsize,
}

impl DmaWakeupHandler for NopWakeupHandler {
    fn wake_on_completion(
        &self,
        _tid: u64,
        _txn_id: DmaTransactionId,
        _result: Result<usize, DmaError>,
    ) {
        self.lost_wakeups.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_all_on_error(&self, _channel_id: u32, _error: DmaError) {
        self.lost_wakeups.fetch_add(1, Ordering::Relaxed);
    }
}

static NOP_HANDLER: NopWakeupHandler = NopWakeupHandler {
    lost_wakeups: AtomicUsize::new(0),
};

/// Retourne le nombre de wakeups perdus (handler non encore enregistré).
pub fn lost_wakeup_count() -> usize {
    NOP_HANDLER.lost_wakeups.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRE GLOBAL DU HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Pointeur atomique vers le handler actif.
/// Encode `*const dyn DmaWakeupHandler` via deux mots (fat pointer = 2 × usize).
///
/// En no_std/bare-metal on encode le fat pointer comme deux AtomicUsize adjacents.
/// Le registre est write-once : on ne supporte pas de swap dynamique.

struct WakeupHandlerSlot {
    data: AtomicUsize,
    vtbl: AtomicUsize,
    registered: AtomicUsize, // 0=nop, 1=real handler
}

// SAFETY: Le slot est write-once; les lectures sont protégées par la barrière Acquire.
unsafe impl Sync for WakeupHandlerSlot {}

static WAKEUP_SLOT: WakeupHandlerSlot = WakeupHandlerSlot {
    data: AtomicUsize::new(0),
    vtbl: AtomicUsize::new(0),
    registered: AtomicUsize::new(0),
};

/// Enregistre le handler de réveil DMA.
///
/// # Safety
/// - Doit être appelé une seule fois, au boot, depuis process/ ou le kernel init.
/// - `handler` doit avoir une durée de vie statique (`'static`).
/// - Aucune transaction DMA ne doit être en cours lors de l'appel.
pub unsafe fn register_wakeup_handler(handler: &'static dyn DmaWakeupHandler) {
    let fat: (*const (), *const ()) = core::mem::transmute(handler);
    WAKEUP_SLOT.data.store(fat.0 as usize, Ordering::Release);
    WAKEUP_SLOT.vtbl.store(fat.1 as usize, Ordering::Release);
    WAKEUP_SLOT.registered.store(1, Ordering::Release);
}

/// Récupère le handler actif (nop si non enregistré).
#[inline]
fn active_handler() -> &'static dyn DmaWakeupHandler {
    if WAKEUP_SLOT.registered.load(Ordering::Acquire) == 0 {
        return &NOP_HANDLER;
    }
    let data = WAKEUP_SLOT.data.load(Ordering::Acquire) as *const ();
    let vtbl = WAKEUP_SLOT.vtbl.load(Ordering::Acquire) as *const ();
    // SAFETY: Le fat pointer a été stocké correctement via register_wakeup_handler().
    unsafe {
        let fat: (*const (), *const ()) = (data, vtbl);
        core::mem::transmute::<(*const (), *const ()), &'static dyn DmaWakeupHandler>(fat)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FONCTIONS PUBLIQUES
// ─────────────────────────────────────────────────────────────────────────────

/// Réveille un thread/processus après completion d'une transaction DMA.
#[inline]
pub fn wake_on_completion(tid: u64, txn_id: DmaTransactionId, result: Result<usize, DmaError>) {
    active_handler().wake_on_completion(tid, txn_id, result);
}

/// Réveille tous les waiters d'un canal sur erreur fatale.
#[inline]
pub fn wake_all_on_error(channel_id: u32, error: DmaError) {
    active_handler().wake_all_on_error(channel_id, error);
}

/// Indique si un handler réel a été enregistré.
#[inline]
pub fn has_real_handler() -> bool {
    WAKEUP_SLOT.registered.load(Ordering::Relaxed) != 0
}
