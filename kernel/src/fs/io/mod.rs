// kernel/src/fs/io/mod.rs
//
// I/O asynchrone FS — uring, zero-copy, AIO POSIX, mmap, direct I/O, completion.

pub mod completion;
pub mod uring;
pub mod zero_copy;
pub mod aio;
pub mod mmap;
pub mod direct_io;

pub use completion::{
    IoOp, IoStatus, IoResult, IoRequest, IoReqRef, IoToken,
    CompletionQueue, CompletionEntry, CqStats, CQ_STATS,
};
pub use uring::{
    SqEntry, CqEntry, UringRing, UringContext, UringStats, URING_STATS,
};
pub use zero_copy::{
    SpliceFlags, ZeroCopyStats, ZC_STATS,
    splice_pages, sendfile_pages, tee_pages,
};
pub use aio::{
    AioOpcode, AioCb, AioCbRef, AioContext, AioStats, AIO_STATS,
};
pub use mmap::{
    MmapProt, MmapRegion, MmapManager, MmapStats, MMAP_STATS,
};
pub use direct_io::{
    DIO_ALIGN, DIO_BLOCK_ALIGN, DioVec, DioStats, DIO_STATS,
    check_dio_alignment, direct_read, direct_write,
    direct_readv, direct_writev,
};
