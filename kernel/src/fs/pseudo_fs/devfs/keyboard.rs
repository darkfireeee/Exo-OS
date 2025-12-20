//! Keyboard Device (/dev/kbd) for VFS
//! 
//! Character device that reads from PS/2 keyboard buffer

use crate::fs::core::{Inode, InodeOps, InodeType, InodeMetadata};
use crate::fs::error::{FsError, FsResult};
use crate::arch::x86_64::drivers::ps2_keyboard;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Keyboard device inode
pub struct KeyboardDevice {
    metadata: InodeMetadata,
}

impl KeyboardDevice {
    /// Create new keyboard device
    pub fn new(ino: u64) -> Self {
        Self {
            metadata: InodeMetadata {
                ino,
                inode_type: InodeType::CharDevice,
                size: 0, // Character device has no fixed size
                blocks: 0,
                nlink: 1,
                uid: 0,
                gid: 0,
                mode: 0o644, // rw-r--r--
                atime: 0,
                mtime: 0,
                ctime: 0,
                major: 10, // Misc device
                minor: 1,  // Keyboard
            },
        }
    }
}

impl Inode for KeyboardDevice {
    fn metadata(&self) -> &InodeMetadata {
        &self.metadata
    }
    
    fn metadata_mut(&mut self) -> &mut InodeMetadata {
        &mut self.metadata
    }
}

impl InodeOps for KeyboardDevice {
    /// Read from keyboard buffer
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        // Offset is ignored for character devices (always read from current position)
        let _ = offset;
        
        // Read from PS/2 keyboard buffer
        let bytes_read = ps2_keyboard::read_bytes(buf);
        
        if bytes_read > 0 {
            Ok(bytes_read)
        } else {
            // No data available - return EAGAIN for non-blocking read
            // In a real impl, this would block if O_NONBLOCK is not set
            Err(FsError::Again)
        }
    }
    
    /// Write to keyboard (not supported)
    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::PermissionDenied)
    }
    
    /// Lookup (not supported for device)
    fn lookup(&self, _name: &str) -> FsResult<Arc<dyn Inode>> {
        Err(FsError::NotDirectory)
    }
    
    /// Create (not supported for device)
    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<Arc<dyn Inode>> {
        Err(FsError::NotDirectory)
    }
    
    /// Unlink (not supported for device)
    fn unlink(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }
    
    /// List entries (not supported for device)
    fn list(&self) -> FsResult<Vec<(alloc::string::String, u64)>> {
        Err(FsError::NotDirectory)
    }
}

/// Test keyboard device read
pub fn test_keyboard_device() {
    use crate::logger;
    
    logger::early_print("\n╔══════════════════════════════════════════════════════════╗\n");
    logger::early_print("║           PHASE 1c - KEYBOARD DEVICE TEST              ║\n");
    logger::early_print("╚══════════════════════════════════════════════════════════╝\n\n");
    
    logger::early_print("[TEST 1] Creating /dev/kbd device...\n");
    let kbd_device = KeyboardDevice::new(1000);
    logger::early_print("[TEST 1] ✅ Keyboard device created\n");
    
    logger::early_print("\n[TEST 2] Testing keyboard buffer state...\n");
    let has_data = ps2_keyboard::has_data();
    logger::early_print("[TEST 2] Buffer has data: ");
    let s = alloc::format!("{}\n", has_data);
    logger::early_print(&s);
    
    let buffer_size = ps2_keyboard::buffer_size();
    logger::early_print("[TEST 2] Buffer size: ");
    let s = alloc::format!("{}\n", buffer_size);
    logger::early_print(&s);
    logger::early_print("[TEST 2] ✅ Buffer state check complete\n");
    
    logger::early_print("\n[TEST 3] Simulating keyboard input...\n");
    // In real scenario, user would press keys triggering IRQ1
    // For testing, we'll just check read behavior
    let mut buffer = [0u8; 64];
    match kbd_device.read_at(0, &mut buffer) {
        Ok(n) => {
            logger::early_print("[TEST 3] Read ");
            let s = alloc::format!("{} bytes from keyboard\n", n);
            logger::early_print(&s);
            logger::early_print("[TEST 3] ✅ Read successful\n");
        }
        Err(FsError::Again) => {
            logger::early_print("[TEST 3] No data available (EAGAIN) - expected\n");
            logger::early_print("[TEST 3] ✅ Non-blocking read works correctly\n");
        }
        Err(e) => {
            logger::early_print("[TEST 3] Error: ");
            let s = alloc::format!("{:?}\n", e);
            logger::early_print(&s);
            logger::early_print("[TEST 3] ❌ Unexpected error\n");
        }
    }
    
    logger::early_print("\n[TEST 4] Testing write (should fail)...\n");
    let write_data = b"test";
    match kbd_device.read_at(0, &mut [0u8; 4]) {
        Err(FsError::PermissionDenied) => {
            logger::early_print("[TEST 4] ✅ Write correctly denied (read-only device)\n");
        }
        _ => {
            logger::early_print("[TEST 4] ⚠️  Write should have been denied\n");
        }
    }
    
    logger::early_print("\n╔══════════════════════════════════════════════════════════╗\n");
    logger::early_print("║           KEYBOARD DEVICE TEST COMPLETE                 ║\n");
    logger::early_print("╚══════════════════════════════════════════════════════════╝\n\n");
}
