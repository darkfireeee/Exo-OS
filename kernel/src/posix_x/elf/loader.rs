//! ELF Binary Loader - Phase 10
//!
//! Handles loading of ELF64 binaries into memory for execution.

use crate::memory::{MemoryError, MemoryResult};
use crate::posix_x::elf::parser::{
    self, Elf64Header, Elf64ProgramHeader, PF_R, PF_W, PF_X, PT_LOAD,
};
// ⏸️ Phase 1b: use crate::posix_x::vfs_posix::file_ops;
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
    log::info!("[EXEC] Loading ELF binary: {}", path);

    // 1. Read file from VFS - REAL implementation
    let file_data = crate::fs::vfs::read_file(path)
        .map_err(|e| {
            log::error!("[EXEC] Failed to read file {}: {:?}", path, e);
            MemoryError::NotFound
        })?;

    log::debug!("[EXEC] Read {} bytes from {}", file_data.len(), path);

    // 2. Parse ELF Header - REAL validation
    let header = parser::parse_elf_header(&file_data)
        .map_err(|e| {
            log::error!("[EXEC] Invalid ELF header: {:?}", e);
            MemoryError::InvalidAddress
        })?;

    log::info!("[EXEC] ELF entry point: {:#x}", header.e_entry);

    // 3. Get Program Headers
    let program_headers = parser::get_program_headers(&file_data, &header)
        .map_err(|e| {
            log::error!("[EXEC] Failed to parse program headers: {:?}", e);
            MemoryError::InvalidAddress
        })?;

    log::debug!("[EXEC] Found {} program headers", program_headers.len());

    // 4. Load PT_LOAD segments into memory
    let mut segment_count = 0;
    for ph in program_headers {
        if ph.p_type == PT_LOAD {
            load_segment(&file_data, &ph)?;
            segment_count += 1;
            log::debug!(
                "[EXEC]   Loaded segment: vaddr={:#x}, memsz={:#x}, filesz={:#x}, flags={:#x}",
                ph.p_vaddr, ph.p_memsz, ph.p_filesz, ph.p_flags
            );
        }
    }

    if segment_count == 0 {
        log::error!("[EXEC] No loadable segments found");
        return Err(MemoryError::InvalidAddress);
    }

    log::info!("[EXEC] Loaded {} segments successfully", segment_count);

    // 5. Setup user stack with args and env
    let stack_top = setup_stack(args, env)?;

    log::info!("[EXEC] Stack setup complete at {:#x}", stack_top);

    Ok(LoadedElf {
        entry_point: header.e_entry,
        stack_top,
    })
}

