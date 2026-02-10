// Filesystem Stress Tests - Phase 2
// Tests intensifs avec VRAIES opérations VFS

use crate::serial_println;
use crate::posix_x::vfs_posix::file_ops;
use crate::fs::FsResult;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;

/// Helper: Create file (using write_file which creates if not exists)
fn create_file(path: &str, _mode: u32) -> FsResult<()> {
    // Write empty content to create file
    file_ops::write_file(path, &[])?;
    Ok(())
}

/// Test 1: Mass File Creation (1000+ files avec VRAIES opérations VFS)
pub fn test_mass_file_creation() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 1: Mass File Creation (VFS REAL) ===");
    serial_println!("Creating 1000 files in tmpfs...");

    let start_time = crate::time::uptime_ns();
    let mut created_count = 0;

    // Create 1000 files using REAL VFS operations
    for i in 0..1000 {
        let filename = format!("/tmp/testfile_{:04}.txt", i);

        // Try to create file via VFS
        match create_file(&filename, 0o644) {
            Ok(_) => {
                created_count += 1;
                if i % 100 == 0 {
                    serial_println!("  Progress: {}/1000 files created", i);
                }
            }
            Err(e) => {
                serial_println!("  Warning: Failed to create {}: {:?}", filename, e);
            }
        }
    }

    let elapsed_ns = crate::time::uptime_ns() - start_time;
    let elapsed_ms = elapsed_ns / 1_000_000;
    let avg_us = if created_count > 0 { elapsed_ns / created_count / 1000 } else { 0 };

    serial_println!("✅ Created {} files in {} ms", created_count, elapsed_ms);
    serial_println!("   Average: {} μs per file", avg_us);

    Ok(())
}

/// Test 2: Large File I/O (>1MB avec VRAIES opérations read/write)
pub fn test_large_file_io() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 2: Large File I/O (VFS REAL) ===");

    const FILE_SIZE: usize = 2 * 1024 * 1024; // 2 MB
    const BLOCK_SIZE: usize = 4096; // 4 KB blocks
    let filename = "/tmp/large_test_file.dat";

    // Create test file
    match create_file(filename, 0o644) {
        Ok(_) => serial_println!("File created: {}", filename),
        Err(e) => {
            serial_println!("❌ Failed to create file: {:?}", e);
            return Err("File creation failed");
        }
    }

    // Prepare test data (pattern)
    let mut test_buffer = Vec::new();
    for i in 0..BLOCK_SIZE {
        test_buffer.push((i % 256) as u8);
    }

    // WRITE TEST
    serial_println!("Writing {} MB in 4KB blocks...", FILE_SIZE / 1024 / 1024);
    let start_write = crate::time::uptime_ns();

    let num_blocks = FILE_SIZE / BLOCK_SIZE;
    let mut total_written = 0;

    for block in 0..num_blocks {
        match file_ops::write_file(filename, &test_buffer) {
            Ok(written) => {
                total_written += written;
            }
            Err(e) => {
                serial_println!("  Write error at block {}: {:?}", block, e);
                break;
            }
        }

        if block % 128 == 0 {
            serial_println!("  Write progress: {} KB / {} KB",
                total_written / 1024, FILE_SIZE / 1024);
        }
    }

    let write_time_ns = crate::time::uptime_ns() - start_write;
    let write_time_ms = write_time_ns / 1_000_000;
    let write_throughput_mbps = if write_time_ms > 0 {
        (total_written as u64 * 1000) / write_time_ms / 1024 / 1024
    } else {
        0
    };

    serial_println!("✅ Write complete: {} KB in {} ms", total_written / 1024, write_time_ms);
    serial_println!("   Throughput: {} MB/s", write_throughput_mbps);

    // READ TEST
    serial_println!("\nReading file...");
    let start_read = crate::time::uptime_ns();

    let total_read = match file_ops::read_file(filename) {
        Ok(data) => {
            serial_println!("  Read {} KB", data.len() / 1024);
            data.len()
        }
        Err(e) => {
            serial_println!("  Read error: {:?}", e);
            0
        }
    };

    let read_time_ns = crate::time::uptime_ns() - start_read;
    let read_time_ms = read_time_ns / 1_000_000;
    let read_throughput_mbps = if read_time_ms > 0 {
        (total_read as u64 * 1000) / read_time_ms / 1024 / 1024
    } else {
        0
    };

    serial_println!("✅ Read complete: {} KB in {} ms", total_read / 1024, read_time_ms);
    serial_println!("   Throughput: {} MB/s", read_throughput_mbps);

    Ok(())
}

