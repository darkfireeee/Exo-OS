extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::rights::{RIGHT_CREATE, RIGHT_LIST, RIGHT_READ, RIGHT_WRITE};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::fs::exofs::storage::virtio_adapter;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::fs::exofs::syscall::object_open::{sys_exofs_object_open, OpenArgs};
use crate::fs::exofs::syscall::object_read::sys_exofs_object_read;
use crate::fs::exofs::syscall::object_store::OBJECT_STORE;
use crate::fs::exofs::syscall::object_write::sys_exofs_object_write;
use crate::fs::exofs::syscall::open_by_path::sys_exofs_open_by_path;
use crate::fs::exofs::syscall::readdir::sys_exofs_readdir;
use crate::fs::exofs::syscall::validation::EXOFS_PATH_MAX;

pub(crate) struct MockBlockDevice {
    storage: Mutex<Vec<u8>>,
    block_size: u32,
}

impl MockBlockDevice {
    pub(crate) fn new(block_size: u32, blocks: usize) -> Self {
        Self {
            storage: Mutex::new(alloc::vec![0u8; block_size as usize * blocks]),
            block_size,
        }
    }
}

impl BlockDevice for MockBlockDevice {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        let block_size = self.block_size as usize;
        if buf.len() != block_size {
            return Err(ExofsError::InvalidArgument);
        }
        let start = lba as usize * block_size;
        let end = start.saturating_add(block_size);
        let storage = self.storage.lock();
        if end > storage.len() {
            return Err(ExofsError::IoError);
        }
        buf.copy_from_slice(&storage[start..end]);
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let block_size = self.block_size as usize;
        if buf.len() != block_size {
            return Err(ExofsError::InvalidArgument);
        }
        let start = lba as usize * block_size;
        let end = start.saturating_add(block_size);
        let mut storage = self.storage.lock();
        if end > storage.len() {
            return Err(ExofsError::IoError);
        }
        storage[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn total_blocks(&self) -> u64 {
        self.storage.lock().len() as u64 / self.block_size as u64
    }

    fn flush(&self) -> ExofsResult<()> {
        Ok(())
    }
}

pub(crate) fn reset_state() {
    // flush_all_force : dans les tests, on veut vider le cache sans erreur
    // même si des entrées dirty existent (pas de disque réel à synchroniser).
    BLOB_CACHE.flush_all_force();
    OBJECT_TABLE.reset_all();
    OBJECT_STORE.reset_all();
    virtio_adapter::clear_global_disk_for_test();
}

pub(crate) fn install_mock_disk(block_size: u32, blocks: usize) -> Arc<MockBlockDevice> {
    let device = Arc::new(MockBlockDevice::new(block_size, blocks));
    virtio_adapter::set_global_disk_for_test(device.clone());
    device
}

fn path_bytes(path: &str) -> Vec<u8> {
    let mut out = alloc::vec![0u8; EXOFS_PATH_MAX];
    let raw = path.as_bytes();
    let copy_len = raw.len().min(EXOFS_PATH_MAX.saturating_sub(1));
    out[..copy_len].copy_from_slice(&raw[..copy_len]);
    out
}

pub(crate) fn open_path(path: &str, flags: u32) -> u32 {
    let bytes = path_bytes(path);
    let mut fd = 0u32;
    let args = OpenArgs {
        flags,
        mode: 0o644,
        epoch_id: 0,
        owner_uid: 0,
        size_hint: 0,
        _reserved: [0u64; 2],
    };
    let rc = sys_exofs_object_open(
        bytes.as_ptr() as u64,
        bytes.len() as u64,
        flags as u64,
        (&mut fd as *mut u32) as u64,
        (&args as *const OpenArgs) as u64,
        (RIGHT_READ | RIGHT_WRITE) as u64,
    );
    assert!(rc >= 0, "open failed: {rc}");
    fd
}

pub(crate) fn open_path_atomic(path: &str, flags: u32) -> u32 {
    let bytes = path_bytes(path);
    let rc = sys_exofs_open_by_path(
        bytes.as_ptr() as u64,
        flags as u64,
        0o644,
        0,
        0,
        (RIGHT_READ | RIGHT_WRITE) as u64,
    );
    assert!(rc >= 0, "open_by_path failed: {rc}");
    rc as u32
}

pub(crate) fn write_at(fd: u32, data: &[u8], offset: u64) -> usize {
    let rc = sys_exofs_object_write(
        fd as u64,
        data.as_ptr() as u64,
        data.len() as u64,
        offset,
        0,
        RIGHT_WRITE as u64,
    );
    assert!(rc >= 0, "write failed: {rc}");
    rc as usize
}

pub(crate) fn read_at(fd: u32, count: usize, offset: u64) -> Vec<u8> {
    let mut out = alloc::vec![0u8; count];
    let rc = sys_exofs_object_read(
        fd as u64,
        out.as_mut_ptr() as u64,
        count as u64,
        offset,
        0,
        RIGHT_READ as u64,
    );
    assert!(rc >= 0, "read failed: {rc}");
    out.truncate(rc as usize);
    out
}

pub(crate) fn close_fd(fd: u32) {
    assert!(OBJECT_TABLE.close(fd), "close failed for fd {fd}");
}

use crate::fs::exofs::syscall::object_create::{sys_exofs_object_create, CreateArgs};

/// Ouvre ou crée un objet RÉPERTOIRE (kind=1) et retourne son fd.
/// Nécessaire pour readdir — open_rdwr crée un fichier ordinaire (kind=0).
/// sys_exofs_object_create exige RIGHT_CREATE dans cap_rights (verify_cap).
pub(crate) fn open_dir(path: &str) -> u32 {
    let bytes = path_bytes(path);
    let args = CreateArgs {
        flags: open_flags::O_RDWR | open_flags::O_CREAT,
        mode: 0o755,
        kind: 1, // ObjectKind::Directory
        _pad: [0u8; 7],
        epoch_id: 0,
        owner_uid: 0,
        initial_size: 0,
    };
    let rc = sys_exofs_object_create(
        bytes.as_ptr() as u64,
        bytes.len() as u64,
        (open_flags::O_RDWR | open_flags::O_CREAT) as u64,
        0, // out_ptr non requis — fd retourné directement
        (&args as *const CreateArgs) as u64,
        (RIGHT_READ | RIGHT_WRITE | RIGHT_CREATE | RIGHT_LIST) as u64,
    );
    assert!(
        rc >= 0,
        "open_dir failed for '{path}': rc={rc} (manque RIGHT_CREATE ?)"
    );
    rc as u32
}
pub(crate) fn open_rdwr(path: &str) -> u32 {
    open_path(path, open_flags::O_RDWR)
}

/// Parse un buffer readdir en liste (nom, d_type).
pub(crate) fn parse_dirents(buf: &[u8]) -> Vec<(alloc::string::String, u8)> {
    use crate::fs::exofs::syscall::readdir::HEADER_SIZE;
    let mut out = alloc::vec::Vec::new();
    let mut off = 0usize;
    while off.saturating_add(HEADER_SIZE) <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off.saturating_add(reclen) > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + HEADER_SIZE;
        let name_end = buf[name_start..off + reclen]
            .iter()
            .position(|&b| b == 0)
            .map(|p| name_start + p)
            .unwrap_or(off + reclen);
        out.push((
            alloc::string::String::from_utf8_lossy(&buf[name_start..name_end]).into_owned(),
            dtype,
        ));
        off = off.saturating_add(reclen);
    }
    out
}

pub(crate) fn readdir_fd(fd: u32, buf_len: usize) -> Vec<u8> {
    let mut out = alloc::vec![0u8; buf_len];
    let rc = sys_exofs_readdir(
        fd as u64,
        out.as_mut_ptr() as u64,
        buf_len as u64,
        0,
        0,
        RIGHT_LIST as u64,
    );
    assert!(rc >= 0, "readdir failed: {rc}");
    out.truncate(rc as usize);
    out
}
