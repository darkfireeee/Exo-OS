//! SYS_EXOFS_SNAPSHOT_CREATE (509) — création de snapshot via syscall (no_std).
//! RÈGLE 9 : copy_from_user() pour tous les pointeurs userspace.

use crate::fs::exofs::core::{EpochId, FsError};
use crate::fs::exofs::snapshot::snapshot_create::{SnapshotCreator, SnapshotParams};
use super::validation::{copy_struct_from_user, UserPtr};

/// Arguments userspace pour SYS_EXOFS_SNAPSHOT_CREATE.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysSnapshotCreateArgs {
    pub epoch_id:  u64,
    pub flags:     u32,
    pub name_len:  u16,
    pub _pad:      [u8; 2],
    pub name_ptr:  u64,   // *const u8 userspace.
    pub out_id:    u64,   // *mut u64 userspace (snapshot_id résultant).
}

const _: () = assert!(core::mem::size_of::<SysSnapshotCreateArgs>() == 32);

pub fn sys_snapshot_create(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysSnapshotCreateArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14, // EFAULT
    };

    // Lire le nom depuis l'espace utilisateur.
    let mut name_arr = [0u8; 64];
    let name_len = (args.name_len as usize).min(64);
    if name_len > 0 {
        if unsafe { crate::fs::exofs::syscall::validation::copy_from_user(
            args.name_ptr as *const u8, name_arr.as_mut_ptr(), name_len
        )}.is_err() {
            return -14;
        }
    }

    let params = SnapshotParams {
        name:      name_arr,
        parent_id: None,
        flags:     args.flags,
    };

    // Créer le snapshot avec une liste de blobs vide (remplie par l'appelant).
    let snap_id = match SnapshotCreator::create(EpochId(args.epoch_id), params, &[]) {
        Ok(id)  => id,
        Err(e)  => return fs_error_to_errno(e),
    };

    // Écrire l'ID résultant vers userspace.
    if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
        args.out_id as *mut u64, &snap_id.0 as *const u64, 8
    )}.is_err() {
        return -14;
    }

    0
}

fn fs_error_to_errno(e: FsError) -> i64 {
    match e {
        FsError::OutOfMemory      => -12,
        FsError::NotFound         => -2,
        FsError::InvalidArgument  => -22,
        FsError::Overflow         => -75,
        _                         => -5,
    }
}
