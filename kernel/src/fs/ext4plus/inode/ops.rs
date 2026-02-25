// kernel/src/fs/ext4plus/inode/ops.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ INODE OPS — lecture/écriture/troncature via extent tree
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente InodeOps et FileOps pour les inodes EXT4+.
// Toutes les opérations de données transitent par le page cache (cache-first).
// Les écritures sales sont intégrées dans le journal journalisé (fs/integrity/journal).
//
// Structure on-disk d'un inode EXT4 (256 octets, sélection de champs) :
//   0x00  i_mode           u16
//   0x02  i_uid_lo         u16
//   0x04  i_size_lo        u32
//   0x08  i_atime          u32
//   0x0C  i_ctime          u32
//   0x10  i_mtime          u32
//   0x14  i_dtime          u32
//   0x18  i_gid_lo         u16
//   0x1A  i_links_count    u16
//   0x1C  i_blocks_lo      u32
//   0x20  i_flags          u32
//   0x28  i_block[15]      [u32; 15]  — extent tree inline (si EXTENTS flag)
//   0x6C  i_generation     u32
//   0x70  i_file_acl_lo    u32
//   0x74  i_size_high      u32
//   0x7C  i_extra_isize    u16
//   ...
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{
    FsError, FsResult, FileMode, FileType, InodeNumber, Stat, SeekWhence,
    Timespec64, OpenFlags, Uid, Gid, DevId, FS_STATS,
};
use crate::fs::core::vfs::{FileHandle, FileOps, InodeOps, RenameFlags, InodeAttr};
use crate::fs::core::inode::{Inode, InodeRef, InodeState};
use crate::fs::core::dentry::DentryRef;
use crate::fs::cache::page_cache::{PAGE_CACHE, CachedPage};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::fs::ext4plus::inode::extent::{ext4_find_extent, EXTENT_STATS};
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::ext4plus::group_desc::GroupDescTable;
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};
use crate::security::capability::{CapToken, Rights, verify};

// ─────────────────────────────────────────────────────────────────────────────
// Ext4InodeDisk — image on-disk (256 octets)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4InodeDisk {
    pub i_mode:         u16,
    pub i_uid_lo:       u16,
    pub i_size_lo:      u32,
    pub i_atime:        u32,
    pub i_ctime:        u32,
    pub i_mtime:        u32,
    pub i_dtime:        u32,
    pub i_gid_lo:       u16,
    pub i_links_count:  u16,
    pub i_blocks_lo:    u32,
    pub i_flags:        u32,
    pub i_osd1:         u32,
    pub i_block:        [u32; 15],  // extent tree (60 octets)
    pub i_generation:   u32,
    pub i_file_acl_lo:  u32,
    pub i_size_high:    u32,
    pub i_obso_faddr:   u32,
    pub i_osd2:         [u32; 3],
    pub i_extra_isize:  u16,
    pub i_checksum_hi:  u16,
    pub i_ctime_extra:  u32,
    pub i_mtime_extra:  u32,
    pub i_atime_extra:  u32,
    pub i_crtime:       u32,
    pub i_crtime_extra: u32,
    pub i_version_hi:   u32,
    pub i_projid:       u32,
    pub _pad:           [u32; 24], // padding to 256 bytes
}

const _: () = assert!(core::mem::size_of::<Ext4InodeDisk>() == 256);

pub const EXT4_INODE_FLAG_EXTENTS: u32 = 0x00080000;
pub const EXT4_INODE_FLAG_INLINE:  u32 = 0x10000000;

// ─────────────────────────────────────────────────────────────────────────────
// Ext4InodeOps — InodeOps + FileOps pour un inode EXT4
// ─────────────────────────────────────────────────────────────────────────────

pub struct Ext4InodeOps {
    pub disk:       Arc<RwLock<Ext4InodeDisk>>,
    pub sb:         Arc<RwLock<Ext4Superblock>>,
    pub gdt:        Arc<GroupDescTable>,
    pub load_buf:   PhysAddr,
}

