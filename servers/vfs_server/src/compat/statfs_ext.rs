//! statfs mapping for ExoFS.

pub const EXOFS_MAGIC: u64 = 0x4558_4F46;
pub const DEFAULT_BLOCK_SIZE: u64 = 4096;
pub const DEFAULT_NAME_MAX: u64 = 255;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StatFsView {
    pub f_type: u64,
    pub f_bsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_namelen: u64,
}

impl StatFsView {
    pub const fn new(blocks: u64, free: u64, files: u64, files_free: u64) -> Self {
        let bavail = if free > blocks { blocks } else { free };
        Self {
            f_type: EXOFS_MAGIC,
            f_bsize: DEFAULT_BLOCK_SIZE,
            f_blocks: blocks,
            f_bfree: free,
            f_bavail: bavail,
            f_files: files,
            f_ffree: files_free,
            f_namelen: DEFAULT_NAME_MAX,
        }
    }
}