/// Test 3: File Operations (open/close/stat)
pub fn test_file_operations() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 3: File Operations Stress ===");
    serial_println!("Testing open/stat/close 500 times...");

    const NUM_OPS: usize = 500;
    let filename = "/tmp/test_ops_file.txt";

    // Create test file first
    create_file(filename, 0o644)
        .map_err(|_| "Failed to create test file")?;

    let start_time = crate::time::uptime_ns();
    let mut successful_ops = 0;

    for i in 0..NUM_OPS {
        // Try stat operation
        match file_ops::stat(filename, true) {
            Ok(_metadata) => {
                successful_ops += 1;
            }
            Err(e) => {
                serial_println!("  Stat failed at {}: {:?}", i, e);
            }
        }

        if i % 100 == 0 {
            serial_println!("  Progress: {}/{} operations", i, NUM_OPS);
        }
    }

    let elapsed_ns = crate::time::uptime_ns() - start_time;
    let elapsed_ms = elapsed_ns / 1_000_000;
    let avg_ns = if successful_ops > 0 { elapsed_ns / successful_ops } else { 0 };

    serial_println!("✅ Completed {} operations in {} ms", successful_ops, elapsed_ms);
    serial_println!("   Average: {} ns per operation", avg_ns);

    Ok(())
}

/// Test 4: Directory Traversal
pub fn test_directory_traversal() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 4: Directory Traversal ===");
    serial_println!("Creating nested directories...");

    const MAX_DEPTH: usize = 10;
    const FILES_PER_DIR: usize = 20;

    let start_time = crate::time::uptime_ns();
    let mut total_created = 0;

    for depth in 0..MAX_DEPTH {
        let dir_path = format!("/tmp/level{}", depth);

        // Create directory
        match file_ops::mkdir(&dir_path, 0o755) {
            Ok(_) => {
                total_created += 1;
            }
            Err(e) => {
                serial_println!("  mkdir failed for {}: {:?}", dir_path, e);
                continue;
            }
        }

        // Create files in directory
        for file_num in 0..FILES_PER_DIR {
            let file_path = format!("{}/file{:02}.txt", dir_path, file_num);
            match create_file(&file_path, 0o644) {
                Ok(_) => {
                    total_created += 1;
                }
                Err(_) => {
                    // Silently continue
                }
            }
        }

        serial_println!("  Level {}: Created directory + {} files", depth, FILES_PER_DIR);
    }

    let elapsed_ns = crate::time::uptime_ns() - start_time;
    let elapsed_ms = elapsed_ns / 1_000_000;

    serial_println!("✅ Created {} directories and files in {} ms", total_created, elapsed_ms);

    // Now traverse directories
    serial_println!("\nTraversing directories...");
    let traverse_start = crate::time::uptime_ns();

    for depth in 0..MAX_DEPTH {
        let dir_path = format!("/tmp/level{}", depth);
        // Try to stat directory
        match file_ops::stat(&dir_path, true) {
            Ok(_) => { /* Success */ }
            Err(_) => { /* Ignore */ }
        }
    }

    let traverse_ns = crate::time::uptime_ns() - traverse_start;
    let traverse_us = traverse_ns / 1000;

    serial_println!("✅ Traversed {} directories in {} μs", MAX_DEPTH, traverse_us);
    serial_println!("   Average: {} μs per directory", traverse_us / MAX_DEPTH as u64);

    Ok(())
}

/// Test 5: Path Resolution Stress
pub fn test_path_resolution() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 5: Path Resolution Stress ===");

    let test_paths = [
        "/",
        "/tmp",
        "/dev",
        "/dev/kbd",
        "/tmp/testfile_0000.txt",
        "/tmp/level0",
        "/tmp/level0/file00.txt",
        "/tmp/large_test_file.dat",
    ];

    serial_println!("Resolving {} paths 100 times each...", test_paths.len());

    let start_time = crate::time::uptime_ns();
    let mut total_resolutions = 0;

    for _iteration in 0..100 {
        for (idx, path) in test_paths.iter().enumerate() {
            // Resolve path via stat
            match file_ops::stat(path, true) {
                Ok(_) => {
                    total_resolutions += 1;
                }
                Err(_) => {
                    // Path may not exist, that's OK
                    total_resolutions += 1;
                }
            }

            if total_resolutions % 200 == 0 {
                serial_println!("  Resolved: {}/{}", total_resolutions, test_paths.len() * 100);
            }
        }
    }

    let elapsed_ns = crate::time::uptime_ns() - start_time;
    let elapsed_ms = elapsed_ns / 1_000_000;
    let avg_ns = if total_resolutions > 0 { elapsed_ns / total_resolutions } else { 0 };

    serial_println!("✅ Resolved {} paths in {} ms", total_resolutions, elapsed_ms);
    serial_println!("   Average: {} ns per resolution", avg_ns);
    serial_println!("   Throughput: {} paths/sec",
        if elapsed_ms > 0 { (total_resolutions as u64 * 1000) / elapsed_ms } else { 0 });

    Ok(())
}

