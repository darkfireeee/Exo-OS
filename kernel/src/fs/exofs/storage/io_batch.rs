//! io_batch.rs (storage) — Regroupement d'I/Os en Bio unique pour le storage ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;

const MAX_BATCH_OPS: usize = 64;

/// Type d'opération.
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum IoBatchKind {
    Read  = 0,
    Write = 1,
}

/// Une opération dans le batch.
#[derive(Clone, Debug)]
pub struct IoBatchOp {
    pub kind:   IoBatchKind,
    pub lba:    u64,
    pub offset: u64,   // Offset dans le buffer.
    pub len:    u32,
}

/// Batch d'I/Os à soumettre en une seule transaction.
pub struct IoBatch {
    ops:       Vec<IoBatchOp>,
    buf:       Vec<u8>,
    submitted: bool,
}

impl IoBatch {
    pub fn new() -> Self {
        Self {
            ops:       Vec::new(),
            buf:       Vec::new(),
            submitted: false,
        }
    }

    /// Ajoute une écriture au batch.
    pub fn add_write(&mut self, lba: u64, data: &[u8]) -> Result<(), FsError> {
        if self.ops.len() >= MAX_BATCH_OPS { return Err(FsError::Busy); }
        let offset = self.buf.len() as u64;
        self.ops.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        self.buf.try_reserve(data.len()).map_err(|_| FsError::OutOfMemory)?;
        self.buf.extend_from_slice(data);
        self.ops.push(IoBatchOp {
            kind:   IoBatchKind::Write,
            lba,
            offset,
            len:    data.len() as u32,
        });
        Ok(())
    }

    /// Soumet le batch (exécute toutes les opérations dans l'ordre).
    pub fn submit(&mut self, device: &mut dyn BatchDevice) -> Result<u32, FsError> {
        if self.submitted { return Err(FsError::Busy); }
        let mut n = 0u32;
        for op in &self.ops {
            let start = op.offset as usize;
            let end   = start.checked_add(op.len as usize).ok_or(FsError::Overflow)?;
            match op.kind {
                IoBatchKind::Write => {
                    device.write_block(op.lba, &self.buf[start..end])?;
                    n += 1;
                }
                IoBatchKind::Read => {}
            }
        }
        self.submitted = true;
        Ok(n)
    }

    pub fn len(&self) -> usize { self.ops.len() }
    pub fn is_empty(&self) -> bool { self.ops.is_empty() }
}

pub trait BatchDevice: Send + Sync {
    fn write_block(&mut self, lba: u64, data: &[u8]) -> Result<(), FsError>;
}
