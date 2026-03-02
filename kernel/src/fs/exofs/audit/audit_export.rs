//! AuditExporter — export du journal d'audit ExoFS (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::audit_entry::AuditEntry;
use super::audit_reader::AuditReader;
use super::audit_filter::{AuditFilter, FilterCriteria};

/// Récepteur de données exportées.
pub trait ExportSink {
    fn write(&mut self, data: &[u8]) -> Result<(), FsError>;
}

pub struct AuditExporter;

impl AuditExporter {
    /// Exporte toutes les entrées récentes dans le sink, format sérialisé brut.
    pub fn export_all(n: usize, sink: &mut dyn ExportSink) -> Result<u64, FsError> {
        let entries = AuditReader::read_recent(n)?;
        Self::export_entries(&entries, sink)
    }

    /// Exporte les entrées correspondant aux critères.
    pub fn export_filtered(
        n: usize,
        criteria: &FilterCriteria,
        sink: &mut dyn ExportSink,
    ) -> Result<u64, FsError> {
        let entries = AuditReader::read_recent(n)?;
        let matched: Vec<AuditEntry> = entries
            .into_iter()
            .filter(|e| AuditFilter::matches(e, criteria))
            .collect();
        Self::export_entries(&matched, sink)
    }

    fn export_entries(entries: &[AuditEntry], sink: &mut dyn ExportSink) -> Result<u64, FsError> {
        let mut n = 0u64;
        for entry in entries {
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    entry as *const AuditEntry as *const u8,
                    core::mem::size_of::<AuditEntry>(),
                )
            };
            sink.write(bytes)?;
            n += 1;
        }
        Ok(n)
    }
}
