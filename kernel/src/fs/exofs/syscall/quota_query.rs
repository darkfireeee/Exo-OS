//! SYS_EXOFS_QUOTA_QUERY (515) — lecture des quotas d'un entité ExoFS.
//! RÈGLE 9 : copy_from_user / copy_to_user.

use crate::fs::exofs::quota::quota_tracker::{QUOTA_TRACKER, QuotaKey};
use super::validation::{write_user_buf, fserr_to_errno, EINVAL};

/// Résultat quota serialisé pour userspace (24 octets, pas d'alignement critique).
#[repr(C)]
struct UserQuotaResult {
    bytes_used:  u64,
    blobs_used:  u64,
    inodes_used: u64,
}

/// `exofs_quota_query(kind:u8, entity_id, out_ptr) -> 0 ou errno`
pub fn sys_exofs_quota_query(
    kind:      u64,
    entity_id: u64,
    out_ptr:   u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if kind > 2 { return EINVAL; }
    let key = QuotaKey { kind: kind as u8, entity_id };
    let usage = QUOTA_TRACKER.get_usage(&key);
    let result = UserQuotaResult {
        bytes_used:  usage.bytes_used,
        blobs_used:  usage.blobs_used,
        inodes_used: usage.inodes_used,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &result as *const UserQuotaResult as *const u8,
            core::mem::size_of::<UserQuotaResult>(),
        )
    };
    write_user_buf(out_ptr, bytes).map(|_| 0).unwrap_or_else(|e| e)
}
