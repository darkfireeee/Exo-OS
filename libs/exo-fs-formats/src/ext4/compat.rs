// drivers/fs/src/ext4/compat.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4 CLASSIQUE — Vérification de compatibilité  (exo-os-driver-fs)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE FS-EXT4-01 : Vérifier les flags INCOMPAT avant tout accès.
// RÈGLE FS-EXT4-02 : Détecter un disque ext4plus (s_exo_version != 0) → refuser.
// RÈGLE FS-EXT4-03 : Journal non propre → lecture seule uniquement.
// RÈGLE FS-EXT4-04 : Jamais de Blake3 ou delayed alloc sur ext4 classique.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::FsDriverError;

// ── Flags INCOMPAT connus ────────────────────────────────────────────────────

pub const EXT4_INCOMPAT_FILETYPE:    u32 = 0x0002;
pub const EXT4_INCOMPAT_RECOVER:     u32 = 0x0004;
pub const EXT4_INCOMPAT_META_BG:     u32 = 0x0010;
pub const EXT4_INCOMPAT_EXTENTS:     u32 = 0x0040;
pub const EXT4_INCOMPAT_64BIT:       u32 = 0x0080;
pub const EXT4_INCOMPAT_MMP:         u32 = 0x0100;
pub const EXT4_INCOMPAT_FLEX_BG:     u32 = 0x0200;
pub const EXT4_INCOMPAT_EA_INODE:    u32 = 0x0400;
pub const EXT4_INCOMPAT_DIRDATA:     u32 = 0x1000;
pub const EXT4_INCOMPAT_LARGEDIR:    u32 = 0x4000;
pub const EXT4_INCOMPAT_INLINE_DATA: u32 = 0x8000;

pub const EXT4_KNOWN_INCOMPAT_FLAGS: u32 =
    EXT4_INCOMPAT_FILETYPE | EXT4_INCOMPAT_RECOVER | EXT4_INCOMPAT_META_BG |
    EXT4_INCOMPAT_EXTENTS  | EXT4_INCOMPAT_64BIT   | EXT4_INCOMPAT_MMP     |
    EXT4_INCOMPAT_FLEX_BG  | EXT4_INCOMPAT_EA_INODE| EXT4_INCOMPAT_DIRDATA |
    EXT4_INCOMPAT_LARGEDIR | EXT4_INCOMPAT_INLINE_DATA;

pub const EXT4_KNOWN_RO_COMPAT_FLAGS: u32 =
    0x0001 | // SPARSE_SUPER
    0x0002 | // LARGE_FILE
    0x0004 | // BTREE_DIR
    0x0008 | // HUGE_FILE
    0x0010 | // GDT_CSUM
    0x0020 | // DIR_NLINK
    0x0040 | // EXTRA_ISIZE
    0x0100 | // QUOTA
    0x0200 | // BIGALLOC
    0x0400;  // METADATA_CSUM

/// Mode de montage proposé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext4MountMode {
    ReadWrite,
    ReadOnly { reason: &'static str },
}

/// Vérifie qu'un superblock ext4 classique est montable.
///
/// # Règles appliquées
/// - FS-EXT4-01 : flags INCOMPAT inconnus → refus
/// - FS-EXT4-02 : s_exo_version != 0 → c'est ext4plus → refus
/// - FS-EXT4-03 : journal non propre → read-only
pub fn verify_before_mount(
    s_feature_incompat: u32,
    s_exo_version:      u32,
    needs_recovery:     bool,
) -> Result<Ext4MountMode, FsDriverError> {
    // ── FS-EXT4-02 : détecter ext4plus ----------
    if s_exo_version != 0 {
        COMPAT_STATS.rejected_ext4plus.fetch_add(1, Ordering::Relaxed);
        return Err(FsDriverError::IsExt4Plus);
    }

    // ── FS-EXT4-01 : flags inconnus ----------
    let unknown = s_feature_incompat & !EXT4_KNOWN_INCOMPAT_FLAGS;
    if unknown != 0 {
        COMPAT_STATS.unknown_incompat.fetch_add(1, Ordering::Relaxed);
        return Err(FsDriverError::UnknownIncompatFlags { flags: unknown });
    }

    // ── FS-EXT4-03 : journal non propre ----------
    if needs_recovery {
        COMPAT_STATS.journal_dirty_ro.fetch_add(1, Ordering::Relaxed);
        return Ok(Ext4MountMode::ReadOnly {
            reason: "Journal has unrecovered data — mounting read-only",
        });
    }

    COMPAT_STATS.successful_verifs.fetch_add(1, Ordering::Relaxed);
    Ok(Ext4MountMode::ReadWrite)
}

// ── Stats ────────────────────────────────────────────────────────────────────

pub struct CompatStats {
    pub rejected_ext4plus:  AtomicU64,
    pub unknown_incompat:   AtomicU64,
    pub journal_dirty_ro:   AtomicU64,
    pub successful_verifs:  AtomicU64,
}

impl CompatStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { rejected_ext4plus: z!(), unknown_incompat: z!(), journal_dirty_ro: z!(), successful_verifs: z!() }
    }
}

pub static COMPAT_STATS: CompatStats = CompatStats::new();
