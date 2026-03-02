// kernel/src/fs/exofs/storage/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module storage — Couche d'accès disque ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

pub mod layout;
pub mod superblock;
pub mod superblock_backup;
pub mod heap;
pub mod block_allocator;
pub mod object_writer;
pub mod object_reader;
pub mod blob_writer;
pub mod blob_reader;
pub mod storage_stats;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use layout::{
    superblock_primary, epoch_slot_a, epoch_slot_b, epoch_slot_c,
    heap_start, zone_end, check_bounds, blocks_to_offset, align_up,
    superblock_mirror_end,
};
pub use superblock::{
    ExoSuperblockDisk, ExoSuperblockInMemory, ExofsVfsSuperblock, read_and_verify,
};
pub use superblock_backup::{superblock_mirror_offsets, write_superblock_mirrors};
pub use heap::{ExofsHeap, round_up_to_block_size, blocks_for_size};
pub use block_allocator::BlockAllocator;
pub use object_writer::{ObjectHeader, ObjectWriteResult, write_object};
pub use object_reader::{ObjectReadResult, read_object, read_object_header};
pub use blob_writer::{BlobHeader, BlobWriteResult, write_blob};
pub use blob_reader::{BlobReadResult, read_blob};
pub use storage_stats::{StorageStats, StorageStatsSnapshot, STORAGE_STATS};
