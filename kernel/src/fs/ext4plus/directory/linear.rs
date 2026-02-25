// kernel/src/fs/ext4plus/directory/linear.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ LINEAR DIRECTORY — scan linéaire des blocs de répertoire
// ═══════════════════════════════════════════════════════════════════════════════
//
// Pour les petits répertoires (<= 1 à 2 blocs) ou en mode fallback HTree.
//
// Format d'une entrée de répertoire EXT4 (Ext4DirEntry2) :
//   ino       u32  — numéro d'inode (0 = entrée libre)
//   rec_len   u16  — longueur de l'enregistrement (alignée sur 4 octets)
//   name_len  u8   — longueur du nom
//   file_type u8   — type de fichier (FEAT_INCOMPAT_FILETYPE requis)
//   name      [u8] — nom (name_len octets, sans \0 terminal)
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::core::types::{FsError, FsResult, InodeNumber, Dirent64, FileType};

// ─────────────────────────────────────────────────────────────────────────────
// Ext4DirEntry2 on-disk header
// ─────────────────────────────────────────────────────────────────────────────

pub const EXT4_FT_UNKNOWN:  u8 = 0;
pub const EXT4_FT_REG_FILE: u8 = 1;
pub const EXT4_FT_DIR:      u8 = 2;
pub const EXT4_FT_CHRDEV:   u8 = 3;
pub const EXT4_FT_BLKDEV:   u8 = 4;
pub const EXT4_FT_FIFO:     u8 = 5;
pub const EXT4_FT_SOCK:     u8 = 6;
pub const EXT4_FT_SYMLINK:  u8 = 7;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Ext4DirEntryHeader {
    pub inode:     u32,
    pub rec_len:   u16,
    pub name_len:  u8,
    pub file_type: u8,
}

pub const DIR_ENTRY_HDR_SIZE: usize = core::mem::size_of::<Ext4DirEntryHeader>();

/// Représentation en mémoire d'une entrée de répertoire.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub ino:       InodeNumber,
    pub name:      Vec<u8>,
    pub file_type: u8,
    /// Offset de l'entrée dans le bloc (pour suppression / modification)
    pub offset:    u32,
    pub rec_len:   u16,
}

impl DirEntry {
    pub fn is_dot(&self)     -> bool { self.name == b"." }
    pub fn is_dotdot(&self)  -> bool { self.name == b".." }
    pub fn is_valid(&self)   -> bool { self.ino.0 != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// parse_dir_block — itère les entrées d'un bloc de répertoire
// ─────────────────────────────────────────────────────────────────────────────

/// Analyse toutes les entrées valides d'un bloc de répertoire.
///
/// # Safety
/// `data` pointe sur `bsize` octets initialisés constitutant un bloc EXT4.
pub unsafe fn parse_dir_block(data: *const u8, bsize: usize) -> Vec<DirEntry> {
    let mut entries = Vec::new();
    let mut offset  = 0usize;

    while offset + DIR_ENTRY_HDR_SIZE <= bsize {
        let hdr = (data.add(offset) as *const Ext4DirEntryHeader).read_unaligned();
        if hdr.rec_len < DIR_ENTRY_HDR_SIZE as u16 || hdr.rec_len == 0 { break; }

        if hdr.inode != 0 && hdr.name_len > 0 {
            let name_ptr   = data.add(offset + DIR_ENTRY_HDR_SIZE);
            let name_len   = hdr.name_len as usize;
            let end_of_name = offset + DIR_ENTRY_HDR_SIZE + name_len;
            if end_of_name <= bsize {
                let name = core::slice::from_raw_parts(name_ptr, name_len).to_vec();
                entries.push(DirEntry {
                    ino:       InodeNumber(hdr.inode as u64),
                    name,
                    file_type: hdr.file_type,
                    offset:    offset as u32,
                    rec_len:   hdr.rec_len,
                });
            }
        }
        offset += hdr.rec_len as usize;
    }
    LINEAR_STATS.parses.fetch_add(1, Ordering::Relaxed);
    entries
}

// ─────────────────────────────────────────────────────────────────────────────
// linear_lookup — cherche un nom dans un bloc
// ─────────────────────────────────────────────────────────────────────────────

/// Trouve l'entrée `name` dans un bloc de répertoire déjà analysé.
pub fn linear_lookup<'a>(entries: &'a [DirEntry], name: &[u8]) -> Option<&'a DirEntry> {
    entries.iter().find(|e| e.name == name)
}

// ─────────────────────────────────────────────────────────────────────────────
// linear_emit — construit des Dirent64 pour getdents64
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit des DirEntry en Dirent64 (offset logique incrémental).
pub fn linear_emit(entries: &[DirEntry], base_offset: u64) -> Vec<Dirent64> {
    let mut out = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        let mut name64 = [0u8; 256];
        let nlen = e.name.len().min(255);
        name64[..nlen].copy_from_slice(&e.name[..nlen]);

        let reclen = (core::mem::size_of::<Dirent64>() + nlen + 1 + 7) & !7;
        out.push(Dirent64 {
            d_ino:    e.ino.0,
            d_off:    (base_offset + i as u64 + 1) as i64,
            d_reclen: reclen as u16,
            d_type:   unsafe { core::mem::transmute::<u8, crate::fs::core::types::DirEntryType>(e.file_type) },
            _pad:     0,
            d_name:   name64,
        });
    }
    LINEAR_STATS.emits.fetch_add(out.len() as u64, Ordering::Relaxed);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// linear_add_entry — insère une entrée dans un bloc (espace libre suffisant)
