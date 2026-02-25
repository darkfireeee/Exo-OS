// kernel/src/fs/pseudo/sysfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SYSFS — /sys virtual filesystem (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Expose l'arborescence des périphériques, des drivers et des classes de
// périphériques via un FS virtuel hiérarchique.
//
// Structure /sys :
//   /sys/bus/          → buses (pci, usb, virtio)
//   /sys/class/        → classes de périphériques
//   /sys/devices/      → arbre des périphériques
//   /sys/block/        → périphériques bloc
//   /sys/fs/           → FS virtuels enregistrés
//   /sys/kernel/       → paramètres kernel
//
// Chaque nœud peut être :
//   • ATTR (attribut) : fichier lu/écrit pour une propriété.
//   • DIR  : sous-répertoire.
//   • LINK : lien symbolique vers un autre nœud.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::ToString;
use alloc::format;

use crate::fs::core::types::{FsError, FsResult, FileMode, FileType, InodeNumber, Stat, SeekWhence, Dirent64};
use crate::fs::core::vfs::{FileHandle, FileOps, InodeOps, InodeAttr};
use crate::fs::core::inode::{Inode, InodeRef, new_inode_ref};
use crate::fs::block::device::BLOCK_DEV_REGISTRY;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// SysfsNodeType
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SysfsNodeType {
    Dir,
    Attr { value: Vec<u8>, writable: bool },
    Link { target: Vec<u8> },
}

// ─────────────────────────────────────────────────────────────────────────────
// SysfsNode
// ─────────────────────────────────────────────────────────────────────────────

pub struct SysfsNode {
    pub name:     Vec<u8>,
    pub ino:      InodeNumber,
    pub kind:     SpinLock<SysfsNodeType>,
    pub children: SpinLock<Vec<Arc<SysfsNode>>>,
}

impl SysfsNode {
    pub fn new_dir(name: &[u8], ino: u64) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_vec(),
            ino:  InodeNumber(ino),
            kind: SpinLock::new(SysfsNodeType::Dir),
            children: SpinLock::new(Vec::new()),
        })
    }

    pub fn new_attr(name: &[u8], ino: u64, value: Vec<u8>, writable: bool) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_vec(),
            ino:  InodeNumber(ino),
            kind: SpinLock::new(SysfsNodeType::Attr { value, writable }),
            children: SpinLock::new(Vec::new()),
        })
    }

    pub fn add_child(&self, child: Arc<SysfsNode>) {
        self.children.lock().push(child);
    }

    pub fn find(&self, name: &[u8]) -> Option<Arc<SysfsNode>> {
        self.children.lock().iter()
            .find(|c| c.name.as_slice() == name)
            .map(|c| c.clone())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SysfsTree — arborescence globale
// ─────────────────────────────────────────────────────────────────────────────

pub struct SysfsTree {
    root: Arc<SysfsNode>,
    next_ino: AtomicU64,
}

impl SysfsTree {
    pub fn new() -> Self {
        let root = SysfsNode::new_dir(b"/", 1);
        // Sous-répertoires de base.
        let dirs = [b"bus".as_ref(), b"class", b"devices", b"block", b"fs", b"kernel"];
        let mut ino = 2u64;
        let tree = Self { root: root.clone(), next_ino: AtomicU64::new(ino) };
        for d in &dirs {
            let node = SysfsNode::new_dir(d, ino);
            root.add_child(node);
            ino += 1;
        }
        tree.next_ino.store(ino, Ordering::Relaxed);

        // Génère les block devices connus.
        let n_devs = BLOCK_DEV_REGISTRY.count();
        let block_node = root.find(b"block").unwrap();
        for i in 0..n_devs {
            let name = format!("sda{}", i).into_bytes();
            let dev_node = SysfsNode::new_dir(&name, tree.next_ino.fetch_add(1, Ordering::Relaxed));
            let attr_size = SysfsNode::new_attr(b"size", tree.next_ino.fetch_add(1, Ordering::Relaxed), b"0\n".to_vec(), false);
            dev_node.add_child(attr_size);
            block_node.add_child(dev_node);
        }
        tree
    }

    pub fn lookup_path(&self, path: &[&[u8]]) -> Option<Arc<SysfsNode>> {
        let mut cur = self.root.clone();
        for component in path {
            cur = cur.find(component)?;
        }
        Some(cur)
    }

    pub fn register_attr(&self, path: &[&[u8]], name: &[u8], value: Vec<u8>, writable: bool) {
        if let Some(parent) = self.lookup_path(path) {
            let ino  = self.next_ino.fetch_add(1, Ordering::Relaxed);
            let node = SysfsNode::new_attr(name, ino, value, writable);
            parent.add_child(node);
            SYSFS_STATS.registrations.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SysfsAttrOps — FileOps pour un attribut
// ─────────────────────────────────────────────────────────────────────────────

pub struct SysfsAttrOps {
    pub node: Arc<SysfsNode>,
}

impl FileOps for SysfsAttrOps {
    fn read(&self, _fh: &FileHandle, buf: &mut [u8], offset: u64) -> FsResult<usize> {
        let kind = self.node.kind.lock();
        if let SysfsNodeType::Attr { ref value, .. } = *kind {
            let off = offset as usize;
            if off >= value.len() { return Ok(0); }
            let n = (value.len() - off).min(buf.len());
            buf[..n].copy_from_slice(&value[off..off+n]);
            SYSFS_STATS.reads.fetch_add(1, Ordering::Relaxed);
            return Ok(n);
        }
        Err(FsError::IsDir)
    }

    fn write(&self, _fh: &FileHandle, buf: &[u8], _offset: u64) -> FsResult<usize> {
        let mut kind = self.node.kind.lock();
        if let SysfsNodeType::Attr { ref mut value, writable } = *kind {
            if !writable { return Err(FsError::ReadOnly); }
            *value = buf.to_vec();
            SYSFS_STATS.writes.fetch_add(1, Ordering::Relaxed);
            return Ok(buf.len());
        }
        Err(FsError::IsDir)
    }

    fn seek(&self, _fh: &FileHandle, offset: i64, _: SeekWhence) -> FsResult<u64> { Ok(offset as u64) }
    fn release(&self, _fh: &FileHandle) -> FsResult<()> { Ok(()) }
    fn fsync(&self, _fh: &FileHandle, _: bool) -> FsResult<()> { Ok(()) }
    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> { Err(FsError::NotSupported) }
    fn mmap(&self, _fh: &FileHandle, _offset: u64, _len: usize, _flags: crate::fs::core::vfs::MmapFlags) -> FsResult<u64> { Err(FsError::NotSupported) }
    fn readdir(&self, _fh: &FileHandle, _offset: &mut u64, _emit: &mut dyn FnMut(Dirent64) -> bool) -> FsResult<()> { Err(FsError::NotDir) }
    fn fallocate(&self, _fh: &FileHandle, _mode: u32, _offset: u64, _len: u64) -> FsResult<()> { Err(FsError::NotSupported) }
    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> { Ok(crate::fs::core::vfs::PollEvents(0x0001 | 0x0004)) }
}
// ─────────────────────────────────────────────────────────────────────────────

pub struct SysfsStats {
    pub reads:         AtomicU64,
    pub writes:        AtomicU64,
    pub registrations: AtomicU64,
}

impl SysfsStats {
    pub const fn new() -> Self {
        Self { reads: AtomicU64::new(0), writes: AtomicU64::new(0), registrations: AtomicU64::new(0) }
    }
}

pub static SYSFS_STATS: SysfsStats = SysfsStats::new();
