//! Pure mount table, FNV-32 path hashing, and VFS access-control helpers for `vfs_server`.

use crate::compat;
use exo_syscall_abi as syscall;

pub const MAX_MOUNTS: usize = 32;

/// Types de pseudo-FS supportés.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FsType {
    None = 0,
    ExoFs = 1,
    ProcFs = 2,
    SysFs = 3,
    DevFs = 4,
}

impl FsType {
    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            compat::FS_EXOFS => Some(Self::ExoFs),
            compat::FS_PROCFS => Some(Self::ProcFs),
            compat::FS_SYSFS => Some(Self::SysFs),
            compat::FS_DEVFS => Some(Self::DevFs),
            _ => None,
        }
    }
}

/// Entrée de la table de montages.
#[derive(Copy, Clone, Debug)]
pub struct MountEntry {
    pub fs_type: FsType,
    /// Hash FNV-32 du chemin de montage (ex : hash("/proc")).
    pub path_hash: u32,
    /// BlobId du répertoire racine ExoFS (0 pour pseudo-FS).
    pub root_blob: u64,
    pub active: bool,
}

impl MountEntry {
    pub const fn empty() -> Self {
        Self {
            fs_type: FsType::None,
            path_hash: 0,
            root_blob: 0,
            active: false,
        }
    }
}

#[inline]
pub fn fnv32(s: &[u8]) -> u32 {
    let mut h: u32 = 2166136261;
    for &b in s {
        h = h.wrapping_mul(16777619).wrapping_add(b as u32);
    }
    h
}

/// FIX-APP-06: PIDs 2..9 (non-storage Ring 1 servers) are read-only except PID 1 (init) & PID 5 (vfs).
#[inline]
pub const fn check_vfs_write_access(sender_pid: u32) -> bool {
    sender_pid == 1 || sender_pid == 5 || sender_pid >= 10
}

/// Pure stateful mount table usable both in Ring 1 (`vfs_server`) and host unit tests.
pub struct MountTable {
    entries: [MountEntry; MAX_MOUNTS],
    pub count: usize,
}

impl MountTable {
    pub const fn new() -> Self {
        Self {
            entries: [MountEntry::empty(); MAX_MOUNTS],
            count: 0,
        }
    }

    pub fn add_or_replace(&mut self, fs: FsType, path: &[u8], blob: u64) -> Result<usize, i64> {
        let path_hash = fnv32(path);
        let mut free_idx = None;
        for i in 0..MAX_MOUNTS {
            if self.entries[i].active && self.entries[i].path_hash == path_hash {
                self.entries[i] = MountEntry {
                    fs_type: fs,
                    path_hash,
                    root_blob: blob,
                    active: true,
                };
                return Ok(i);
            }
            if free_idx.is_none() && !self.entries[i].active {
                free_idx = Some(i);
            }
        }
        let idx = free_idx.ok_or(syscall::ENOSPC)?;
        self.entries[idx] = MountEntry {
            fs_type: fs,
            path_hash,
            root_blob: blob,
            active: true,
        };
        self.count = self.count.saturating_add(1);
        Ok(idx)
    }

    pub fn find_by_path(&self, path: &[u8]) -> Option<&MountEntry> {
        let h = fnv32(path);
        self.entries
            .iter()
            .find(|e| e.active && e.path_hash == h)
    }

    pub fn remove(&mut self, path: &[u8]) -> Option<usize> {
        let h = fnv32(path);
        for (i, e) in self.entries.iter_mut().enumerate() {
            if e.active && e.path_hash == h {
                *e = MountEntry::empty();
                self.count = self.count.saturating_sub(1);
                return Some(i);
            }
        }
        None
    }

    pub fn active_count(&self) -> usize {
        self.entries.iter().filter(|e| e.active).count()
    }

    pub fn mount_defaults(&mut self) {
        for spec in compat::DEFAULT_PSEUDO_MOUNTS.iter() {
            if let Some(fs_type) = FsType::from_wire(spec.fs_type) {
                let _ = self.add_or_replace(fs_type, spec.path, spec.root_blob);
            }
        }
    }
}

/// Parses a `VFS_MOUNT` IPC wire payload and inserts into `table`.
pub fn parse_and_mount(table: &mut MountTable, payload: &[u8]) -> Result<(usize, u64), i64> {
    if payload.len() < 14 {
        return Err(syscall::EINVAL);
    }
    let fs = FsType::from_wire(payload[0]).ok_or(syscall::EINVAL)?;
    let root_blob = u64::from_le_bytes([
        payload[5],
        payload[6],
        payload[7],
        payload[8],
        payload[9],
        payload[10],
        payload[11],
        payload[12],
    ]);
    let path = &payload[13..];
    let path_len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let idx = table.add_or_replace(fs, &path[..path_len], root_blob)?;
    Ok((idx, root_blob))
}
