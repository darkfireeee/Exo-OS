//! SYS_EXOFS_SNAPSHOT_MOUNT (511) — montage d'un snapshot via syscall (no_std).

use crate::fs::exofs::core::FsError;
use crate::fs::exofs::snapshot::snapshot_mount::SNAPSHOT_MOUNT;
use crate::fs::exofs::snapshot::snapshot::SnapshotId;
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_SNAPSHOT_MOUNT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysSnapshotMountArgs {
    pub snap_id:       u64,
    pub out_mount_id:  u64,  // *mut u64 — ID de montage résultant.
    pub flags:         u32,
    pub _pad:          u32,
}

const _: () = assert!(core::mem::size_of::<SysSnapshotMountArgs>() == 24);

pub fn sys_snapshot_mount(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysSnapshotMountArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    let mount_id = match SNAPSHOT_MOUNT.mount(SnapshotId(args.snap_id)) {
        Ok(id)  => id,
        Err(FsError::NotFound)       => return -2,
        Err(FsError::OutOfMemory)    => return -12,
        Err(_)                       => return -5,
    };

    if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
        args.out_mount_id as *mut u64, &mount_id as *const u64, 8
    )}.is_err() {
        // Rollback le montage si on ne peut pas écrire le résultat.
        SNAPSHOT_MOUNT.umount(mount_id);
        return -14;
    }

    0
}

pub fn sys_snapshot_umount(mount_id: u64, _uid: u64) -> i64 {
    if SNAPSHOT_MOUNT.umount(mount_id) { 0 } else { -2 }
}