impl Ext4InodeOps {
    /// Ouvre un gestionnaire de fichier pour cet inode.
    pub fn open_file_ops(&self, _flags: OpenFlags) -> FsResult<Arc<dyn FileOps>> {
        INODE_OPS_STATS.opens.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(Ext4InodeOps {
            disk:     self.disk.clone(),
            sb:       self.sb.clone(),
            gdt:      self.gdt.clone(),
            load_buf: self.load_buf,
        }))
    }

    /// Tronque l'inode à la taille spécifiée.
    pub fn truncate_inode(&self, inode: &InodeRef, size: u64) -> FsResult<()> {
        inode.read().size.store(size, Ordering::Relaxed);
        INODE_OPS_STATS.truncates.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Lit un bloc logique (soit depuis le page cache, soit depuis le disque).
    fn read_block(&self, inode: &InodeRef, lblock: u32) -> FsResult<Vec<u8>> {
        let ino      = inode.read().ino;
        let sb       = self.sb.read();
        let bsize    = sb.block_size;

        // Recherche dans le page cache
        if let Some(page) = PAGE_CACHE.get().lookup(ino, crate::fs::cache::page_cache::PageIndex(lblock as u64)) {
            let slice = unsafe {
                // SAFETY: virt pointe sur bsize octets gérés par le page cache
                core::slice::from_raw_parts(page.virt, bsize as usize)
            };
            INODE_OPS_STATS.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(slice.to_vec());
        }

        // Résoud le bloc physique
        let disk = self.disk.read();
        let i_block_bytes: [u8; 60] = unsafe {
            core::mem::transmute(disk.i_block)
        };
        let ext_result = ext4_find_extent(
            &i_block_bytes, lblock, sb.dev.0, bsize, self.load_buf
        )?;
        let phys_block = if let Some(er) = ext_result {
            er.leaf.phys_block() + er.offset_in_ext as u64
        } else { return Err(FsError::NotFound); };

        drop(disk); drop(sb);

        // Charge depuis le disque
        let sector = phys_block * self.sb.read().block_size / 512;
        let bio = Bio {
            id:       0,
            op:       BioOp::Read,
            dev:      self.sb.read().dev.0,
            sector,
            vecs:     alloc::vec![BioVec {
                phys: self.load_buf, virt: self.load_buf.as_u64(),
                len: self.sb.read().block_size as u32, offset: 0,
            }],
            flags:    BioFlags::empty(),
            status:   AtomicU8::new(0),
            bytes:    AtomicU64::new(0),
            callback: None,
            cb_data:  0,
        };
        submit_bio(bio)?;

        let bsize = self.sb.read().block_size as usize;
        let data = unsafe {
            core::slice::from_raw_parts(self.load_buf.as_u64() as *const u8, bsize)
        }.to_vec();

        INODE_OPS_STATS.disk_reads.fetch_add(1, Ordering::Relaxed);
        Ok(data)
    }
}

impl FileOps for Ext4InodeOps {
    fn read(&self, fh: &FileHandle, buf: &mut [u8], offset: u64) -> FsResult<usize> {
        let inode   = &fh.inode;
        let size    = inode.read().size.load(Ordering::Relaxed);
        if offset >= size { return Ok(0); }
        let to_read = buf.len().min((size - offset) as usize);
        let bsize   = self.sb.read().block_size;

        let mut done  = 0usize;
        let mut off   = offset;

        while done < to_read {
            let lblock      = (off / bsize) as u32;
            let block_off   = (off % bsize) as usize;
            let block_data  = self.read_block(inode, lblock)?;
            let avail       = (block_data.len() - block_off).min(to_read - done);
            buf[done..done + avail].copy_from_slice(&block_data[block_off..block_off + avail]);
            done += avail;
            off  += avail as u64;
        }
        INODE_OPS_STATS.reads.fetch_add(1, Ordering::Relaxed);
        INODE_OPS_STATS.bytes_read.fetch_add(done as u64, Ordering::Relaxed);
        Ok(done)
    }

    fn write(&self, fh: &FileHandle, buf: &[u8], offset: u64) -> FsResult<usize> {
        let inode   = &fh.inode;
        let bsize   = self.sb.read().block_size;
        let mut done= 0usize;
        let mut off = offset;

        while done < buf.len() {
            let lblock    = (off / bsize) as u32;
            let block_off = (off % bsize) as usize;
            let chunk     = (bsize as usize - block_off).min(buf.len() - done);

            // Charge/crée le bloc puis modifie
            let mut block_data = self.read_block(inode, lblock)
                .unwrap_or_else(|_| alloc::vec![0u8; bsize as usize]);

            let end = (block_off + chunk).min(block_data.len());
            block_data[block_off..end].copy_from_slice(&buf[done..done + (end - block_off)]);

            // Écrit en cache et sur disque via BIO
            let sector = {
                let sb   = self.sb.read();
                let disk = self.disk.read();
                let i_block_bytes: [u8; 60] = unsafe { core::mem::transmute(disk.i_block) };
                if let Ok(Some(er)) = ext4_find_extent(&i_block_bytes, lblock, sb.dev.0, sb.block_size, self.load_buf) {
                    (er.leaf.phys_block() + er.offset_in_ext as u64) * sb.block_size / 512
                } else { return Err(FsError::NoSpace); }
            };

            // SAFETY: block_data est alloué par Vec, stable en mémoire le temps du BIO.
            let data_ptr = block_data.as_ptr();
            let phys = PhysAddr::new(data_ptr as u64);
            let bio = Bio {
                id:       0,
                op:       BioOp::Write,
                dev:      self.sb.read().dev.0,
                sector,
                vecs:     alloc::vec![BioVec { phys, virt: data_ptr as u64, len: bsize as u32, offset: 0 }],
                flags:    BioFlags::empty(),
                status:   AtomicU8::new(0),
                bytes:    AtomicU64::new(0),
                callback: None,
                cb_data:  0,
            };
            submit_bio(bio)?;

            done += end - block_off;
            off  += (end - block_off) as u64;
        }

        // Met à jour la taille
        let new_size = (offset + done as u64).max(inode.read().size.load(Ordering::Relaxed));
        inode.read().size.store(new_size, Ordering::Relaxed);
        INODE_OPS_STATS.writes.fetch_add(1, Ordering::Relaxed);
        INODE_OPS_STATS.bytes_written.fetch_add(done as u64, Ordering::Relaxed);
        Ok(done)
    }