/// Load a single ELF segment into memory - REAL IMPLEMENTATION
fn load_segment(file_data: &[u8], ph: &Elf64ProgramHeader) -> MemoryResult<()> {
    use crate::memory::{VirtualAddress, PageProtection};
    use crate::memory::mmap::{mmap, MmapFlags};

    log::debug!(
        "[EXEC] Loading segment: vaddr={:#x}, memsz={:#x}, filesz={:#x}, flags={:#x}",
        ph.p_vaddr,
        ph.p_memsz,
        ph.p_filesz,
        ph.p_flags
    );

    if ph.p_memsz == 0 {
        log::debug!("[EXEC] Skipping zero-size segment");
        return Ok(());
    }

    // Get segment data from file
    let segment_data = parser::get_segment_data(file_data, ph)
        .map_err(|e| {
            log::error!("[EXEC] Failed to extract segment data: {:?}", e);
            MemoryError::InvalidAddress
        })?;

    // Calculate page-aligned addresses
    const PAGE_SIZE: usize = 4096;
    let vaddr = ph.p_vaddr as usize;
    let memsz = ph.p_memsz as usize;
    let filesz = ph.p_filesz as usize;

    let aligned_start = vaddr & !(PAGE_SIZE - 1);
    let aligned_end = (vaddr + memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let aligned_size = aligned_end - aligned_start;

    log::debug!(
        "[EXEC]   Mapping {:#x}-{:#x} ({} pages)",
        aligned_start,
        aligned_end,
        aligned_size / PAGE_SIZE
    );

    // Determine page protection based on segment flags
    let mut prot = 0u32;
    if ph.p_flags & PF_R != 0 {
        prot |= 0x1; // PROT_READ
    }
    if ph.p_flags & PF_W != 0 {
        prot |= 0x2; // PROT_WRITE
    }
    if ph.p_flags & PF_X != 0 {
        prot |= 0x4; // PROT_EXEC
    }

    let page_prot = PageProtection::from_prot(prot);

    // Map anonymous memory (MAP_PRIVATE | MAP_ANONYMOUS)
    let mmap_flags = MmapFlags::new(0x22);

    let mapped_addr = mmap(
        Some(VirtualAddress::new(aligned_start)),
        aligned_size,
        page_prot,
        mmap_flags,
        None,
        0,
    )?;

    log::debug!("[EXEC]   Mapped at {:#x}", mapped_addr.value());

    // Copy segment file data to mapped memory
    if filesz > 0 {
        let page_offset = vaddr - aligned_start;
        unsafe {
            let dest = (mapped_addr.value() + page_offset) as *mut u8;
            core::ptr::copy_nonoverlapping(
                segment_data.as_ptr(),
                dest,
                filesz
            );
        }
        log::debug!("[EXEC]   Copied {} bytes of segment data", filesz);
    }

    // Zero BSS section (memsz - filesz)
    if memsz > filesz {
        let bss_size = memsz - filesz;
        let bss_offset = (vaddr + filesz) - aligned_start;
        unsafe {
            let dest = (mapped_addr.value() + bss_offset) as *mut u8;
            core::ptr::write_bytes(dest, 0, bss_size);
        }
        log::debug!("[EXEC]   Zeroed {} bytes of BSS", bss_size);
    }

    // TODO: Record memory region in current process
    // This requires access to the current process's memory region list

    Ok(())
}

/// Setup user stack with arguments and environment variables - REAL IMPLEMENTATION
fn setup_stack(args: &[String], env: &[String]) -> MemoryResult<u64> {
    use crate::memory::{VirtualAddress, PageProtection};
    use crate::memory::mmap::{mmap, MmapFlags};

    log::debug!(
        "[EXEC] Setting up stack with {} args and {} env vars",
        args.len(),
        env.len()
    );

    // Allocate 2MB stack (standard Linux size)
    const STACK_SIZE: usize = 0x200000; // 2MB
    const STACK_TOP: usize = 0x7FFF_FFFF_F000;
    let stack_bottom = STACK_TOP - STACK_SIZE;

    // Map stack pages (MAP_PRIVATE | MAP_ANONYMOUS)
    let mmap_flags = MmapFlags::new(0x22);

    let _stack_addr = mmap(
        Some(VirtualAddress::new(stack_bottom)),
        STACK_SIZE,
        PageProtection::READ_WRITE,
        mmap_flags,
        None,
        0,
    )?;

    log::debug!("[EXEC] Stack allocated: {:#x}-{:#x}", stack_bottom, STACK_TOP);

    // Setup stack according to System V ABI for x86_64
    // Layout from high to low:
    // - argument strings
    // - environment strings  
    // - NULL
    // - env pointers
    // - NULL
    // - argv pointers
    // - argc

    let mut sp = STACK_TOP;

    // Helper: align down to 8 bytes
    let align_down = |addr: usize| addr & !0x7;

    // 1. Push argument strings and collect their addresses
    let mut arg_addrs = Vec::new();
    for arg in args.iter().rev() {
        let bytes = arg.as_bytes();
        sp -= bytes.len() + 1; // +1 for null terminator
        sp = align_down(sp);

        unsafe {
            let dest = sp as *mut u8;
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
            *dest.add(bytes.len()) = 0; // null terminator
        }

        arg_addrs.push(sp);
    }
    arg_addrs.reverse();

    log::debug!("[EXEC]   Pushed {} argument strings", args.len());

    // 2. Push environment strings and collect their addresses
    let mut env_addrs = Vec::new();
    for var in env.iter().rev() {
        let bytes = var.as_bytes();
        sp -= bytes.len() + 1;
        sp = align_down(sp);

        unsafe {
            let dest = sp as *mut u8;
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
            *dest.add(bytes.len()) = 0;
        }

        env_addrs.push(sp);
    }
    env_addrs.reverse();

    log::debug!("[EXEC]   Pushed {} environment strings", env.len());

    // 3. Align stack to 16 bytes (required by System V ABI before any function call)
    sp &= !0xF;

    // 4. Push auxiliary vectors (AT_NULL terminator)
    sp -= 16;
    unsafe {
        *(sp as *mut u64) = 0; // AT_NULL type
        *((sp + 8) as *mut u64) = 0; // AT_NULL value
    }

    // 5. Push NULL terminator for environment
    sp -= 8;
    unsafe {
        *(sp as *mut u64) = 0;
    }

    // 6. Push environment pointers
    for addr in env_addrs.iter().rev() {
        sp -= 8;
        unsafe {
            *(sp as *mut u64) = *addr as u64;
        }
    }

    // 7. Push NULL terminator for argv
    sp -= 8;
    unsafe {
        *(sp as *mut u64) = 0;
    }

    // 8. Push argument pointers
    for addr in arg_addrs.iter().rev() {
        sp -= 8;
        unsafe {
            *(sp as *mut u64) = *addr as u64;
        }
    }

    // 9. Push argc
    sp -= 8;
    unsafe {
        *(sp as *mut u64) = args.len() as u64;
    }

    // Final alignment to 16 bytes (ABI requirement)
    sp &= !0xF;

    log::info!(
        "[EXEC] Stack setup complete: sp={:#x}, argc={}, envc={}",
        sp,
        args.len(),
        env.len()
    );

    Ok(sp as u64)
}
