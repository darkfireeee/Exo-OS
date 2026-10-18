#![allow(unused_imports, dead_code)]
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::object_write::{persist_cached_blob_if_disk, reject_if_immutable};
use crate::fs::exofs::syscall::quota_query::{check_quota, quota_add_usage, quota_sub_usage};

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfs_inode_size() {
        assert_eq!(core::mem::size_of::<VfsInode>(), 32);
    }

    #[test]
    fn test_vfs_dirent_size() {
        assert_eq!(core::mem::size_of::<VfsDirent>(), 271);
    }

    #[test]
    fn test_hash_name_stable() {
        let h1 = hash_name(1, b"foo");
        let h2 = hash_name(1, b"foo");
        let h3 = hash_name(1, b"bar");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_name_nonzero() {
        // Vérifie que le résultat n'est jamais 0.
        let h = hash_name(0, b"\x00");
        assert_ne!(h, 0);
    }

    #[test]
    fn test_validate_name_ok() {
        assert!(validate_name(b"hello").is_ok());
    }

    #[test]
    fn test_validate_name_slash() {
        assert!(matches!(
            validate_name(b"a/b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }

    #[test]
    fn test_validate_name_null() {
        assert!(matches!(
            validate_name(b"a\x00b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }

    #[test]
    fn test_make_dirent() {
        let d = make_dirent(42, b"hello", 8);
        let ino = d.ino;
        let name_len = d.name_len;
        assert_eq!(ino, 42);
        assert_eq!(name_len, 5);
        assert_eq!(&d.name[..5], b"hello");
    }

    #[test]
    fn test_entry_to_vfs_inode_dir() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 1,
            object_id: 1,
            flags: inode_flags::DIRECTORY,
            link_count: 2,
            size: 0,
            uid: 0,
            epoch_id: 0,
            access_ts: 0,
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_dir());
    }

    #[test]
    fn test_entry_to_vfs_inode_file() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 10,
            object_id: 10,
            flags: inode_flags::REGULAR,
            link_count: 1,
            size: 1024,
            uid: 1000,
            epoch_id: 0,
            access_ts: 0,
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_regular());
        assert!(!v.is_dir());
    }

    #[test]
    fn test_inode_kind_values() {
        assert_eq!(inode_kind(inode_flags::DIRECTORY), 4);
        assert_eq!(inode_kind(inode_flags::REGULAR), 8);
        assert_eq!(inode_kind(inode_flags::SYMLINK), 10);
    }

    #[test]
    fn test_fd_table_open_close() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(5, open_flags::O_RDONLY, 1).test_unwrap();
        assert!(fdt.get_fd(fd).is_some());
        fdt.close_fd(fd).test_unwrap();
        assert!(fdt.get_fd(fd).is_none());
    }

    #[test]
    fn test_fd_table_update_offset() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(7, open_flags::O_RDONLY, 1).test_unwrap();
        fdt.update_offset(fd, 512);
        let d = fdt.get_fd(fd).test_unwrap();
        assert_eq!(d.offset, 512);
        fdt.close_fd(fd).test_unwrap();
    }
}
