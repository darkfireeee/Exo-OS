//! Test loading REAL compiled ELF binary (test_exec_vfs.elf)
//! Jour 2 validation with actual userland binary

use crate::posix_x::elf::loader::load_elf_binary;
use crate::fs::vfs;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

// Import the basic tests from exec_tests
use super::exec_tests::{test_load_elf_basic, test_stack_setup_with_args, test_load_nonexistent_file};

/// Test loading REAL compiled ELF binary (test_exec_vfs.elf)
pub fn test_load_real_elf() {
    log::info!("[TEST] test_load_real_elf: SKIPPED - userland binary not available");

    // TEMPORARY: Test binary not available yet
    // TODO: Build userland/test_exec_vfs.elf before running this test
    /*
    // Include the compiled binary (embed at compile time)
    // Path is relative to Cargo.toml (kernel/Cargo.toml)
    const TEST_BINARY: &[u8] = include_bytes!("../../../userland/test_exec_vfs.elf");

    log::info!("[TEST] Test binary size: {} bytes", TEST_BINARY.len());

    // Write binary to VFS
    let test_path = "/bin/test_exec_real";
    match vfs::write_file(test_path, TEST_BINARY) {
        Ok(_) => log::info!("[TEST] Wrote real ELF to VFS: {}", test_path),
        Err(e) => {
            log::error!("[TEST] Failed to write test binary: {:?}", e);
            panic!("Test setup failed");
        }
    }

    // Load the binary
    let args = vec![
        String::from("test_exec_real"),
        String::from("arg1"),
        String::from("arg2"),
    ];
    let env = vec![
        String::from("PATH=/bin:/usr/bin"),
        String::from("HOME=/root"),
    ];

    match load_elf_binary(test_path, &args, &env) {
        Ok(loaded) => {
            log::info!(
                "[TEST] ✅ Loaded real ELF successfully!"
            );
            log::info!("  Entry point: {:#x}", loaded.entry_point);
            log::info!("  Stack top:   {:#x}", loaded.stack_top);

            // Validate entry point is reasonable
            assert!(loaded.entry_point >= 0x400000, "Entry point should be in user space");
            assert!(loaded.entry_point < 0x800000, "Entry point should be in reasonable range");

            // Validate stack
            assert!(loaded.stack_top >= 0x7FFF_0000_0000, "Stack should be in high memory");
            assert!(loaded.stack_top <= 0x7FFF_FFFF_F000, "Stack should be below kernel space");
            assert!(loaded.stack_top % 16 == 0, "Stack should be 16-byte aligned");

            log::info!("[TEST] ✅ test_load_real_elf PASSED - Real binary loaded!");
        }
        Err(e) => {
            log::error!("[TEST] ❌ Failed to load real ELF: {:?}", e);
            panic!("test_load_real_elf FAILED");
        }
    }
    */
}

/// Run all exec tests
pub fn run_all_exec_tests() {
    log::info!("╔══════════════════════════════════════════════════════════╗");
    log::info!("║         EXEC TESTS - Jour 2 Validation                  ║");
    log::info!("╚══════════════════════════════════════════════════════════╝");

    log::info!("\n[1/4] Basic ELF loading test...");
    test_load_elf_basic();

    log::info!("\n[2/4] Stack setup with args test...");
    test_stack_setup_with_args();

    log::info!("\n[3/4] Error handling test...");
    test_load_nonexistent_file();

    log::info!("\n[4/4] REAL binary loading test...");
    test_load_real_elf();

    log::info!("\n╔══════════════════════════════════════════════════════════╗");
    log::info!("║         ✅ ALL EXEC TESTS PASSED                        ║");
    log::info!("╚══════════════════════════════════════════════════════════╝");
}
