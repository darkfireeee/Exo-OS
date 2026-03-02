//! AuditReader — lecture du journal d'audit ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::audit_entry::AuditEntry;
use super::audit_log::AUDIT_LOG;

pub struct AuditReader;

impl AuditReader {
    /// Lit les `n` dernières entrées.
    pub fn read_recent(n: usize) -> Result<Vec<AuditEntry>, FsError> {
        let total = AUDIT_LOG.count() as usize;
        let n     = n.min(total).min(65536);
        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| FsError::OutOfMemory)?;
        let head = AUDIT_LOG.count() as usize;
        for i in 0..n {
            let pos = head.wrapping_sub(n).wrapping_add(i);
            out.push(AUDIT_LOG.read_at(pos));
        }
        Ok(out)
    }
}
