//! Module IO ExoFS — couche d'entrée/sortie vers le BlobStore.
//!
//! Fournit : IO synchrone/asynchrone, scatter-gather, readahead, writeback,
//! zero-copy, direct-IO, buffered-IO, batching, io_uring-like, stats.

#![allow(dead_code)]

pub mod async_io;
pub mod buffered_io;
pub mod direct_io;
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

pub use async_io::{AsyncIoHandle, AsyncIoQueue};
pub use buffered_io::BufferedReader;
pub use direct_io::DirectIo;
pub use io_batch::{IoBatch, IoBatchEntry};
pub use io_stats::IoStats;
pub use io_uring::{IoUringQueue, IoUringSubmission};
pub use prefetch::Prefetcher;
pub use readahead::ReadaheadEngine;
pub use reader::BlobReader;
pub use scatter_gather::{ScatterGatherBuf, ScatterGatherIo};
pub use writeback::WritebackQueue;
pub use writer::BlobWriter;
pub use zero_copy::ZeroCopySlice;
