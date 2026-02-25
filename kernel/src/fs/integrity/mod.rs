// kernel/src/fs/integrity/mod.rs
//
// Intégrité FS — checksum, journal WAL, recovery, scrubbing, healing, validation.

pub mod checksum;
pub mod journal;
pub mod recovery;
pub mod scrubbing;
pub mod healing;
pub mod validator;

pub use checksum::{
    ChecksumType, Checksum, ChecksumStats, CKSUM_STATS,
    crc32c, adler32, xxhash64, blake3_hash,
    compute_checksum, verify_checksum,
    checksum_crc32c, checksum_adler32, checksum_xxhash64, checksum_blake3,
};
pub use journal::{
    JournalEntryType, LogHeader, JournalEntry, JournalHandle, JournalHandleRef,
    Journal, JournalStats, JOURNAL_STATS, JOURNAL_MAGIC,
};
pub use recovery::{
    RecoveryResult, RecoveryStats, RECOVERY_STATS,
    journal_recovery, needs_recovery,
};
pub use scrubbing::{
    ScrubTask, ScrubResult, ScrubEngine, ScrubStats,
    SCRUB_ENGINE, SCRUB_STATS,
};
pub use healing::{
    HealStrategy, HealRequest, HealEngine, HealStats,
    HEAL_ENGINE, HEAL_STATS,
};
pub use validator::{
    EXT4_MAGIC, ValidatorStats, VAL_STATS,
    validate_inode, validate_dentry, validate_block,
    validate_superblock_magic, on_read_page, on_write_page,
};
