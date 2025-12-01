//! File Descriptors and related types

use crate::fs::vfs::inode::Inode;
use alloc::sync::Arc;
use spin::RwLock;

/// File offset
pub type Offset = i64;

/// File permissions/mode
pub type Mode = u32;

/// File descriptor type
pub type Fd = i32;

/// File flags
#[derive(Debug, Clone, Copy)]
pub struct FileFlags {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
    pub nonblock: bool,
}

/// File statistics
#[derive(Debug, Clone, Copy)]
pub struct FileStat {
    pub size: usize,
    pub blocks: usize,
    pub block_size: usize,
    pub inode: u64,
    pub mode: Mode,
    pub nlink: usize,
}

/// Seek whence
#[derive(Debug, Clone, Copy)]
pub enum SeekWhence {
    Start = 0,
    Current = 1,
    End = 2,
}

/// File Descriptor
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub fd: Fd,
    pub inode: u64,
    pub offset: usize,
    pub flags: FileFlags,
}
