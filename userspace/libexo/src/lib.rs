pub mod errno;
pub mod sys;
pub mod vfs;

pub type Result<T> = core::result::Result<T, errno::Errno>;
