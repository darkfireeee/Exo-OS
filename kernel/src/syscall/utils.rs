//! Syscall Utilities
//!
//! Helper functions for handling user space data.

use alloc::string::String;
use alloc::vec::Vec;
use core::slice;

/// Read a null-terminated string from user space
pub unsafe fn read_user_string(ptr: *const i8) -> Result<String, ()> {
    if ptr.is_null() {
        return Err(());
    }

    let mut len = 0;
    loop {
        let c = *ptr.add(len);
        if c == 0 {
            break;
        }
        len += 1;
        if len > 4096 {
            // Max path length safety check
            return Err(());
        }
    }

    let slice = slice::from_raw_parts(ptr as *const u8, len);
    String::from_utf8(slice.to_vec()).map_err(|_| ())
}

/// Copy data to user space
pub unsafe fn copy_to_user(dest: *mut u8, src: &[u8]) -> Result<(), ()> {
    if dest.is_null() {
        return Err(());
    }

    // TODO: Validate user pointer range
    core::ptr::copy_nonoverlapping(src.as_ptr(), dest, src.len());
    Ok(())
}

/// Write a type to user space
pub unsafe fn write_user_type<T>(dest: *mut T, value: T) -> Result<(), ()> {
    if dest.is_null() {
        return Err(());
    }

    // TODO: Validate user pointer range
    *dest = value;
    Ok(())
}
