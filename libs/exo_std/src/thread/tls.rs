//! Thread-Local Storage (TLS) implementation
//!
//! Provides userspace TLS management that integrates with kernel-provided
//! TLS templates from ELF loading. Handles allocation, initialization, and
//! cleanup of per-thread TLS blocks.

extern crate alloc;

use alloc::alloc::{alloc, dealloc, Layout};
use core::ptr;
use crate::error::ThreadError;
use crate::syscall::{syscall2, SyscallNumber};

/// arch_prctl constants (Linux-compatible)
pub const ARCH_SET_GS: i32 = 0x1001;
pub const ARCH_SET_FS: i32 = 0x1002;
pub const ARCH_GET_FS: i32 = 0x1003;
pub const ARCH_GET_GS: i32 = 0x1004;

/// TLS template from ELF loading
///
/// This structure mirrors the kernel's TlsTemplate and contains
/// the information needed to initialize TLS for each thread.
#[derive(Debug, Clone)]
pub struct TlsTemplate {
    /// Address of TLS initialization data (.tdata segment)
    pub addr: usize,
    /// Size of initialized data from file
    pub file_size: usize,
    /// Total size in memory (includes .tbss zero-initialized data)
    pub mem_size: usize,
    /// Alignment requirement
    pub align: usize,
}

impl TlsTemplate {
    /// Create a new TLS template
    pub const fn new(addr: usize, file_size: usize, mem_size: usize, align: usize) -> Self {
        Self {
            addr,
            file_size,
            mem_size,
            align,
        }
    }

    /// Calculate the properly aligned block size
    pub fn block_size(&self) -> usize {
        (self.mem_size + self.align - 1) & !(self.align - 1)
    }

    /// Check if template is valid
    pub fn is_valid(&self) -> bool {
        self.file_size <= self.mem_size && self.align > 0 && self.align.is_power_of_two()
    }
}

/// Thread-Local Storage block
///
/// Represents allocated TLS data for a single thread. Contains both
/// initialized data (from .tdata) and zero-initialized data (from .tbss).
pub struct TlsBlock {
    /// Pointer to allocated TLS data
    data: *mut u8,
    /// Total size of allocation
    size: usize,
    /// Alignment requirement
    align: usize,
}

impl TlsBlock {
    /// Allocate and initialize a new TLS block from a template
    ///
    /// # Safety
    /// - Template must be valid and point to accessible memory
    /// - Template data must remain valid during allocation
    ///
    /// # Process
    /// 1. Allocate aligned memory block
    /// 2. Copy initialized data from .tdata segment
    /// 3. Zero-initialize remaining space (.tbss)
    /// 4. Call arch_prctl to set FS base
    pub unsafe fn allocate(template: &TlsTemplate) -> Result<Self, ThreadError> {
        if !template.is_valid() {
            return Err(ThreadError::TlsInvalid);
        }

        let size = template.block_size();
        let align = template.align.max(8); // Minimum 8-byte alignment

        // Allocate aligned memory
        let layout = Layout::from_size_align(size, align)
            .map_err(|_| ThreadError::TlsAllocationFailed)?;

        let data = alloc(layout);
        if data.is_null() {
            return Err(ThreadError::TlsAllocationFailed);
        }

        // Copy initialized data from .tdata
        if template.file_size > 0 && template.addr != 0 {
            ptr::copy_nonoverlapping(template.addr as *const u8, data, template.file_size);
        }

        // Zero-initialize .tbss (uninitialized data)
        if template.mem_size > template.file_size {
            let tbss_start = data.add(template.file_size);
            let tbss_size = template.mem_size - template.file_size;
            ptr::write_bytes(tbss_start, 0, tbss_size);
        }

        let block = Self { data, size, align };

        // Set FS base to point to TLS block
        block.set_fs_base()?;

        Ok(block)
    }

    /// Set the FS base register to point to this TLS block
    ///
    /// FS register is used for thread-local storage on x86_64.
    /// Applications access TLS via %fs:offset addressing.
    fn set_fs_base(&self) -> Result<(), ThreadError> {
        unsafe {
            let ret = syscall2(
                SyscallNumber::ArchPrctl,
                ARCH_SET_FS as usize,
                self.data as usize,
            );

            if ret < 0 {
                Err(ThreadError::TlsSetupFailed)
            } else {
                Ok(())
            }
        }
    }

    /// Get the FS base register value
    pub fn get_fs_base() -> Result<usize, ThreadError> {
        let mut addr: usize = 0;

        unsafe {
            let ret = syscall2(
                SyscallNumber::ArchPrctl,
                ARCH_GET_FS as usize,
                &mut addr as *mut usize as usize,
            );

            if ret < 0 {
                Err(ThreadError::TlsSetupFailed)
            } else {
                Ok(addr)
            }
        }
    }

    /// Set GS base register (alternative to FS)
    pub fn set_gs_base(&self) -> Result<(), ThreadError> {
        unsafe {
            let ret = syscall2(
                SyscallNumber::ArchPrctl,
                ARCH_SET_GS as usize,
                self.data as usize,
            );

            if ret < 0 {
                Err(ThreadError::TlsSetupFailed)
            } else {
                Ok(())
            }
        }
    }

    /// Get pointer to TLS data
    pub fn as_ptr(&self) -> *mut u8 {
        self.data
    }

    /// Get size of TLS block
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get alignment of TLS block
    pub fn align(&self) -> usize {
        self.align
    }

    /// Write a value at a specific offset in TLS
    ///
    /// # Safety
    /// - Offset + size_of::<T>() must be within bounds
    pub unsafe fn write_at<T>(&mut self, offset: usize, value: T) -> Result<(), ThreadError> {
        if offset + core::mem::size_of::<T>() > self.size {
            return Err(ThreadError::TlsInvalid);
        }

        let ptr = self.data.add(offset) as *mut T;
        ptr::write(ptr, value);
        Ok(())
    }