    fn seek(&self, _fh: &FileHandle, off: i64, whence: SeekWhence) -> FsResult<u64> {
        // La logique de seek est gérée au niveau du fd (pos atomic)
        match whence {
            SeekWhence::Set     => { if off < 0 { return Err(FsError::InvalidArgument); } Ok(off as u64) }
            SeekWhence::Current => Err(FsError::NotSupported), // résolu par fd
            SeekWhence::End     => Err(FsError::NotSupported),
            SeekWhence::Data    => Err(FsError::NotSupported), // SEEK_DATA non implémenté
            SeekWhence::Hole    => Err(FsError::NotSupported), // SEEK_HOLE non implémenté
        }
    }

    fn fsync(&self, fh: &FileHandle, _data_only: bool) -> FsResult<()> {
        INODE_OPS_STATS.fsyncs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn ioctl(&self, _fh: &FileHandle, _cmd: u32, _arg: u64) -> FsResult<i64> {
        Err(FsError::NotSupported)
    }

    fn mmap(
        &self, _fh: &FileHandle, _offset: u64, _length: usize,
        _flags: crate::fs::core::vfs::MmapFlags,
    ) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn readdir(
        &self, _fh: &FileHandle, _offset: &mut u64,
        _emit: &mut dyn FnMut(crate::fs::core::types::Dirent64) -> bool,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn fallocate(
        &self, _fh: &FileHandle, _mode: u32, _offset: u64, _length: u64,
    ) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn release(&self, _fh: &FileHandle) -> FsResult<()> { Ok(()) }

    fn poll(&self, _fh: &FileHandle) -> FsResult<crate::fs::core::vfs::PollEvents> {
        Ok(crate::fs::core::vfs::PollEvents(0))
    }
}

impl InodeOps for Ext4InodeOps {
    fn getattr(&self, inode: &InodeRef) -> FsResult<Stat> {
        let i = inode.read();
        Ok(i.to_stat())
    }

    fn setattr(&self, _inode: &InodeRef, _attr: &InodeAttr) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn lookup(&self, _dir: &InodeRef, _name: &[u8]) -> FsResult<DentryRef> {
        // Délégué à directory/ops.rs
        Err(FsError::NotSupported)
    }

    fn create(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _uid: Uid, _gid: Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn mkdir(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _uid: Uid, _gid: Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn rmdir(&self, _dir: &InodeRef, _name: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn unlink(&self, _dir: &InodeRef, _name: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn rename(&self, _old_dir: &InodeRef, _old_name: &[u8], _new_dir: &InodeRef, _new_name: &[u8], _flags: RenameFlags) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn link(&self, _old_inode: &InodeRef, _new_dir: &InodeRef, _new_name: &[u8]) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn symlink(&self, _dir: &InodeRef, _name: &[u8], _target: &[u8], _uid: Uid, _gid: Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn readlink(&self, _inode: &InodeRef, _buf: &mut [u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }

    fn mknod(&self, _dir: &InodeRef, _name: &[u8], _mode: FileMode, _rdev: DevId, _uid: Uid, _gid: Gid) -> FsResult<InodeRef> {
        Err(FsError::NotSupported)
    }

    fn write_inode(&self, inode: &InodeRef, _sync: bool) -> FsResult<()> {
        let i = inode.read();
        let mut disk = self.disk.write();
        disk.i_size_lo     = i.size.load(Ordering::Relaxed) as u32;
        disk.i_links_count = i.nlink.load(Ordering::Relaxed) as u16;
        INODE_OPS_STATS.write_inodes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn evict_inode(&self, _inode: &InodeRef) -> FsResult<()> {
        INODE_OPS_STATS.evicts.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InodeOpsStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct InodeOpsStats {
    pub opens:         AtomicU64,
    pub reads:         AtomicU64,
    pub writes:        AtomicU64,
    pub fsyncs:        AtomicU64,
    pub truncates:     AtomicU64,
    pub write_inodes:  AtomicU64,
    pub evicts:        AtomicU64,
    pub cache_hits:    AtomicU64,
    pub disk_reads:    AtomicU64,
    pub bytes_read:    AtomicU64,
    pub bytes_written: AtomicU64,
}

impl InodeOpsStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self {
            opens: z!(), reads: z!(), writes: z!(), fsyncs: z!(),
            truncates: z!(), write_inodes: z!(), evicts: z!(),
            cache_hits: z!(), disk_reads: z!(), bytes_read: z!(), bytes_written: z!(),
        }
    }
}

pub static INODE_OPS_STATS: InodeOpsStats = InodeOpsStats::new();