// ─────────────────────────────────────────────────────────────────────────────

/// Tente d'insérer `name` → `ino` dans `block_data` (modifié en place).
/// Retourne Ok(()) si l'insertion a réussi, Err(NoSpace) si le bloc est plein.
pub fn linear_add_entry(
    block_data: &mut [u8],
    name:       &[u8],
    ino:        InodeNumber,
    file_type:  u8,
) -> FsResult<()> {
    let needed = (DIR_ENTRY_HDR_SIZE + name.len() + 3) & !3;
    let mut offset = 0usize;

    while offset + DIR_ENTRY_HDR_SIZE <= block_data.len() {
        // SAFETY: offset+HDR <= block_data.len() par le check du while
        let hdr = unsafe {
            (block_data.as_ptr().add(offset) as *const Ext4DirEntryHeader).read_unaligned()
        };
        if hdr.rec_len == 0 { break; }

        let actual_used = if hdr.inode != 0 && hdr.name_len > 0 {
            (DIR_ENTRY_HDR_SIZE + hdr.name_len as usize + 3) & !3
        } else { 0 };
        let free_space = hdr.rec_len as usize - actual_used;

        if free_space >= needed {
            // Rétrécit l'entrée courante et insère après
            let new_off = offset + actual_used;
            // Réécrit rec_len de l'entrée courante
            let new_rec_len = actual_used as u16;
            unsafe {
                let ptr = block_data.as_mut_ptr().add(offset + 4) as *mut u16;
                ptr.write_unaligned(new_rec_len);
            }
            // Écrit la nouvelle entrée
            let new_hdr = Ext4DirEntryHeader {
                inode:     ino.0 as u32,
                rec_len:   (free_space as u16).max(needed as u16),
                name_len:  name.len() as u8,
                file_type,
            };
            let dest = &mut block_data[new_off..];
            // SAFETY: new_off + HDR_SIZE <= block_data.len() car free_space >= needed
            unsafe {
                (dest.as_mut_ptr() as *mut Ext4DirEntryHeader).write_unaligned(new_hdr);
                dest.as_mut_ptr().add(DIR_ENTRY_HDR_SIZE).copy_from(name.as_ptr(), name.len());
            }
            LINEAR_STATS.inserts.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        offset += hdr.rec_len as usize;
    }
    Err(FsError::NoSpace)
}

// ─────────────────────────────────────────────────────────────────────────────
// linear_remove_entry — marque une entrée comme libre (inode → 0)
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime l'entrée `name` dans `block_data` en mettant son inode à 0
/// et en fusionnant son espace libre avec l'entrée précédente.
pub fn linear_remove_entry(block_data: &mut [u8], name: &[u8]) -> FsResult<()> {
    let bsize     = block_data.len();
    let mut offset= 0usize;
    let mut prev_off: Option<usize> = None;

    while offset + DIR_ENTRY_HDR_SIZE <= bsize {
        let hdr = unsafe {
            (block_data.as_ptr().add(offset) as *const Ext4DirEntryHeader).read_unaligned()
        };
        if hdr.rec_len == 0 { break; }

        let matches = hdr.inode != 0 && hdr.name_len as usize == name.len() && {
            let nptr = unsafe { block_data.as_ptr().add(offset + DIR_ENTRY_HDR_SIZE) };
            unsafe { core::slice::from_raw_parts(nptr, name.len()) == name }
        };

        if matches {
            if let Some(prev) = prev_off {
                // Fusionne l'espace libéré avec l'entrée précédente
                let prev_hdr = unsafe {
                    (block_data.as_ptr().add(prev) as *const Ext4DirEntryHeader).read_unaligned()
                };
                let merged_rec = prev_hdr.rec_len + hdr.rec_len;
                unsafe {
                    let p = block_data.as_mut_ptr().add(prev + 4) as *mut u16;
                    p.write_unaligned(merged_rec);
                }
            } else {
                // Première entrée : met juste inode à 0
                unsafe {
                    let p = block_data.as_mut_ptr().add(offset) as *mut u32;
                    p.write_unaligned(0u32);
                }
            }
            LINEAR_STATS.removes.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        prev_off = Some(offset);
        offset  += hdr.rec_len as usize;
    }
    Err(FsError::NotFound)
}

// ─────────────────────────────────────────────────────────────────────────────
// LinearStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct LinearStats {
    pub parses:  AtomicU64,
    pub emits:   AtomicU64,
    pub inserts: AtomicU64,
    pub removes: AtomicU64,
}

impl LinearStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { parses: z!(), emits: z!(), inserts: z!(), removes: z!() }
    }
}

pub static LINEAR_STATS: LinearStats = LinearStats::new();