    /// Read a value from a specific offset in TLS
    ///
    /// # Safety
    /// - Offset + size_of::<T>() must be within bounds
    /// - Data at offset must be initialized
    pub unsafe fn read_at<T: Copy>(&self, offset: usize) -> Result<T, ThreadError> {
        if offset + core::mem::size_of::<T>() > self.size {
            return Err(ThreadError::TlsInvalid);
        }

        let ptr = self.data.add(offset) as *const T;
        Ok(ptr::read(ptr))
    }
}

impl Drop for TlsBlock {
    fn drop(&mut self) {
        if !self.data.is_null() {
            unsafe {
                let layout = Layout::from_size_align_unchecked(self.size, self.align);
                dealloc(self.data, layout);
            }
        }
    }
}

unsafe impl Send for TlsBlock {}

/// Global TLS template (set during program initialization)
///
/// This will be populated by the program loader when parsing the ELF
/// PT_TLS segment. Each thread will allocate its own TLS block based
/// on this template.
static mut GLOBAL_TLS_TEMPLATE: Option<TlsTemplate> = None;

/// Initialize the global TLS template
///
/// Should be called once during program startup by the loader.
///
/// # Safety
/// - Must be called only once
/// - Must be called before any threads are spawned
pub unsafe fn init_tls_template(template: TlsTemplate) {
    GLOBAL_TLS_TEMPLATE = Some(template);
}

/// Get the global TLS template
pub fn get_tls_template() -> Option<TlsTemplate> {
    unsafe { GLOBAL_TLS_TEMPLATE.clone() }
}

/// Allocate TLS for the current thread using the global template
///
/// This is the main entry point for initializing TLS for a new thread.
pub fn allocate_current_thread_tls() -> Result<TlsBlock, ThreadError> {
    let template = get_tls_template().ok_or(ThreadError::TlsNotInitialized)?;

    unsafe { TlsBlock::allocate(&template) }
}

/// Helper function to setup TLS for thread spawn
///
/// Called automatically by thread::spawn() to initialize TLS for new threads.
#[doc(hidden)]
pub fn setup_thread_tls() -> Result<Option<TlsBlock>, ThreadError> {
    // If there's a global template, allocate TLS
    if let Some(template) = get_tls_template() {
        unsafe {
            let block = TlsBlock::allocate(&template)?;
            Ok(Some(block))
        }
    } else {
        // No TLS template, thread runs without TLS
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_template_creation() {
        let template = TlsTemplate::new(0x1000, 64, 128, 16);
        assert_eq!(template.addr, 0x1000);
        assert_eq!(template.file_size, 64);
        assert_eq!(template.mem_size, 128);
        assert_eq!(template.align, 16);
        assert!(template.is_valid());
    }

    #[test]
    fn test_tls_template_block_size() {
        let template = TlsTemplate::new(0x1000, 64, 100, 16);
        assert_eq!(template.block_size(), 112); // Aligned to 16
    }

    #[test]
    fn test_tls_template_invalid() {
        // file_size > mem_size
        let template = TlsTemplate::new(0x1000, 200, 100, 16);
        assert!(!template.is_valid());

        // Non-power-of-2 alignment
        let template = TlsTemplate::new(0x1000, 64, 128, 15);
        assert!(!template.is_valid());

        // Zero alignment
        let template = TlsTemplate::new(0x1000, 64, 128, 0);
        assert!(!template.is_valid());
    }

    #[test]
    #[cfg(feature = "test_mode")]
    fn test_tls_block_allocation() {
        // Create a mock template with static data
        static TLS_DATA: [u8; 64] = [42u8; 64];

        let template = TlsTemplate::new(
            TLS_DATA.as_ptr() as usize,
            64,
            128,
            16,
        );

        unsafe {
            let block = TlsBlock::allocate(&template);
            assert!(block.is_ok());

            let block = block.unwrap();
            assert_eq!(block.size(), 128);
            assert_eq!(block.align(), 16);

            // Verify initialized data was copied
            let data = core::slice::from_raw_parts(block.as_ptr(), 64);
            assert_eq!(data[0], 42);
            assert_eq!(data[63], 42);

            // Verify .tbss was zeroed
            let tbss = core::slice::from_raw_parts(block.as_ptr().add(64), 64);
            assert_eq!(tbss[0], 0);
            assert_eq!(tbss[63], 0);
        }
    }

    #[test]
    #[cfg(feature = "test_mode")]
    fn test_tls_write_read() {
        let template = TlsTemplate::new(0, 0, 128, 8);

        unsafe {
            let mut block = TlsBlock::allocate(&template).unwrap();

            // Write an integer
            block.write_at(0, 0x12345678u32).unwrap();

            // Read it back
            let value: u32 = block.read_at(0).unwrap();
            assert_eq!(value, 0x12345678);

            // Write at different offset
            block.write_at(64, 0xDEADBEEFu64).unwrap();
            let value: u64 = block.read_at(64).unwrap();
            assert_eq!(value, 0xDEADBEEF);

            // Test out of bounds
            assert!(block.write_at::<u64>(125, 0).is_err());
        }
    }

    #[test]
    fn test_global_template() {
        let template = TlsTemplate::new(0x2000, 32, 64, 8);

        unsafe {
            init_tls_template(template.clone());
        }

        let retrieved = get_tls_template().unwrap();
        assert_eq!(retrieved.addr, 0x2000);
        assert_eq!(retrieved.file_size, 32);
        assert_eq!(retrieved.mem_size, 64);
        assert_eq!(retrieved.align, 8);
    }
}
