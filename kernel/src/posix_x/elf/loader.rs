//! ELF Binary Loader - Phase 10
//!
//! Handles loading of ELF64 binaries into memory for execution.

use crate::memory::{MemoryError, MemoryResult};
use crate::posix_x::elf::parser::{
    self, Elf64Header, Elf64ProgramHeader, PF_R, PF_W, PF_X, PT_LOAD,
};
use crate::posix_x::vfs_posix::file_ops;
use alloc::string::String;
use alloc::vec::Vec;

/// Loaded ELF binary info
#[derive(Debug)]
pub struct LoadedElf {
    pub entry_point: u64,
    pub stack_top: u64,
}

/// Load an ELF binary from a file path
pub fn load_elf_binary(path: &str, args: &[String], env: &[String]) -> MemoryResult<LoadedElf> {
    log::info!("Loading ELF binary: {}", path);

    // 1. Read file from VFS
    let file_data = file_ops::read_file(path).map_err(|_| MemoryError::NotFound)?;

    // 2. Parse ELF Header
    let header = parser::parse_elf_header(&file_data).map_err(|_| MemoryError::InvalidAddress)?; // Map ElfError to MemoryError

    // 3. Get Program Headers
    let program_headers =
        parser::get_program_headers(&file_data, header).map_err(|_| MemoryError::InvalidAddress)?;

    // 4. Load Segments
    for ph in program_headers {
        if ph.p_type == PT_LOAD {
            load_segment(&file_data, ph)?;
        }
    }

    // 5. Setup Stack
    let stack_top = setup_stack(args, env)?;

    Ok(LoadedElf {
        entry_point: header.e_entry,
        stack_top,
    })
}

/// Load a single ELF segment into memory
fn load_segment(file_data: &[u8], ph: &Elf64ProgramHeader) -> MemoryResult<()> {
    log::debug!(
        "Loading segment: vaddr={:#x}, size={:#x}, flags={:#x}",
        ph.p_vaddr,
        ph.p_memsz,
        ph.p_flags
    );

    // Get segment data from file
    let segment_data =
        parser::get_segment_data(file_data, ph).map_err(|_| MemoryError::InvalidAddress)?;

    // TODO: Real memory mapping
    // In a real kernel, we would:
    // 1. Allocate virtual pages at ph.p_vaddr
    // 2. Map them to physical frames
    // 3. Copy segment_data into the mapped memory
    // 4. Zero out remaining BSS (p_memsz - p_filesz)
    // 5. Set page permissions based on ph.p_flags (R/W/X)

    // For now, we just log that we would do this
    // This allows us to test the parsing and loading logic without breaking
    // the current running kernel (since we don't have separate address spaces yet)

    Ok(())
}

/// Setup user stack with arguments and environment variables
fn setup_stack(args: &[String], env: &[String]) -> MemoryResult<u64> {
    // TODO: Allocate stack pages
    // For now, return a dummy stack pointer
    let stack_top = 0x7FFF_FFFF_F000;

    log::debug!(
        "Setting up stack at {:#x} with {} args and {} env vars",
        stack_top,
        args.len(),
        env.len()
    );

    // In a real implementation:
    // 1. Push strings to stack
    // 2. Push pointers to strings
    // 3. Push argc

    Ok(stack_top)
}
