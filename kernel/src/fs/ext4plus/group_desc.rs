// kernel/src/fs/ext4plus/group_desc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ GROUP DESCRIPTOR — descripteurs de groupes de blocs (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Chaque groupe de blocs possède un Ext4GroupDesc décrivant :
//   • L'adresse des bitmaps (inodes et blocs).
//   • La table des inodes.
//   • Les compteurs de blocs / inodes libres.
//   • Les checksums de métadonnées (feature metadata_csum).
//
// La table des descripteurs est stockée immédiatement après le superbloc
// (bloc 1 si block_size >= 2048, sinon bloc 2).
//
// En mode 64-bit (FEAT_INCOMPAT_64BIT), les descripteurs font 64 octets ;
// en mode 32-bit ils font 32 octets.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::fs::core::types::{FsError, FsResult, DevId};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::integrity::checksum::{crc32c, ChecksumKind};
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};

// ─────────────────────────────────────────────────────────────────────────────
// Ext4GroupDescDisk32 — format on-disk 32-bit (32 octets)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4GroupDescDisk32 {
    pub bg_block_bitmap_lo:      u32,  // 0x00
    pub bg_inode_bitmap_lo:      u32,  // 0x04
    pub bg_inode_table_lo:       u32,  // 0x08
    pub bg_free_blocks_count_lo: u16,  // 0x0C
    pub bg_free_inodes_count_lo: u16,  // 0x0E
    pub bg_used_dirs_count_lo:   u16,  // 0x10
    pub bg_flags:                u16,  // 0x12
    pub bg_exclude_bitmap_lo:    u32,  // 0x14
    pub bg_block_bitmap_csum_lo: u16,  // 0x18
    pub bg_inode_bitmap_csum_lo: u16,  // 0x1A
    pub bg_itable_unused_lo:     u16,  // 0x1C
    pub bg_checksum:             u16,  // 0x1E
}

