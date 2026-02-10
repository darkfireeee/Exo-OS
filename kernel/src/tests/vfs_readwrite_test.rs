//! VFS Read/Write Validation Test
//!
//! Tests that VFS correctly writes and reads back data
//! Specifically for large files like ELF binaries

use crate::fs::core::vfs;
use alloc::vec::Vec;

/// Test VFS read/write round-trip with various sizes
pub fn test_vfs_readwrite_roundtrip() {
    log::info!("[VFS_TEST] === Starting VFS Read/Write Round-Trip Tests ===");

    // Test 1: Small file (< 1 page)
    test_roundtrip("/tmp/test_small.bin", 128);

    // Test 2: Exactly 1 page
    test_roundtrip("/tmp/test_1page.bin", 4096);

    // Test 3: Multiple pages
    test_roundtrip("/tmp/test_multi.bin", 8192);

    // Test 4: ELF-like size (4112 bytes - what create_minimal_elf creates)
    test_roundtrip("/tmp/test_elf_size.bin", 4112);

    // Test 5: Large file
    test_roundtrip("/tmp/test_large.bin", 16384);

    log::info!("[VFS_TEST] === All VFS Read/Write Tests PASSED ===");
}

/// Test write then read for a specific size
fn test_roundtrip(path: &str, size: usize) {
    log::info!("[VFS_TEST] Testing {} bytes at {}", size, path);

    // Create test data with pattern
    let mut original_data = Vec::with_capacity(size);
    for i in 0..size {
        original_data.push((i % 256) as u8);
    }

    // Write to VFS
    match vfs::write_file(path, &original_data) {
        Ok(_) => log::debug!("[VFS_TEST]   Write OK: {} bytes", original_data.len()),
        Err(e) => {
            log::error!("[VFS_TEST]   ❌ Write FAILED: {:?}", e);
            panic!("VFS write failed for {}", path);
        }
    }

    // Read back from VFS
    let read_data = match vfs::read_file(path) {
        Ok(data) => {
            log::debug!("[VFS_TEST]   Read OK: {} bytes", data.len());
            data
        }
        Err(e) => {
            log::error!("[VFS_TEST]   ❌ Read FAILED: {:?}", e);
            panic!("VFS read failed for {}", path);
        }
    };

    // Verify size matches
    if read_data.len() != original_data.len() {
        log::error!(
            "[VFS_TEST]   ❌ SIZE MISMATCH: wrote {} bytes, read {} bytes",
            original_data.len(),
            read_data.len()
        );
        panic!("VFS size mismatch");
    }

    // Verify content matches
    for (i, (&original, &read)) in original_data.iter().zip(read_data.iter()).enumerate() {
        if original != read {
            log::error!(
                "[VFS_TEST]   ❌ DATA MISMATCH at offset {}: wrote {:#x}, read {:#x}",
                i, original, read
            );
            panic!("VFS data corruption");
        }
    }

    log::info!("[VFS_TEST]   ✅ {} bytes: write/read verified", size);
}

/// Test that demonstrates the exact ELF scenario
pub fn test_elf_scenario() {
    log::info!("[VFS_TEST] === Testing ELF Write/Read Scenario ===");

    // Create data that mimics create_minimal_elf()
    // - 64 bytes ELF header
    // - 56 bytes program header  
    // - Padding to 0x1000
    // - 16 bytes code
    let mut elf_data = Vec::new();

    // ELF header (64 bytes)
    for i in 0..64 {
        elf_data.push(i as u8);
    }

    // Program header (56 bytes)
    for i in 0..56 {
        elf_data.push((64 + i) as u8);
    }

    // Padding to 0x1000
    while elf_data.len() < 0x1000 {
        elf_data.push(0);
    }

    // Code (16 bytes)
    elf_data.extend_from_slice(&[
        0x48, 0xc7, 0xc0, 0x3c, 0x00, 0x00, 0x00,
        0x48, 0x31, 0xff,
        0x0f, 0x05,
        0xf4, 0xf4, 0xf4, 0xf4,
    ]);

    let total_size = elf_data.len();
    log::info!("[VFS_TEST] Created ELF-like data: {} bytes", total_size);

    // Write to VFS
    let test_path = "/bin/test_elf_scenario";
    match vfs::write_file(test_path, &elf_data) {
        Ok(_) => log::info!("[VFS_TEST]   Wrote {} bytes to {}", total_size, test_path),
        Err(e) => {
            log::error!("[VFS_TEST]   ❌ Write failed: {:?}", e);
            panic!("ELF scenario write failed");
        }
    }

    // Read back
    let read_back = match vfs::read_file(test_path) {
        Ok(data) => {
            log::info!("[VFS_TEST]   Read {} bytes from {}", data.len(), test_path);
            data
        }
        Err(e) => {
            log::error!("[VFS_TEST]   ❌ Read failed: {:?}", e);
            panic!("ELF scenario read failed");
        }
    };

    // Verify
    if read_back.len() != total_size {
        log::error!(
            "[VFS_TEST]   ❌ SIZE MISMATCH: wrote {} bytes, read {} bytes",
            total_size,
            read_back.len()
        );
        log::error!("[VFS_TEST]   This is the BUG that breaks ELF loading!");
        panic!("VFS ELF scenario size mismatch");
    }

    // Verify segment data region (offset 0x1000 + 16 bytes)
    let segment_offset = 0x1000;
    let segment_size = 16;

    if read_back.len() < segment_offset + segment_size {
        log::error!(
            "[VFS_TEST]   ❌ File too short: {} bytes, need {} bytes for segment",
            read_back.len(),
            segment_offset + segment_size
        );
        panic!("Cannot read segment data - file truncated");
    }

    log::info!("[VFS_TEST]   ✅ Segment data accessible at offset {:#x}", segment_offset);
    log::info!("[VFS_TEST]   ✅ ELF scenario test PASSED");
}
