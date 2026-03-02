//! SYS_EXOFS_RELATION_QUERY (513) — requête de relations via syscall (no_std).
//! RÈGLE 9 : copy_from_user/copy_to_user pour tous les pointeurs userspace.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::relation::relation_query::RelationQuery;
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_RELATION_QUERY.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysRelationQueryArgs {
    pub blob_id:       [u8; 32],
    pub direction:     u8,    // 0=outgoing, 1=incoming.
    pub kind_filter:   u8,    // 0=toutes.
    pub _pad:          [u8; 6],
    pub out_ptr:       u64,   // *mut [u8;32] — tableau de blob_ids résultants.
    pub max_results:   u32,
    pub _pad2:         u32,
    pub out_count_ptr: u64,   // *mut u32.
}

const _: () = assert!(core::mem::size_of::<SysRelationQueryArgs>() == 64);

pub fn sys_relation_query(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysRelationQueryArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    let blob = BlobId::from_raw(args.blob_id);

    let query_result = if args.direction == 0 {
        RelationQuery::outgoing(&blob)
    } else {
        RelationQuery::incoming(&blob)
    };

    let qr = match query_result {
        Ok(r)  => r,
        Err(_) => return -5,
    };

    let count = qr.n_total.min(args.max_results as usize);
    for (i, rel) in qr.relations[..count].iter().enumerate() {
        let blob_bytes = rel.to.as_bytes();
        let dst = (args.out_ptr as usize)
            .checked_add(i * 32)
            .unwrap_or(0) as *mut u8;
        if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
            dst, blob_bytes.as_ptr(), 32
        )}.is_err() {
            return -14;
        }
    }

    let n = count as u32;
    if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
        args.out_count_ptr as *mut u32, &n as *const u32, 4
    )}.is_err() {
        return -14;
    }

    0
}
