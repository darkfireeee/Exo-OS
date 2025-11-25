//! Memory Management for x86_64

pub mod paging;
pub mod tlb;
pub mod pat;
pub mod numa;

pub use paging::*;
pub use tlb::*;
