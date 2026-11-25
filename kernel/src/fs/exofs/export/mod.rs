//! mod.rs — ExoFS public export module wiring facade.

pub mod exoar_format;
pub mod exoar_reader;
pub mod exoar_writer;
pub mod export_audit;
pub mod incremental_export;
pub mod metadata_export;
mod pipeline;
pub mod stream_export;
pub mod stream_import;
pub mod tar_compat;

pub use exoar_format::{
    crc32c_compute, crc32c_verify, ExoarEntryHeader, ExoarEntryInfo, ExoarEntryKind, ExoarFooter,
    ExoarHeader, ExoarSummary, ARCHIVE_FLAG_INCREMENTAL, ARCHIVE_FLAG_SNAPSHOT,
    ARCHIVE_FLAG_VERIFIED, ENTRY_FLAG_COMPRESSED, ENTRY_FLAG_ENCRYPTED, ENTRY_FLAG_TOMBSTONE,
    EXOAR_ENTRY_MAGIC, EXOAR_FOOTER_MAGIC, EXOAR_MAGIC, EXOAR_MAX_ENTRIES, EXOAR_MAX_PAYLOAD,
    EXOAR_VERSION,
};
pub use exoar_reader::{
    ArchiveSource, BlobReceiver, CollectingReceiver, ExoarReadError, ExoarReadReport, ExoarReader,
    ExoarReaderConfig, ExoarScanner, SliceSource,
};
pub use exoar_writer::{
    ArchiveSink, ExoarBufferedWriter, ExoarWriteError, ExoarWriteOptions, ExoarWriteStats,
    ExoarWriter, SinkVec,
};
pub use export_audit::{
    ExportAuditEntry, ExportAuditLog, ExportAuditStats, ExportEvent, ExportSession,
    ExportSessionConfig, SessionState, EXPORT_AUDIT, EXPORT_AUDIT_RING,
};
pub use incremental_export::{
    DiffSet, EpochId, EpochRange, IncrementalBlobSource, IncrementalExport,
    IncrementalExportConfig, IncrementalExportResult, MultiEpochExportSummary, SnapshotExport,
};
pub use metadata_export::{
    BlobMeta, ChunkMeta, ExportManifest, ManifestBlobEntry, MetaBinaryHeader, MetadataBinaryWriter,
    MetadataExporter, SnapshotMeta, TextSink, VecTextSink, META_BINARY_MAGIC,
};
pub use pipeline::{
    exoar_to_tar, export_blob_to_archive, import_first_from_archive, verify_archive, ExportConfig,
    ExportHealthStatus, ExportModule, ExportModuleSummary,
};
pub use stream_export::{
    BatchState, BlobDataProvider, FilterMode, StreamCheckpoint, StreamExportBatch,
    StreamExportConfig, StreamExportReport, StreamExporter, StreamFilter, StreamSink,
};
pub use stream_import::{
    BlobWriter, ConflictResolver, ImportCheckpoint, ImportEntryHeader, ImportSource,
    ImportStreamBuilder, SliceImportSource, StreamImportConfig, StreamImportReport, StreamImporter,
    TombstoneHandler, IMPORT_ENTRY_MAGIC, IMPORT_FLAG_TOMBSTONE, IMPORT_FLAG_VERIFIED,
};
pub use tar_compat::{
    tar_checksum_compute, tar_checksum_verify, ExoarToTarConverter, SliceTarSource, TarBlock,
    TarEmitStats, TarEmitter, TarEntry, TarEntryKind, TarHeader, TarParseReport, TarParser,
    TarSink, TarSource, TarToExoarConverter, TarToExoarResult, VecTarSink, TAR_BLOCK_SIZE,
    TAR_MAGIC, TAR_NAME_MAX, TAR_VERSION,
};
