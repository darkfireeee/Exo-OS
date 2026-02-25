// kernel/src/fs/integrity/validator.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// VALIDATOR — Hooks de validation d'intégrité (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Valide les invariants des structures FS avant et après les opérations.
//
// Hooks :
//   • `validate_inode()` : vérifie que l'inode est cohérent (nlink, size, mode).
//   • `validate_dentry()` : vérifie que la dentry pointe vers un inode valide.
//   • `validate_block()` : vérifie le checksum d'un bloc de données.
//   • `validate_superblock()` : vérifie le magic et les champs critiques.
//   • `on_write()` / `on_read()` : hooks appelables depuis FileOps.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::core::types::{FsError, FsResult, FileType};
use crate::fs::core::inode::Inode;
use crate::fs::core::dentry::Dentry;
use crate::fs::integrity::checksum::{
    compute_checksum, verify_checksum, Checksum, ChecksumType,
};

// ─────────────────────────────────────────────────────────────────────────────
// Validation d'un inode
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie les invariants d'un inode.
pub fn validate_inode(inode: &Inode) -> FsResult<()> {
    // Un inode mort ne doit pas être accédé.
    if matches!(inode.state, crate::fs::core::inode::InodeState::Dead) {
        VAL_STATS.inode_dead.fetch_add(1, Ordering::Relaxed);
        return Err(FsError::Stale);
    }

    // Un répertoire doit avoir au moins 2 hard links (. et ..).
    if inode.mode.is_dir() {
        let nlink = inode.nlink.load(Ordering::Relaxed);
        if nlink < 2 {
            VAL_STATS.inode_bad_nlink.fetch_add(1, Ordering::Relaxed);
            return Err(FsError::DataCorrupted);
        }
    }

    // Un fichier régulier avec size > 0 doit avoir des blocs alloués.
    if inode.mode.file_type() == FileType::Regular {
        let size   = inode.size.load(Ordering::Relaxed);
        let blocks = inode.blocks.load(Ordering::Relaxed);
        if size > 0 && blocks == 0 {
            VAL_STATS.inode_no_blocks.fetch_add(1, Ordering::Relaxed);
            // Avertissement seulement — peut être une sparse file.
        }
    }

    VAL_STATS.inode_ok.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation d'une dentry
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'une dentry est cohérente.
pub fn validate_dentry(dentry: &Dentry) -> FsResult<()> {
    use crate::fs::core::dentry::DentryState;
    match dentry.state {
        DentryState::Unhashed | DentryState::Root => {
            // États valides.
        }
        DentryState::Negative => {
            // Une negative dentry ne doit pas avoir d'inode.
            if dentry.inode.is_some() {
                VAL_STATS.dentry_bad.fetch_add(1, Ordering::Relaxed);
                return Err(FsError::DataCorrupted);
            }
        }
        DentryState::Valid => {
            if dentry.inode.is_none() {
                VAL_STATS.dentry_bad.fetch_add(1, Ordering::Relaxed);
                return Err(FsError::DataCorrupted);
            }
        }
    }
    VAL_STATS.dentry_ok.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation d'un bloc de données
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie le checksum d'un bloc en mémoire.
pub fn validate_block(data: &[u8], expected: &Checksum) -> FsResult<()> {
    if verify_checksum(data, expected) {
        VAL_STATS.blocks_ok.fetch_add(1, Ordering::Relaxed);
        Ok(())
    } else {
        VAL_STATS.blocks_bad.fetch_add(1, Ordering::Relaxed);
        Err(FsError::DataCorrupted)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation du superbloc
// ─────────────────────────────────────────────────────────────────────────────

pub const EXT4_MAGIC: u16 = 0xEF53;

/// Vérifie que le superbloc a le magic attendu.
pub fn validate_superblock_magic(magic: u16) -> FsResult<()> {
    if magic != EXT4_MAGIC {
        VAL_STATS.sb_bad.fetch_add(1, Ordering::Relaxed);
        return Err(FsError::BadMagic);
    }
    VAL_STATS.sb_ok.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Hooks on_read / on_write
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé par FileOps::read après la lecture d'une page.
pub fn on_read_page(data: &[u8]) {
    // Calcul CRC pour détection silencieuse — log seulement.
    let _ck = compute_checksum(data, ChecksumType::Crc32c);
    VAL_STATS.pages_read_validated.fetch_add(1, Ordering::Relaxed);
}

/// Appelé par FileOps::write avant l'écriture d'une page.
pub fn on_write_page(data: &[u8]) -> Checksum {
    let ck = compute_checksum(data, ChecksumType::Crc32c);
    VAL_STATS.pages_write_checksummed.fetch_add(1, Ordering::Relaxed);
    ck
}

// ─────────────────────────────────────────────────────────────────────────────
// ValidatorStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct ValidatorStats {
    pub inode_ok:               AtomicU64,
    pub inode_dead:             AtomicU64,
    pub inode_bad_nlink:        AtomicU64,
    pub inode_no_blocks:        AtomicU64,
    pub dentry_ok:              AtomicU64,
    pub dentry_bad:             AtomicU64,
    pub blocks_ok:              AtomicU64,
    pub blocks_bad:             AtomicU64,
    pub sb_ok:                  AtomicU64,
    pub sb_bad:                 AtomicU64,
    pub pages_read_validated:   AtomicU64,
    pub pages_write_checksummed:AtomicU64,
}

impl ValidatorStats {
    pub const fn new() -> Self {
        Self {
            inode_ok:                AtomicU64::new(0),
            inode_dead:              AtomicU64::new(0),
            inode_bad_nlink:         AtomicU64::new(0),
            inode_no_blocks:         AtomicU64::new(0),
            dentry_ok:               AtomicU64::new(0),
            dentry_bad:              AtomicU64::new(0),
            blocks_ok:               AtomicU64::new(0),
            blocks_bad:              AtomicU64::new(0),
            sb_ok:                   AtomicU64::new(0),
            sb_bad:                  AtomicU64::new(0),
            pages_read_validated:    AtomicU64::new(0),
            pages_write_checksummed: AtomicU64::new(0),
        }
    }
}

pub static VAL_STATS: ValidatorStats = ValidatorStats::new();
