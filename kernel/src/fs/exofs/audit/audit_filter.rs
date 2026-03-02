//! AuditFilter — filtrage des entrées d'audit ExoFS (no_std).

use super::audit_entry::{AuditEntry, AuditOp};

/// Critères de filtre.
#[derive(Clone, Debug, Default)]
pub struct FilterCriteria {
    pub actor_uid:  Option<u64>,
    pub op:         Option<AuditOp>,
    pub object_id:  Option<u64>,
    pub since_tick: Option<u64>,
}

pub struct AuditFilter;

impl AuditFilter {
    /// Retourne true si l'entrée correspond aux critères.
    pub fn matches(entry: &AuditEntry, criteria: &FilterCriteria) -> bool {
        if let Some(uid) = criteria.actor_uid {
            if entry.actor_uid != uid { return false; }
        }
        if let Some(op) = criteria.op {
            if entry.op as u8 != op as u8 { return false; }
        }
        if let Some(oid) = criteria.object_id {
            if entry.object_id != oid { return false; }
        }
        if let Some(since) = criteria.since_tick {
            if entry.tick < since { return false; }
        }
        true
    }

    /// Filtre une slice d'entrées et collecte celles qui correspondent.
    pub fn filter<'a>(
        entries: &'a [AuditEntry],
        criteria: &FilterCriteria,
    ) -> alloc::vec::Vec<&'a AuditEntry> {
        entries.iter().filter(|e| Self::matches(e, criteria)).collect()
    }
}
