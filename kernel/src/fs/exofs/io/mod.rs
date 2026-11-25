//! mod.rs — Module IO ExoFS (façade de câblage).

pub mod async_io;
pub mod buffered_io;
pub mod direct_io;
mod facade;
pub mod io_batch;
pub mod io_stats;
pub mod io_uring;
pub mod prefetch;
pub mod readahead;
pub mod reader;
pub mod scatter_gather;
pub mod writeback;
pub mod writer;
pub mod zero_copy;

pub use async_io::{AsyncIoHandle, AsyncIoQueue, AsyncOpKind, AsyncState, ASYNC_IO_QUEUE};
pub use buffered_io::{BufferedReader, BufferedWriter, IoBuffer, RingBuffer};
pub use direct_io::{BlockSize, DirectIo, DirectIoBuffer, DirectIoConfig};
pub use facade::{
    flush_all, read_blob_quick, write_blob_quick, IoConfig, IoHealthStatus, IoModule,
    IoModuleSummary, IO_MODULE,
};
pub use io_batch::{BatchResult, BatchStats, IoBatch, IoBatchEntry};
pub use io_stats::{IoOpKind, IoStats, IoStatsSnapshot, IO_STATS};
pub use io_uring::{IoUringCqe, IoUringQueue, IoUringSqe, SqeOpcode};
pub use prefetch::{PrefetchConfig, PrefetchEntry, PrefetchQueue, PrefetchStrategy, Prefetcher};
pub use readahead::{
    BlockAccessLog, ReadaheadEngine, ReadaheadPolicy, ReadaheadScheduler, ReadaheadStats,
    ReadaheadWindow,
};
pub use reader::{BlobReader, BlobStore, ReadConfig, ReadResult, ReaderStats, VerifyMode};
pub use scatter_gather::{PhysSegment, PhysSgList, SgEngine, SgFragment, SgList};
pub use writeback::{
    WritebackConfig, WritebackEntry, WritebackQueue, WritebackStats, WritebackWorker,
    WRITEBACK_QUEUE,
};
pub use writer::{BlobStoreWrite, BlobWriter, WriteConfig, WriterStats};
pub use zero_copy::{
    ZeroCopyPipe, ZeroCopyReader, ZeroCopySlice, ZeroCopyStats, ZeroCopyWindow, ZeroCopyWriter,
};
