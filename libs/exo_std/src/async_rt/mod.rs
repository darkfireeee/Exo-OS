//! Async runtime for Exo-OS
//!
//! Provides a single-threaded async executor with task scheduling,
//! wakers, and integration with kernel async primitives.

pub mod executor;
pub mod task;
pub mod waker;

pub use executor::{Executor, block_on, spawn};
pub use task::{Task, TaskId, JoinHandle};
pub use waker::Waker;

/// Re-export core async types for convenience
pub use core::future::Future;
pub use core::task::{Context, Poll};
