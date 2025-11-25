//! FFI (Foreign Function Interface) module
//! 
//! Provides safe interfaces for interoperability between Rust and C code

pub mod types;
pub mod c_str;
pub mod va_list;
pub mod callbacks;
pub mod bindings;

// Re-exports
pub use types::*;
pub use c_str::{CStr, CString};
pub use va_list::VaList;
pub use callbacks::*;
