//! Shared coreutils helpers split by concern (formatting, paths/FS, recursive remove, Strata `ls`).
#![allow(unused_imports, dead_code)]

mod io_fmt;
mod path_fs;
mod remove;
mod strata_ls;

pub use io_fmt::*;
pub use path_fs::*;
pub use remove::*;
pub use strata_ls::*;
