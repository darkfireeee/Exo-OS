//! Log sinks (output destinations)

pub mod file;
pub mod ipc;

pub use file::FileSink;
pub use ipc::IpcSink;
