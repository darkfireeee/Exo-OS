// kernel/src/ipc/ring/batch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BATCH TRANSFERS — transferts groupés pour amortir le coût par message
// (Exo-OS · IPC Couche 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Au lieu d'envoyer chaque message individuellement (coût fixe par message :
// atomics, barrières), le batching groupe N messages en une seule opération.
//
// BÉNÉFICES :
//   • Réduit le nombre d'opérations atomiques de N → 1.
//   • Améliore la localité cache (N messages dans la même cache line).
//   • Déblogage en masse : wake un seul waiter pour N messages.
//
// CONTRAINTE : introduit une latence de buffering (FUSION_BATCH_THRESHOLD msgs
//              ou FUSION_MAX_DELAY_TICKS ticks de scheduler).
// ═══════════════════════════════════════════════════════════════════════════════

use super::spsc::SpscRing;
use crate::ipc::core::constants::FUSION_BATCH_THRESHOLD;
use crate::ipc::core::{IpcError, MsgFlags};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// BatchBuffer — tampon local pour les messages en attente d'envoi
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un batch (limité par FUSION_BATCH_THRESHOLD).
pub const MAX_BATCH_SIZE: usize = 32;

/// Un message en attente dans le batch buffer.
#[derive(Clone)]
pub struct BatchEntry {
    /// Flags du message.
    pub flags: MsgFlags,
    /// Longueur du payload.
    pub len: u16,
    /// Payload inline (max 4080 bytes, mais en pratique ≤ 512 pour batching).
    pub data: [u8; 512],
}

impl BatchEntry {
    pub const fn zeroed() -> Self {
        Self {
            flags: MsgFlags(0),
            len: 0,
            data: [0u8; 512],
        }
    }
}

/// Tampon de batch côté producteur.
/// Accumulé localement, vidé vers le ring en une seule opération.
pub struct BatchBuffer {
    /// Messages en attente de flush.
    entries: [BatchEntry; MAX_BATCH_SIZE],
    /// Nombre de messages accumulés.
    count: usize,
    /// Tick d'expiration : au-delà, flush forcé.
    expire_tick: AtomicU64,
}

impl BatchBuffer {
    pub const fn new() -> Self {
        const ZERO: BatchEntry = BatchEntry::zeroed();
        Self {
            entries: [ZERO; MAX_BATCH_SIZE],
            count: 0,
            expire_tick: AtomicU64::new(0),
        }
    }

    /// Ajoute un message au batch. Retourne `true` si le batch doit être flushé maintenant.
    pub fn add(&mut self, src: &[u8], flags: MsgFlags, current_tick: u64) -> bool {
        // Limite de payload par message dans un batch.
        let len = src.len().min(512);
        if self.count == 0 {
            // Premier message du nouveau batch — définir l'expiration.
            self.expire_tick.store(
                current_tick + crate::ipc::core::constants::FUSION_MAX_DELAY_TICKS,
                Ordering::Relaxed,
            );
        }
        let idx = self.count;
        if idx < MAX_BATCH_SIZE {
            let entry = &mut self.entries[idx];
            entry.flags = flags;
            entry.len = len as u16;
            entry.data[..len].copy_from_slice(&src[..len]);
            self.count += 1;
        }
        // Flush si : batch plein OU seuil atteint.
        self.count >= FUSION_BATCH_THRESHOLD || self.count >= MAX_BATCH_SIZE
    }

    /// Vérifie si le batch a expiré (tick courant > expire_tick).
    #[inline(always)]
    pub fn is_expired(&self, current_tick: u64) -> bool {
        self.count > 0 && current_tick >= self.expire_tick.load(Ordering::Relaxed)
    }

    /// Retourne le nombre de messages accumulés.
    #[inline(always)]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Vide le batch vers un ring SPSC.
    /// Retourne le nombre de messages effectivement envoyés.
    pub fn flush_to_ring(&mut self, ring: &SpscRing) -> usize {
        let mut sent = 0;
        for i in 0..self.count {
            let entry = &self.entries[i];
            let data = &entry.data[..entry.len as usize];
            match ring.push_copy(data, entry.flags) {
                Ok(_) => sent += 1,
                Err(_) => break, // ring plein — réessayer au prochain tick
            }
        }
        // Décaler les messages non envoyés vers le début.
        if sent > 0 && sent < self.count {
            for i in 0..(self.count - sent) {
                self.entries[i] = self.entries[sent + i].clone();
            }
        }
        self.count -= sent;
        if self.count == 0 {
            self.expire_tick.store(0, Ordering::Relaxed);
        }
        sent
    }

    /// Vide le batch (discard).
    #[inline(always)]
    pub fn clear(&mut self) {
        self.count = 0;
        self.expire_tick.store(0, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchReceiver — réception groupée depuis un ring
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une réception en batch.
pub struct BatchReceiveResult {
    /// Nombre de messages reçus.
    pub received: usize,
    /// Bytes totaux transférés.
    pub bytes: usize,
}

/// Vide autant de messages que possible depuis un ring vers un tableau de buffers.
///
/// # Arguments
/// - `ring`    : ring source.
/// - `bufs`    : tableau de buffers destination; chaque entrée est (ptr, len).
/// - `max`     : nombre maximum de messages à recevoir.
///
/// # Safety
/// Chaque buffer dans `bufs` doit être valide et mutable.
pub fn batch_receive(
    ring: &SpscRing,
    bufs: &mut [([u8; crate::ipc::core::MAX_MSG_SIZE], usize)],
    max: usize,
) -> BatchReceiveResult {
    let mut received = 0;
    let mut bytes = 0;
    let limit = max.min(bufs.len()).min(MAX_BATCH_SIZE);

    for buf in bufs.iter_mut().take(limit) {
        match ring.pop_into(&mut buf.0) {
            Ok((n, _flags)) => {
                buf.1 = n;
                bytes += n;
                received += 1;
            }
            Err(IpcError::QueueEmpty) => break,
            Err(_) => break,
        }
    }
    BatchReceiveResult { received, bytes }
}
