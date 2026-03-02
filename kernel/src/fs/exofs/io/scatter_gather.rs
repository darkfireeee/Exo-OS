//! Scatter-Gather IO ExoFS.
//!
//! Permet de lire/écrire des buffers non contigus en une seule opération NVMe.
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>
//! RÈGLE 14 : checked_add pour offsets.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

/// Un fragment d'un buffer scatter-gather.
#[derive(Debug, Clone)]
pub struct ScatterGatherBuf {
    /// Adresse virtuelle du fragment (kernel space).
    pub virt_addr: *mut u8,
    /// Longeur en bytes.
    pub len: usize,
}

// SAFETY: ScatterGatherBuf n'est utilisé qu'en Ring 0 sur des buffers alloués
// par le kernel allocator ; aucun thread userspace ne peut les accéder.
unsafe impl Send for ScatterGatherBuf {}
unsafe impl Sync for ScatterGatherBuf {}

/// Opération scatter-gather complète (plusieurs fragments).
pub struct ScatterGatherIo {
    frags: Vec<ScatterGatherBuf>,
    /// Offset de début dans le blob logique.
    pub blob_offset: u64,
}

impl ScatterGatherIo {
    pub fn new(blob_offset: u64) -> Self {
        Self { frags: Vec::new(), blob_offset }
    }

    /// Ajoute un fragment.
    pub fn add_frag(&mut self, frag: ScatterGatherBuf) -> Result<(), FsError> {
        self.frags.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        self.frags.push(frag);
        Ok(())
    }

    /// Calcule la taille totale de l'opération.
    pub fn total_len(&self) -> Result<usize, FsError> {
        let mut total: usize = 0;
        for f in &self.frags {
            total = total.checked_add(f.len).ok_or(FsError::Overflow)?;
        }
        Ok(total)
    }

    /// Itère sur les fragments.
    pub fn frags(&self) -> &[ScatterGatherBuf] {
        &self.frags
    }

    /// Copie tous les fragments dans un buffer linéaire (pour drivers sans SG natif).
    pub fn flatten(&self) -> Result<Vec<u8>, FsError> {
        let total = self.total_len()?;
        let mut out = Vec::new();
        out.try_reserve(total).map_err(|_| FsError::OutOfMemory)?;
        for frag in &self.frags {
            // SAFETY: virt_addr est un pointeur kernel valide de longueur frag.len,
            // alloué par le kernel et non accessible depuis userspace.
            let slice = unsafe { core::slice::from_raw_parts(frag.virt_addr, frag.len) };
            out.extend_from_slice(slice);
        }
        Ok(out)
    }
}
