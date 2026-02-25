// kernel/src/fs/ext4plus/allocation/balloc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ BALLOC — allocateur de blocs simple par bitmap de groupe
// ═══════════════════════════════════════════════════════════════════════════════
//
// Alloue et libère des blocs en manipulant la bitmap de blocs de chaque groupe.
// La bitmap occupe un bloc (généralement 4096 octets = 32768 bits = 32768 blocs).
//
// Principe :
//   1. Cherche un groupe avec des blocs libres (GroupDescTable).
//   2. Charge la bitmap de blocs du groupe (BIO Read).
//   3. Recherche le premier bit à 0 (premier bloc libre).
//   4. Marque le bit à 1 et réécrit la bitmap sur disque.
//   5. Met à jour le GroupDesc (free_blocks_count--).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{FsError, FsResult};
use crate::fs::block::bio::{Bio, BioOp, BioFlags, BioVec};
use crate::fs::block::queue::submit_bio;
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::ext4plus::group_desc::GroupDescTable;
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};

// ─────────────────────────────────────────────────────────────────────────────
// Bitmap helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Trouve le premier bit à 0 dans `bitmap`. Retourne son index ou None si pleine.
fn find_first_zero_bit(bitmap: &[u8]) -> Option<usize> {
    for (byte_idx, &byte) in bitmap.iter().enumerate() {
        if byte != 0xFF {
            let bit = byte.trailing_ones() as usize;
            return Some(byte_idx * 8 + bit);
        }
    }
    None
}

/// Positionne le bit à `bit_idx` (0-based) dans la bitmap.
fn set_bit(bitmap: &mut [u8], bit_idx: usize) {
    bitmap[bit_idx / 8] |= 1 << (bit_idx % 8);
}

/// Efface le bit à `bit_idx` dans la bitmap.
fn clear_bit(bitmap: &mut [u8], bit_idx: usize) {
    bitmap[bit_idx / 8] &= !(1 << (bit_idx % 8));
}

/// Teste si le bit `bit_idx` est positionné.
fn test_bit(bitmap: &[u8], bit_idx: usize) -> bool {
    (bitmap[bit_idx / 8] >> (bit_idx % 8)) & 1 != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// ext4_alloc_block — alloue un bloc
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue un seul bloc EXT4 dans le groupe `preferred_group` si possible,
/// sinon dans n'importe quel groupe avec des blocs libres.
/// Retourne le numéro de bloc physique (absolu).
pub fn ext4_alloc_block(
    sb:              &Arc<RwLock<Ext4Superblock>>,
    gdt:             &GroupDescTable,
    preferred_group: Option<u64>,
    bitmap_buf:      PhysAddr,
) -> FsResult<u64> {
    let bsize         = sb.read().block_size;
    let blks_per_grp  = sb.read().blocks_per_group;
    let dev           = sb.read().dev.0;

    let group_idx = if let Some(g) = preferred_group {
        let gd = gdt.get(g)?;
        if gd.free_blocks_count > 0 { g }
        else { gdt.find_group_with_free_blocks(1).ok_or(FsError::NoSpace)? }
    } else {
        gdt.find_group_with_free_blocks(1).ok_or(FsError::NoSpace)?
    };

    let mut gd = gdt.get(group_idx)?;

    // Charge la bitmap de blocs
    let bmap_sector = gd.block_bitmap * bsize / 512;
    let bio = Bio {
        id: 0, op: BioOp::Read, dev, sector: bmap_sector,
        vecs: alloc::vec![BioVec { phys: bitmap_buf, virt: bitmap_buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags: BioFlags::META,
        status: AtomicU8::new(0), bytes: AtomicU64::new(0),
        callback: None, cb_data: 0,
    };
    submit_bio(bio)?;

    // SAFETY: bitmap_buf rempli par submit_bio, bsize octets valides
    let bitmap_slice = unsafe {
        core::slice::from_raw_parts_mut(bitmap_buf.as_u64() as *mut u8, bsize as usize)
    };

    let bit = find_first_zero_bit(bitmap_slice).ok_or(FsError::NoSpace)?;
    if bit >= blks_per_grp as usize { return Err(FsError::NoSpace); }

    set_bit(bitmap_slice, bit);

    // Réécrit la bitmap sur disque
    let write_bio = Bio {
        id: 0, op: BioOp::Write, dev, sector: bmap_sector,
        vecs: alloc::vec![BioVec { phys: bitmap_buf, virt: bitmap_buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags: BioFlags::META | BioFlags::FUA,
        status: AtomicU8::new(0), bytes: AtomicU64::new(0),
        callback: None, cb_data: 0,
    };
    submit_bio(write_bio)?;

    // Met à jour le GDT
    gd.free_blocks_count -= 1;
    gd.dirty = true;
    gdt.update(gd.clone());

    let phys_block = group_idx * blks_per_grp as u64 + bit as u64;

    BALLOC_STATS.allocs.fetch_add(1, Ordering::Relaxed);
    Ok(phys_block)
}

// ─────────────────────────────────────────────────────────────────────────────
// ext4_free_block — libère un bloc
// ─────────────────────────────────────────────────────────────────────────────

/// Libère le bloc `phys_block` dans sa bitmap de groupe.
pub fn ext4_free_block(
    sb:         &Arc<RwLock<Ext4Superblock>>,
    gdt:        &GroupDescTable,
    phys_block: u64,
    bitmap_buf: PhysAddr,
) -> FsResult<()> {
    let bsize        = sb.read().block_size;
    let blks_per_grp = sb.read().blocks_per_group as u64;
    let dev          = sb.read().dev.0;

    let group_idx = phys_block / blks_per_grp;
    let bit       = (phys_block % blks_per_grp) as usize;
    let mut gd    = gdt.get(group_idx)?;

    // Charge la bitmap
    let bmap_sector = gd.block_bitmap * bsize / 512;
    let bio = Bio {
        id: 0, op: BioOp::Read, dev, sector: bmap_sector,
        vecs: alloc::vec![BioVec { phys: bitmap_buf, virt: bitmap_buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags: BioFlags::META,
        status: AtomicU8::new(0), bytes: AtomicU64::new(0),
        callback: None, cb_data: 0,
    };
    submit_bio(bio)?;

    let bitmap_slice = unsafe {
        core::slice::from_raw_parts_mut(bitmap_buf.as_u64() as *mut u8, bsize as usize)
    };

    if !test_bit(bitmap_slice, bit) { return Err(FsError::InvalidArgument); }
    clear_bit(bitmap_slice, bit);

    // Réécrit
    let write_bio = Bio {
        id: 0, op: BioOp::Write, dev, sector: bmap_sector,
        vecs: alloc::vec![BioVec { phys: bitmap_buf, virt: bitmap_buf.as_u64(), len: bsize as u32, offset: 0 }],
        flags: BioFlags::META | BioFlags::FUA,
        status: AtomicU8::new(0), bytes: AtomicU64::new(0),
        callback: None, cb_data: 0,
    };
    submit_bio(write_bio)?;

    gd.free_blocks_count += 1;
    gd.dirty = true;
    gdt.update(gd);

    BALLOC_STATS.frees.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// BAllocStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct BAllocStats {
    pub allocs:   AtomicU64,
    pub frees:    AtomicU64,
    pub failures: AtomicU64,
}

impl BAllocStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { allocs: z!(), frees: z!(), failures: z!() }
    }
}

pub static BALLOC_STATS: BAllocStats = BAllocStats::new();
