//! AuditWriter — écriture lock-free dans le journal d'audit ExoFS (no_std).

use crate::arch::time::read_ticks;
use super::audit_entry::{AuditEntry, AuditOp, AuditResult};
use super::audit_log::AUDIT_LOG;

pub struct AuditWriter;

impl AuditWriter {
    pub fn record(
        actor_uid: u64,
        actor_cap: u64,
        object_id: u64,
        op:        AuditOp,
        result:    AuditResult,
    ) {
        AUDIT_LOG.push(AuditEntry {
            tick: read_ticks(), actor_uid, actor_cap, object_id,
            blob_id: [0; 32], op, result, _pad: [0; 6],
        });
    }

    pub fn record_blob(
        actor_uid: u64,
        actor_cap: u64,
        object_id: u64,
        blob_id:   [u8; 32],
        op:        AuditOp,
        result:    AuditResult,
    ) {
        AUDIT_LOG.push(AuditEntry {
            tick: read_ticks(), actor_uid, actor_cap, object_id,
            blob_id, op, result, _pad: [0; 6],
        });
    }
}
