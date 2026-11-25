//! storage — Couche d'accès disque ExoFS (façade de câblage).

pub mod ata_pio;
pub mod blob_reader;
pub mod blob_writer;
pub mod block_allocator;
pub mod block_cache;
pub mod checksum_reader;
pub mod checksum_writer;
pub mod compression_choice;
pub mod compression_reader;
pub mod compression_writer;
pub mod dedup_reader;
pub mod dedup_writer;
pub mod extent_reader;
pub mod extent_writer;
pub mod heap;
pub mod heap_allocator;
pub mod heap_coalesce;
pub mod heap_free_map;
pub mod io_batch;
pub mod layout;
mod lifecycle;
pub mod object_reader;
pub mod object_writer;
pub mod partition_scan;
pub mod ramdisk;
pub mod storage_stats;
pub mod superblock;
pub mod superblock_backup;
pub mod virtio_adapter;

pub use blob_reader::{
    verify_blob_range, BatchBlobReadResult, BatchBlobReader, BlobReadRequest, BlobReadResult,
    BlobReader, BlobScanner, BlobVerifyMode, IntegrityReport, BLOB_READER_STATS,
};
pub use blob_writer::{
    blob_total_disk_size, verify_blob_header, BatchBlobWriter, BlobHeaderDisk, BlobWriteResult,
    BlobWriter, BlobWriterConfig, BLOB_HEADER_MAGIC, BLOB_HEADER_SIZE, BLOB_WRITER_STATS,
};
pub use block_allocator::{BlockAllocator, ExtentHandle, ExtentState};
pub use block_cache::BlockCache;
pub use checksum_reader::{ChecksumReader, VerifyMode};
pub use checksum_writer::{BlockChecksumMap, ChecksumTag, ChecksumWriter};
pub use compression_choice::{choose_compression, CompressionType, ContentHint};
pub use compression_reader::DecompressReader;
pub use compression_writer::{CompressWriter, CompressedBlockHeader};
pub use dedup_reader::DedupReader;
pub use dedup_writer::{DedupDecision, DedupEntry, DedupWriter};
pub use extent_reader::{ExtentBlockIterator, ExtentReader};
pub use extent_writer::{Extent, ExtentMap, ExtentWriter};
pub use heap::{ExofsHeap, HeapConfig};
pub use heap_allocator::{AllocationPolicy, HeapAllocator};
pub use heap_coalesce::{CoalesceReport, HeapCoalescer};
pub use heap_free_map::HeapFreeMap;
pub use io_batch::{BatchStats, IoBatch, IoBatchQueue, IoBatchReport};
pub use layout::{align_down, align_up, DiskZone, LayoutMap, BLOCK_SIZE, HEAP_START_OFFSET};
pub use lifecycle::{
    storage_init, storage_is_ready, storage_shutdown, StorageHealthReport, StorageInitError,
    StorageInitResult,
};
pub use object_reader::{
    verify_objects, ObjectIntegrityReport, ObjectMeta, ObjectReadResult, ObjectReader,
    ObjectScanner, ObjectVerifyMode, OBJECT_READER_STATS,
};
pub use object_writer::{
    BlobRef, ObjectHeaderDisk, ObjectType, ObjectWriteResult, ObjectWriter, ObjectWriterConfig,
    OBJECT_HEADER_MAGIC, OBJECT_HEADER_SIZE, OBJECT_WRITER_STATS,
};
pub use storage_stats::{StorageStats, StorageStatsSnapshot, STORAGE_STATS};
pub use superblock::{
    superblock_mirror_offsets, verify_superblock_bytes, ExoSuperblockDisk, MirrorSlot,
    SbManagerState, SuperblockManager, SuperblockSnapshot, EXOFS_MAGIC, FORMAT_VERSION_MAJOR,
    SUPERBLOCK_DISK_SIZE,
};
pub use superblock_backup::{recover_superblock, write_superblock_mirrors};
