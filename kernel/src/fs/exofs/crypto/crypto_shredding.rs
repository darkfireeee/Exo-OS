//! CryptoShredding — destruction cryptographique sécurisée de blobs ExoFS (no_std).
//!
//! La crypto-shredding consiste à :
//! 1. Remplir le ciphertext avec des données aléatoires (overwrite physique).
//! 2. Révoquer la clé objet associée du KeyStorage.
//! 3. Marquer le blob comme shredded dans les métadonnées.
//! RÈGLE 3 : tout unsafe → // SAFETY: <raison>

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::entropy::ENTROPY_POOL;
use super::key_storage::{KEY_STORAGE, KeyKind, KeySlotId};
use super::crypto_audit::CRYPTO_AUDIT;
use super::crypto_audit::CryptoEvent;

/// Nombre de passes d'overwrite (DoD 5220.22-M simplifié).
const SHRED_PASSES: usize = 3;

/// Résultat d'un shredding.
#[derive(Debug)]
pub struct ShredResult {
    pub blob_id:       BlobId,
    pub bytes_shredded: u64,
    pub passes:        usize,
    pub key_revoked:   bool,
}

/// File d'attente de shredding différé.
pub static CRYPTO_SHREDDER: CryptoShredder = CryptoShredder::new_const();

use alloc::collections::VecDeque;

pub struct CryptoShredder {
    queue:         SpinLock<VecDeque<ShredRequest>>,
    total_shredded: AtomicU64,
    bytes_shredded: AtomicU64,
}

struct ShredRequest {
    blob_id:  BlobId,
    slot_id:  Option<KeySlotId>,  // Slot de la clé objet à révoquer, si connu.
    size:     u64,                // Taille en bytes du ciphertext.
}

impl CryptoShredder {
    pub const fn new_const() -> Self {
        Self {
            queue:          SpinLock::new(VecDeque::new()),
            total_shredded: AtomicU64::new(0),
            bytes_shredded: AtomicU64::new(0),
        }
    }

    /// Enfile un BlobId pour destruction différée.
    pub fn enqueue_shred(
        &self,
        blob_id: BlobId,
        slot_id: Option<KeySlotId>,
        size: u64,
    ) -> Result<(), FsError> {
        let mut q = self.queue.lock();
        q.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        q.push_back(ShredRequest { blob_id, slot_id, size });
        Ok(())
    }

    /// Shred immédiat d'un buffer en mémoire (overwrite multi-passes).
    ///
    /// Utilisé avant la libération d'un buffer contenant un plaintext.
    pub fn shred_buffer(&self, buf: &mut [u8]) {
        // Passe 1 : remplir avec 0x00.
        for b in buf.iter_mut() { *b = 0x00; }
        // Passe 2 : remplir avec 0xFF.
        for b in buf.iter_mut() { *b = 0xFF; }
        // Passe 3 : remplir avec des bytes aléatoires.
        ENTROPY_POOL.fill_bytes(buf);
        // Barrière compilateur pour éviter l'optimisation away.
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    /// Shred d'un buffer avec Vec → consomme le Vec (zeroize puis drop).
    pub fn shred_vec(&self, mut v: Vec<u8>) {
        self.shred_buffer(&mut v);
        drop(v);
    }

    /// Traite la file de shredding différée.
    /// Appeler depuis le GC thread ou un thread dédié.
    pub fn process_queue_batch(
        &self,
        blob_store: &dyn BlobShredder,
    ) -> Result<Vec<ShredResult>, FsError> {
        const BATCH_SIZE: usize = 64;
        let mut batch = Vec::new();
        {
            let mut q = self.queue.lock();
            while batch.len() < BATCH_SIZE {
                match q.pop_front() {
                    Some(r) => {
                        batch.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                        batch.push(r);
                    }
                    None => break,
                }
            }
        }

        let mut results = Vec::new();
        results.try_reserve(batch.len()).map_err(|_| FsError::OutOfMemory)?;

        for req in batch {
            let result = self.shred_one(blob_store, req)?;
            results.push(result);
        }

        Ok(results)
    }

    fn shred_one(
        &self,
        blob_store: &dyn BlobShredder,
        req: ShredRequest,
    ) -> Result<ShredResult, FsError> {
        let mut key_revoked = false;

        // 1. Overwrite physique multi-passes sur le BlobStore.
        for pass in 0..SHRED_PASSES {
            let pattern: u8 = match pass {
                0 => 0x00,
                1 => 0xFF,
                _ => {
                    let mut b = [0u8; 1];
                    ENTROPY_POOL.fill_bytes(&mut b);
                    b[0]
                }
            };
            blob_store.overwrite_blob(&req.blob_id, req.size, pattern)?;
        }

        // 2. Révoquer la clé objet si connue.
        if let Some(slot) = req.slot_id {
            if KEY_STORAGE.revoke_key(slot).is_ok() {
                key_revoked = true;
            }
        }

        // 3. Audit.
        CRYPTO_AUDIT.record(CryptoEvent::KeyRevoked, 1, Some(&req.blob_id), req.size);

        self.total_shredded.fetch_add(1, Ordering::Relaxed);
        self.bytes_shredded.fetch_add(req.size, Ordering::Relaxed);

        Ok(ShredResult {
            blob_id:        req.blob_id,
            bytes_shredded: req.size,
            passes:         SHRED_PASSES,
            key_revoked,
        })
    }

    pub fn total_shredded(&self) -> u64 {
        self.total_shredded.load(Ordering::Relaxed)
    }

    pub fn bytes_shredded(&self) -> u64 {
        self.bytes_shredded.load(Ordering::Relaxed)
    }

    pub fn queue_depth(&self) -> usize {
        self.queue.lock().len()
    }
}

/// Trait d'abstraction pour le BlobStore (overwrite physique).
pub trait BlobShredder {
    fn overwrite_blob(&self, blob_id: &BlobId, size: u64, pattern: u8) -> Result<(), FsError>;
}
