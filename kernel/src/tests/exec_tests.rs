//! Tests for exec() implementation
//!
//! Validates that load_elf_binary() correctly:
//! - Reads files from VFS
//! - Parses ELF headers
//! - Maps segments into memory
//! - Sets up stack with System V ABI

use crate::posix_x::elf::loader::load_elf_binary;
use crate::fs::vfs;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

/// Test that load_elf_binary() can load a simple ELF file
pub fn test_load_elf_basic() {
    log::info!("[TEST] test_load_elf_basic: starting");

    // First, we need a valid ELF binary in VFS
    // For now, create a minimal test ELF structure
    let test_elf = create_minimal_elf();
    
    log::info!("[TEST] Created minimal ELF: {} bytes", test_elf.len());
    
    // Write to VFS
    let test_path = "/bin/test_exec";
    match vfs::write_file(test_path, &test_elf) {
        Ok(_) => log::info!("[TEST] Wrote test ELF to VFS: {} ({} bytes)", test_path, test_elf.len()),
        Err(e) => {
            log::error!("[TEST] Failed to write test ELF: {:?}", e);
            panic!("Test setup failed");
        }
    }

    // Now try to load it
    let args = vec![String::from("test_exec")];
    let env = vec![];

    match load_elf_binary(test_path, &args, &env) {
        Ok(loaded) => {
            log::info!(
                "[TEST] Successfully loaded ELF: entry={:#x}, stack={:#x}",
                loaded.entry_point,
                loaded.stack_top
            );

            // Validate results
            assert!(loaded.entry_point != 0, "Entry point should not be zero");
            assert!(loaded.stack_top >= 0x7FFF_0000_0000, "Stack should be in high memory");

            log::info!("[TEST] ✅ test_load_elf_basic PASSED");
        }
        Err(e) => {
            log::error!("[TEST] ❌ Failed to load ELF: {:?}", e);
            panic!("load_elf_binary() failed");
        }
    }
}

/// Test stack setup with arguments
pub fn test_stack_setup_with_args() {
    log::info!("[TEST] test_stack_setup_with_args: starting");

    let test_elf = create_minimal_elf();
    let test_path = "/bin/test_args";
    
    vfs::write_file(test_path, &test_elf).expect("Failed to write test ELF");

    let args = vec![
        String::from("program"),
        String::from("arg1"),
        String::from("arg2"),
        String::from("--flag=value"),
    ];
    let env = vec![
        String::from("PATH=/bin:/usr/bin"),
        String::from("HOME=/home/user"),
    ];

    match load_elf_binary(test_path, &args, &env) {
        Ok(loaded) => {
            log::info!(
                "[TEST] Stack setup with {} args, {} env vars: stack={:#x}",
                args.len(),
                env.len(),
                loaded.stack_top
            );

            // Stack should be 16-byte aligned (System V ABI requirement)
            assert_eq!(
                loaded.stack_top & 0xF,
                0,
                "Stack pointer must be 16-byte aligned"
            );

            log::info!("[TEST] ✅ test_stack_setup_with_args PASSED");
        }
        Err(e) => {
            log::error!("[TEST] ❌ Failed: {:?}", e);
            panic!("Stack setup test failed");
        }
    }
}

/// Test loading non-existent file
pub fn test_load_nonexistent_file() {
    log::info!("[TEST] test_load_nonexistent_file: starting");

    let args = vec![String::from("nonexistent")];
    let env = vec![];

    match load_elf_binary("/bin/nonexistent_file_12345", &args, &env) {
        Ok(_) => {
            log::error!("[TEST] ❌ Should have failed for non-existent file");
            panic!("Expected error for non-existent file");
        }
        Err(e) => {
            log::info!("[TEST] Correctly failed with error: {:?}", e);
            log::info!("[TEST] ✅ test_load_nonexistent_file PASSED");
        }
    }
}

/// Create a minimal valid ELF64 binary
/// This is a tiny "hello world" that just exits immediately
fn create_minimal_elf() -> Vec<u8> {
    // ELF64 header (64 bytes)
    let mut elf = vec![0u8; 64];

    // Magic number
    elf[0..4].copy_from_slice(b"\x7fELF");

    // Class: 64-bit (2)
    elf[4] = 2;

    // Data: Little-endian (1)
    elf[5] = 1;

    // Version (1)
    elf[6] = 1;

    // OS/ABI: System V (0)
    elf[7] = 0;

    // Type: Executable (2)
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());

    // Machine: x86-64 (0x3E)
    elf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());

    // Version (1)
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());

    // Entry point: 0x40000000 (1GB - user space, avoiding huge pages)
    elf[24..32].copy_from_slice(&0x40000000u64.to_le_bytes());

    // Program header offset: 64 (right after ELF header)
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());

    // Section header offset: 0 (none)
    elf[40..48].copy_from_slice(&0u64.to_le_bytes());

    // Flags: 0
    elf[48..52].copy_from_slice(&0u32.to_le_bytes());

    // ELF header size: 64
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());

    // Program header entry size: 56
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());

    // Program header count: 1
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());

    // Section header entry size: 0
    elf[58..60].copy_from_slice(&0u16.to_le_bytes());

    // Section header count: 0
    elf[60..62].copy_from_slice(&0u16.to_le_bytes());

    // Section name string table index: 0
    elf[62..64].copy_from_slice(&0u16.to_le_bytes());

    // Program header (56 bytes) - PT_LOAD segment
    let mut phdr = vec![0u8; 56];

    // Type: PT_LOAD (1)
    phdr[0..4].copy_from_slice(&1u32.to_le_bytes());

    // Flags: R+X (5)
    phdr[4..8].copy_from_slice(&5u32.to_le_bytes());

    // Offset in file: 0x1000 (4KB)
    phdr[8..16].copy_from_slice(&0x1000u64.to_le_bytes());

    // Virtual address: 0x40000000
    phdr[16..24].copy_from_slice(&0x40000000u64.to_le_bytes());

    // Physical address: 0x40000000
    phdr[24..32].copy_from_slice(&0x40000000u64.to_le_bytes());

    // File size: 16 bytes (minimal code)
    phdr[32..40].copy_from_slice(&16u64.to_le_bytes());

    // Memory size: 4096 bytes (one page)
    phdr[40..48].copy_from_slice(&4096u64.to_le_bytes());

    // Alignment: 0x1000 (4KB)
    phdr[48..56].copy_from_slice(&0x1000u64.to_le_bytes());

    elf.extend_from_slice(&phdr);

    // Pad to offset 0x1000
    while elf.len() < 0x1000 {
        elf.push(0);
    }

    // Add minimal x86-64 code: mov rax, 60; xor rdi, rdi; syscall (exit(0))
    let code = [
        0x48, 0xc7, 0xc0, 0x3c, 0x00, 0x00, 0x00, // mov rax, 60
        0x48, 0x31, 0xff, // xor rdi, rdi
        0x0f, 0x05, // syscall
        0xf4, 0xf4, 0xf4, 0xf4, // hlt hlt hlt hlt (padding to 16 bytes)
    ];
    elf.extend_from_slice(&code);

    elf
}

/// Run all exec tests
pub fn run_all_exec_tests() {
    log::info!("========================================");
    log::info!("   EXEC IMPLEMENTATION TESTS");
    log::info!("========================================");

    test_load_elf_basic();
    test_stack_setup_with_args();
    test_load_nonexistent_file();

    log::info!("========================================");
    log::info!("   ✅ ALL EXEC TESTS PASSED");
    log::info!("========================================");
}
