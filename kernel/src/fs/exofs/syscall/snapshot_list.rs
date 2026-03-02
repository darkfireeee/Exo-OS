//! SYS_EXOFS_SNAPSHOT_LIST (510) — liste des snapshots via syscall (no_std).
//! RÈGLE 9 : copy_to_user() pour écriture vers userspace.

use crate::fs::exofs::core::FsError;
use crate::fs::exofs::snapshot::snapshot_list::SNAPSHOT_LIST;
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_SNAPSHOT_LIST.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysSnapshotListArgs {
    pub out_ids_ptr:   u64,  // *mut u64 — tableau d'IDs snapshots.
    pub max_count:     u32,
    pub _pad:          u32,
    pub out_count_ptr: u64,  // *mut u32 — nombre effectivement écrit.
}

const _: () = assert!(core::mem::size_of::<SysSnapshotListArgs>() == 24);

pub fn sys_snapshot_list(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysSnapshotListArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    let all_ids = SNAPSHOT_LIST.all_ids();
    let count = (all_ids.len()).min(args.max_count as usize);

    for (i, snap_id) in all_ids[..count].iter().enumerate() {
        let dst = (args.out_ids_ptr as usize)
            .checked_add(i * 8)
            .unwrap_or(0) as *mut u64;
        if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
            dst, &snap_id.0 as *const u64, 8
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