// ─────────────────────────────────────────────────────────────────────────────
// Ext4GroupDescDisk64 — format on-disk 64-bit (64 octets)
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4GroupDescDisk64 {
    // --- Champs 32-bit (communs) ---
    pub bg_block_bitmap_lo:      u32,
    pub bg_inode_bitmap_lo:      u32,
    pub bg_inode_table_lo:       u32,
    pub bg_free_blocks_count_lo: u16,
    pub bg_free_inodes_count_lo: u16,
    pub bg_used_dirs_count_lo:   u16,
    pub bg_flags:                u16,
    pub bg_exclude_bitmap_lo:    u32,
    pub bg_block_bitmap_csum_lo: u16,
    pub bg_inode_bitmap_csum_lo: u16,
    pub bg_itable_unused_lo:     u16,
    pub bg_checksum:             u16,
    // --- Extension 64-bit ---
    pub bg_block_bitmap_hi:      u32,
    pub bg_inode_bitmap_hi:      u32,
    pub bg_inode_table_hi:       u32,
    pub bg_free_blocks_count_hi: u16,
    pub bg_free_inodes_count_hi: u16,
    pub bg_used_dirs_count_hi:   u16,
    pub bg_itable_unused_hi:     u16,
    pub bg_exclude_bitmap_hi:    u32,
    pub bg_block_bitmap_csum_hi: u16,
    pub bg_inode_bitmap_csum_hi: u16,
    pub bg_reserved:             u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Ext4GroupDesc — représentation en mémoire (normalisée)
// ─────────────────────────────────────────────────────────────────────────────

/// Version en mémoire d'un descripteur de groupe.
/// Combine les parties lo+hi pour toujours avoir des valeurs 64-bit.
#[derive(Clone, Default)]
pub struct Ext4GroupDesc {
    pub group_idx:              u64,
    pub block_bitmap:           u64,
    pub inode_bitmap:           u64,
    pub inode_table:            u64,
    pub free_blocks_count:      u32,
    pub free_inodes_count:      u32,
    pub used_dirs_count:        u32,
    pub flags:                  u16,
    pub checksum:               u16,
    pub itable_unused:          u32,
    pub dirty:                  bool,
}

impl Ext4GroupDesc {
    /// Construit depuis un descripteur 32-bit.
    pub fn from_disk32(idx: u64, d: &Ext4GroupDescDisk32) -> Self {
        Self {
            group_idx:         idx,
            block_bitmap:      d.bg_block_bitmap_lo as u64,
            inode_bitmap:      d.bg_inode_bitmap_lo as u64,
            inode_table:       d.bg_inode_table_lo  as u64,
            free_blocks_count: d.bg_free_blocks_count_lo as u32,
            free_inodes_count: d.bg_free_inodes_count_lo as u32,
            used_dirs_count:   d.bg_used_dirs_count_lo   as u32,
            flags:             d.bg_flags,
            checksum:          d.bg_checksum,
            itable_unused:     d.bg_itable_unused_lo as u32,
            dirty:             false,
        }
    }

    /// Construit depuis un descripteur 64-bit.
    pub fn from_disk64(idx: u64, d: &Ext4GroupDescDisk64) -> Self {
        let hi = |lo: u32, hi: u32| -> u64 { lo as u64 | ((hi as u64) << 32) };
        Self {
            group_idx:         idx,
            block_bitmap:      hi(d.bg_block_bitmap_lo, d.bg_block_bitmap_hi),
            inode_bitmap:      hi(d.bg_inode_bitmap_lo, d.bg_inode_bitmap_hi),
            inode_table:       hi(d.bg_inode_table_lo,  d.bg_inode_table_hi),
            free_blocks_count: d.bg_free_blocks_count_lo as u32
                             + d.bg_free_blocks_count_hi as u32,
            free_inodes_count: d.bg_free_inodes_count_lo as u32
                             + d.bg_free_inodes_count_hi as u32,
            used_dirs_count:   d.bg_used_dirs_count_lo as u32
                             + d.bg_used_dirs_count_hi as u32,
            flags:             d.bg_flags,
            checksum:          d.bg_checksum,
            itable_unused:     d.bg_itable_unused_lo as u32
                             + d.bg_itable_unused_hi as u32,
            dirty:             false,
        }
    }

    /// Calcule le checksum CRC16 d'un descripteur.
    pub fn compute_csum(&self, sb_uuid: &[u8; 16]) -> u16 {
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&(self.group_idx as u32).to_le_bytes());
        let crc = crc32c(sb_uuid);
        (crc ^ (self.block_bitmap as u32)) as u16
    }

    /// Vérifie le checksum (si feature metadata_csum activée).
    pub fn verify_csum(&self, sb_uuid: &[u8; 16]) -> bool {
        self.checksum == self.compute_csum(sb_uuid)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GroupDescTable — table chargée en mémoire
// ─────────────────────────────────────────────────────────────────────────────

pub struct GroupDescTable {
    pub groups: SpinLock<Vec<Ext4GroupDesc>>,
    dev:        DevId,
    block_size: u64,
    desc_size:  u16,
    is_64bit:   bool,
}

impl GroupDescTable {
    /// Charge toute la table depuis le disque.
    pub fn load(sb: &Ext4Superblock, phys_buf: PhysAddr) -> FsResult<Self> {
        let group_count   = sb.group_count as usize;
        let block_size    = sb.block_size;
        let first_gdt_blk = if block_size == 1024 { 2u64 } else { 1u64 };
        let desc_size     = sb.desc_size;
        let is_64bit      = sb.has_64bit;
        let dev           = sb.dev;

        // Calcule le nombre de blocs occupés par la table
        let total_bytes    = group_count as u64 * desc_size as u64;
        let total_blocks   = (total_bytes + block_size - 1) / block_size;

        let mut groups = Vec::with_capacity(group_count);

        for blk_idx in 0..total_blocks {
            let sector = (first_gdt_blk + blk_idx) * block_size / 512;
            let bio = Bio {
                id:       0,
                op:       BioOp::Read,
                dev:      dev.0,
                sector,
                vecs:     alloc::vec![BioVec {
                    phys: phys_buf,
                    virt: phys_buf.as_u64(),
                    len:  block_size as u32,
                    offset: 0,
                }],
                flags:    BioFlags::META,
                status:   core::sync::atomic::AtomicU8::new(0),
                bytes:    core::sync::atomic::AtomicU64::new(0),
                callback: None,
                cb_data:  0,
            };
            submit_bio(bio)?;

            let entries_per_block = (block_size / desc_size as u64) as usize;
            let base_idx          = (blk_idx * entries_per_block as u64) as usize;

            for i in 0..entries_per_block {
                let idx = base_idx + i;
                if idx >= group_count { break; }
                let offset = (i * desc_size as usize) as u64;
                let ptr    = (phys_buf.as_u64() + offset) as usize;

                let gd = if is_64bit {
                    // SAFETY: buffer lu depuis disque, aligné u8, taille vérifiée
                    let d = unsafe { &*(ptr as *const Ext4GroupDescDisk64) };
                    Ext4GroupDesc::from_disk64(idx as u64, d)
                } else {
                    // SAFETY: idem
                    let d = unsafe { &*(ptr as *const Ext4GroupDescDisk32) };
                    Ext4GroupDesc::from_disk32(idx as u64, d)
                };
                groups.push(gd);
            }
        }

        GDT_STATS.loads.fetch_add(1, Ordering::Relaxed);
        Ok(Self { groups: SpinLock::new(groups), dev, block_size, desc_size, is_64bit })
    }

    /// Accède à un descripteur donné.
    pub fn get(&self, idx: u64) -> FsResult<Ext4GroupDesc> {
        let guard = self.groups.lock();
        guard.get(idx as usize).cloned().ok_or(FsError::InvalidArgument)
    }

    /// Met à jour un descripteur et le marque dirty.
    pub fn update(&self, gd: Ext4GroupDesc) {
        let mut guard = self.groups.lock();
        let idx = gd.group_idx as usize;
        if idx < guard.len() { guard[idx] = gd; guard[idx].dirty = true; }
        GDT_STATS.updates.fetch_add(1, Ordering::Relaxed);
    }

    /// Itère sur tous les groupes (lecture seule).
    pub fn iter_all(&self) -> alloc::vec::Vec<Ext4GroupDesc> {
        self.groups.lock().clone()
    }

    /// Localise un groupe ayant des inodes libres.
    pub fn find_group_with_free_inodes(&self) -> Option<u64> {
        let guard = self.groups.lock();
        guard.iter().find(|g| g.free_inodes_count > 0).map(|g| g.group_idx)
    }

    /// Localise un groupe ayant des blocs libres.
    pub fn find_group_with_free_blocks(&self, needed: u32) -> Option<u64> {
        let guard = self.groups.lock();
        guard.iter().find(|g| g.free_blocks_count >= needed).map(|g| g.group_idx)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GdtStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct GdtStats {
    pub loads:   AtomicU64,
    pub updates: AtomicU64,
    pub errors:  AtomicU64,
}

impl GdtStats {
    pub const fn new() -> Self {
        Self { loads: AtomicU64::new(0), updates: AtomicU64::new(0), errors: AtomicU64::new(0) }
    }
}

pub static GDT_STATS: GdtStats = GdtStats::new();
