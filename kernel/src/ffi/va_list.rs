//! Variable argument list support (va_list)
//! 
//! Provides C-compatible variadic function support

use core::fmt;

/// Variable argument list (x86_64 ABI)
#[repr(C)]
pub struct VaList {
    gp_offset: u32,
    fp_offset: u32,
    overflow_arg_area: *mut u8,
    reg_save_area: *mut u8,
}

impl VaList {
    /// Get next argument as type T
    pub unsafe fn arg<T: VaArg>(&mut self) -> T {
        T::va_arg(self)
    }
}

/// Trait for types that can be passed as varargs
pub trait VaArg: Sized {
    unsafe fn va_arg(list: &mut VaList) -> Self;
}

// Implement VaArg for common types
impl VaArg for i32 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        // General purpose argument
        if list.gp_offset < 48 {
            let ptr = list.reg_save_area.add(list.gp_offset as usize) as *const i32;
            list.gp_offset += 8;
            *ptr
        } else {
            let ptr = list.overflow_arg_area as *const i32;
            list.overflow_arg_area = list.overflow_arg_area.add(8);
            *ptr
        }
    }
}

impl VaArg for u32 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        i32::va_arg(list) as u32
    }
}

impl VaArg for i64 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        if list.gp_offset < 48 {
            let ptr = list.reg_save_area.add(list.gp_offset as usize) as *const i64;
            list.gp_offset += 8;
            *ptr
        } else {
            let ptr = list.overflow_arg_area as *const i64;
            list.overflow_arg_area = list.overflow_arg_area.add(8);
            *ptr
        }
    }
}

impl VaArg for u64 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        i64::va_arg(list) as u64
    }
}

impl VaArg for usize {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        u64::va_arg(list) as usize
    }
}

impl VaArg for *const u8 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        usize::va_arg(list) as *const u8
    }
}

impl VaArg for *mut u8 {
    unsafe fn va_arg(list: &mut VaList) -> Self {
        usize::va_arg(list) as *mut u8
    }
}

impl fmt::Debug for VaList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "VaList {{ gp_offset: {}, fp_offset: {} }}", 
               self.gp_offset, self.fp_offset)
    }
}
