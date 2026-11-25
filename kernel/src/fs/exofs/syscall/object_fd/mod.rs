//! # fs/exofs/syscall/object_fd/mod.rs — Table de descripteurs de fichiers ExoFS (façade)

pub mod entry;
pub mod flags;
mod table;
pub mod table_inner;
#[cfg(test)]
mod tests;

pub use entry::{ObjectFdEntry, FD_MAX, FD_RESERVED, MAX_FDS};
pub use flags as open_flags;
pub use table::{ObjectFdTable, OBJECT_LIFECYCLE_LOCK, OBJECT_TABLE};