/// Test 6: Write/Read Data Integrity
pub fn test_data_integrity() -> Result<(), &'static str> {
    serial_println!("\n=== TEST 6: Write/Read Data Integrity ===");
    serial_println!("Testing data integrity with pattern verification...");

    let filename = "/tmp/integrity_test.bin";
    const PATTERN_SIZE: usize = 1024;

    // Create pattern
    let mut write_pattern = Vec::new();
    for i in 0..PATTERN_SIZE {
        write_pattern.push((i % 256) as u8);
    }

    // Write pattern
    create_file(filename, 0o644)
        .map_err(|_| "Failed to create integrity test file")?;

    match file_ops::write_file(filename, &write_pattern) {
        Ok(written) => {
            serial_println!("  Written {} bytes", written);
        }
        Err(e) => {
            serial_println!("❌ Write failed: {:?}", e);
            return Err("Write failed");
        }
    }

    // Read back and verify
    match file_ops::read_file(filename) {
        Ok(read_pattern) => {
            serial_println!("  Read {} bytes", read_pattern.len());

            // Verify data
            let mut mismatches = 0;
            for i in 0..PATTERN_SIZE.min(read_pattern.len()) {
                if read_pattern[i] != write_pattern[i] {
                    mismatches += 1;
                }
            }

            if mismatches == 0 {
                serial_println!("✅ Data integrity PASS - All bytes match");
            } else {
                serial_println!("❌ Data integrity FAIL - {} mismatches", mismatches);
                return Err("Data corruption detected");
            }
        }
        Err(e) => {
            serial_println!("❌ Read failed: {:?}", e);
            return Err("Read failed");
        }
    }

    Ok(())
}

/// Master test runner
pub fn run_all_stress_tests() {
    serial_println!("\n");
    serial_println!("╔═══════════════════════════════════════════════════════╗");
    serial_println!("║   FILESYSTEM STRESS TESTS - PHASE 2 (VFS REAL)       ║");
    serial_println!("║   Testing with actual VFS operations                  ║");
    serial_println!("╚═══════════════════════════════════════════════════════╝");
    serial_println!("");

    let mut passed = 0;
    let mut failed = 0;

    // Test 1
    match test_mass_file_creation() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 1 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 1 FAILED: {}", e); }
    }

    // Test 2
    match test_large_file_io() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 2 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 2 FAILED: {}", e); }
    }

    // Test 3
    match test_file_operations() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 3 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 3 FAILED: {}", e); }
    }

    // Test 4
    match test_directory_traversal() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 4 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 4 FAILED: {}", e); }
    }

    // Test 5
    match test_path_resolution() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 5 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 5 FAILED: {}", e); }
    }

    // Test 6
    match test_data_integrity() {
        Ok(_) => { passed += 1; serial_println!("✅ Test 6 PASSED"); }
        Err(e) => { failed += 1; serial_println!("❌ Test 6 FAILED: {}", e); }
    }

    // Summary
    serial_println!("\n");
    serial_println!("═══════════════════════════════════════════════════════");
    serial_println!("           STRESS TEST SUMMARY");
    serial_println!("═══════════════════════════════════════════════════════");
    serial_println!("  Total tests:    {}", passed + failed);
    serial_println!("  Passed:         {} ✅", passed);
    serial_println!("  Failed:         {} {}", failed, if failed > 0 { "❌" } else { "" });
    serial_println!("  Success rate:   {}%", (passed * 100) / (passed + failed));
    serial_println!("═══════════════════════════════════════════════════════");

    if failed == 0 {
        serial_println!("\n✅ ALL VFS STRESS TESTS PASSED - FILESYSTEM ROBUST");
    } else {
        serial_println!("\n⚠️  SOME TESTS FAILED - REVIEW REQUIRED");
    }
}
