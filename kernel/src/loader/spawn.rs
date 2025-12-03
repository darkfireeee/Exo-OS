//! Process Spawning
//! 
//! High-level API for creating and launching user processes.

use alloc::string::String;
use alloc::vec::Vec;
use crate::loader::{load_elf, LoadedElf, ElfError, build_auxv, ProcessStack, LoadedSegment};
use crate::memory::{VirtualAddress, MemoryError, UserAddressSpace, UserPageFlags, PAGE_SIZE};
use crate::arch::x86_64::usermode::{UserContext, jump_to_usermode};
use crate::scheduler::thread::{Thread, ThreadId, alloc_thread_id};

/// Default user stack size (2MB for testing)
pub const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024;

/// Default user stack top address
pub const USER_STACK_TOP: usize = 0x7FFF_FFFF_F000;

/// Default kernel stack size for user threads (16KB)
pub const DEFAULT_KERNEL_STACK_SIZE: usize = 16 * 1024;

/// Process spawn error
#[derive(Debug)]
pub enum SpawnError {
    /// ELF loading failed
    ElfError(ElfError),
    /// Memory allocation failed
    MemoryError(MemoryError),
    /// File not found
    FileNotFound,
    /// I/O error reading file
    IoError,
    /// Invalid executable
    InvalidExecutable,
}

impl From<ElfError> for SpawnError {
    fn from(e: ElfError) -> Self {
        SpawnError::ElfError(e)
    }
}

impl From<MemoryError> for SpawnError {
    fn from(e: MemoryError) -> Self {
        SpawnError::MemoryError(e)
    }
}

/// Process ID
pub type Pid = u32;

/// Spawn parameters
pub struct SpawnParams<'a> {
    /// Arguments (argv)
    pub args: &'a [&'a str],
    /// Environment variables
    pub env: &'a [&'a str],
    /// Working directory
    pub cwd: Option<&'a str>,
    /// Stack size (default 8MB)
    pub stack_size: usize,
}

impl<'a> Default for SpawnParams<'a> {
    fn default() -> Self {
        Self {
            args: &[],
            env: &[],
            cwd: None,
            stack_size: DEFAULT_STACK_SIZE,
        }
    }
}

/// Spawn a new user process from an ELF file
/// 
/// This function:
/// 1. Loads the ELF executable
/// 2. Creates a new address space
/// 3. Maps the ELF segments
/// 4. Sets up the user stack with args, env, auxv
/// 5. Creates a new thread with user context
/// 
/// The new process will start executing when scheduled.
pub fn spawn_process(
    elf_data: &[u8],
    params: &SpawnParams,
) -> Result<SpawnedProcess, SpawnError> {
    // Load ELF
    let loaded = load_elf(elf_data, None)?;
    
    log::info!("Loaded ELF: entry={:#x}, {} segments", 
        loaded.entry_point.value(), 
        loaded.segments.len()
    );
    
    // Check if needs interpreter (dynamic linking)
    if loaded.needs_interpreter() {
        log::warn!("Dynamic executables not yet supported, interpreter: {:?}", 
            loaded.interpreter);
        // For now, we'll try to run it anyway as PIE
    }
    
    // Calculate memory layout
    let base = loaded.base_address();
    let end = loaded.end_address();
    let code_size = end.value() - base.value();
    
    // Stack layout
    let stack_top = VirtualAddress::new(USER_STACK_TOP);
    let stack_bottom = VirtualAddress::new(USER_STACK_TOP - params.stack_size);
    
    log::info!("Process memory: code [{:#x}-{:#x}], stack [{:#x}-{:#x}]",
        base.value(), end.value(),
        stack_bottom.value(), stack_top.value()
    );
    
    // Build auxiliary vector
    // For random bytes, we'd normally use /dev/urandom
    let random_ptr = VirtualAddress::new(stack_top.value() - 16);
    let auxv = build_auxv(&loaded, None, random_ptr);
    
    // Setup stack with args, env, auxv
    let stack = ProcessStack::setup(
        stack_top,
        params.args,
        params.env,
        &auxv,
    );
    
    // Create user context
    let mut context = UserContext::new(loaded.entry_point, stack.sp);
    
    // Set argc, argv, envp according to x86_64 ABI
    let argc = params.args.len() as u64;
    context.set_args(argc, stack.argv_ptr.as_u64(), stack.envp_ptr.as_u64());
    
    Ok(SpawnedProcess {
        loaded,
        context,
        stack_top,
        stack_size: params.stack_size,
    })
}

/// A spawned process ready to execute
pub struct SpawnedProcess {
    /// Loaded ELF information
    pub loaded: LoadedElf,
    /// Initial CPU context
    pub context: UserContext,
    /// Top of user stack
    pub stack_top: VirtualAddress,
    /// Stack size
    pub stack_size: usize,
}

impl SpawnedProcess {
    /// Get the entry point address
    pub fn entry_point(&self) -> VirtualAddress {
        self.loaded.entry_point
    }
    
    /// Get the user context (for modifying before launch)
    pub fn context_mut(&mut self) -> &mut UserContext {
        &mut self.context
    }
    
    /// Launch the process (transfers control to user mode)
    /// 
    /// # Safety
    /// 
    /// This function never returns. It:
    /// - Switches to the process's address space
    /// - Jumps to user mode (Ring 3)
    /// 
    /// The caller must ensure:
    /// - Page tables are properly set up
    /// - TSS RSP0 is set for kernel re-entry
    pub unsafe fn launch(self) -> ! {
        log::info!("Launching user process at {:#x}", self.loaded.entry_point.value());
        
        // Jump to user mode!
        jump_to_usermode(&self.context);
    }
}

/// Spawn a user process with page tables and add it to the scheduler
/// 
/// This is the full integration that:
/// 1. Loads the ELF executable
/// 2. Creates a user address space with page tables
/// 3. Maps all ELF segments
/// 4. Maps the user stack
/// 5. Creates a Thread with user context
/// 6. Adds the thread to the scheduler
/// 
/// Returns the thread ID of the new process.
pub fn spawn_and_schedule(
    elf_data: &[u8],
    name: &str,
    params: &SpawnParams,
) -> Result<ThreadId, SpawnError> {
    log::info!("spawn_and_schedule: starting for '{}'", name);
    
    // Load ELF
    let loaded = load_elf(elf_data, None)?;
    
    log::info!("ELF loaded: entry={:#x}, {} segments", 
        loaded.entry_point.value(), 
        loaded.segments.len());
    
    // Create user address space with page tables
    let mut address_space = UserAddressSpace::new()?;
    log::info!("Created user address space, CR3={:#x}", address_space.cr3());
    
    // Map ELF segments
    for (i, segment) in loaded.segments.iter().enumerate() {
        let flags = segment_flags_to_page_flags(&segment.flags);
        
        log::debug!("Mapping segment {}: vaddr={:#x}, size={:#x}, flags={:?}",
            i, segment.vaddr.value(), segment.mem_size, segment.flags);
        
        // Get segment data from ELF
        let data_start = segment.data_offset;
        let data_end = data_start + segment.file_size;
        let segment_data = if data_end <= elf_data.len() {
            &elf_data[data_start..data_end]
        } else {
            &[]
        };
        
        // Map with data
        address_space.map_segment_data(
            segment.vaddr,
            segment_data,
            segment.mem_size,
            flags,
        )?;
    }
    
    // Map user stack
    let stack_top = VirtualAddress::new(USER_STACK_TOP);
    let stack_bottom = VirtualAddress::new(USER_STACK_TOP - params.stack_size);
    
    log::info!("Mapping user stack: {:#x}-{:#x}", stack_bottom.value(), stack_top.value());
    
    address_space.map_range(
        stack_bottom,
        params.stack_size,
        UserPageFlags::user_stack(),
    )?;
    
    // Allocate a thread ID
    let tid = alloc_thread_id();
    
    // Create the user thread
    // Note: The thread will use user_mode_trampoline which calls jump_to_usermode
    let thread = Thread::new_user(
        tid,
        name,
        loaded.entry_point,
        stack_top,
        DEFAULT_KERNEL_STACK_SIZE,
    );
    
    log::info!(
        "Created user thread {} '{}': entry={:#x}, stack={:#x}",
        tid,
        name,
        loaded.entry_point.value(),
        stack_top.value()
    );
    
    // Store the address space in the thread
    // TODO: Add address_space field to Thread
    // For now, we leak it so it stays valid
    let _address_space = alloc::boxed::Box::leak(alloc::boxed::Box::new(address_space));
    
    // Add to scheduler
    crate::scheduler::SCHEDULER.add_thread(thread);
    
    log::info!("User process '{}' (tid={}) added to scheduler", name, tid);
    
    Ok(tid)
}

/// Convert segment flags to page flags
fn segment_flags_to_page_flags(flags: &crate::loader::SegmentFlags) -> UserPageFlags {
    let mut page_flags = UserPageFlags::empty().present().user();
    
    if flags.write {
        page_flags = page_flags.writable();
    }
    
    if !flags.execute {
        page_flags = page_flags.no_execute();
    }
    
    page_flags
}

/// Spawn from a file path (uses VFS)
pub fn spawn_from_path(
    path: &str,
    params: &SpawnParams,
) -> Result<ThreadId, SpawnError> {
    log::info!("spawn_from_path: {}", path);
    
    // TODO: Read file from VFS
    Err(SpawnError::FileNotFound)
}

/// Create a minimal ELF that just calls exit(0)
/// 
/// This is useful for testing the user mode infrastructure.
pub fn create_test_elf() -> Vec<u8> {
    let mut elf = Vec::new();
    
    // ===== ELF Header (64 bytes) =====
    // e_ident
    elf.extend_from_slice(&[0x7F, b'E', b'L', b'F']); // Magic
    elf.push(2);  // ELFCLASS64
    elf.push(1);  // ELFDATA2LSB
    elf.push(1);  // EV_CURRENT
    elf.push(0);  // ELFOSABI_NONE
    elf.extend_from_slice(&[0u8; 8]); // padding
    
    // e_type: ET_EXEC
    elf.extend_from_slice(&2u16.to_le_bytes());
    // e_machine: EM_X86_64
    elf.extend_from_slice(&62u16.to_le_bytes());
    // e_version
    elf.extend_from_slice(&1u32.to_le_bytes());
    // e_entry: 0x400078 (code starts right after headers)
    elf.extend_from_slice(&0x400078u64.to_le_bytes());
    // e_phoff: 64 (program headers start after ELF header)
    elf.extend_from_slice(&64u64.to_le_bytes());
    // e_shoff: 0 (no section headers)
    elf.extend_from_slice(&0u64.to_le_bytes());
    // e_flags
    elf.extend_from_slice(&0u32.to_le_bytes());
    // e_ehsize: 64
    elf.extend_from_slice(&64u16.to_le_bytes());
    // e_phentsize: 56
    elf.extend_from_slice(&56u16.to_le_bytes());
    // e_phnum: 1
    elf.extend_from_slice(&1u16.to_le_bytes());
    // e_shentsize: 0
    elf.extend_from_slice(&0u16.to_le_bytes());
    // e_shnum: 0
    elf.extend_from_slice(&0u16.to_le_bytes());
    // e_shstrndx: 0
    elf.extend_from_slice(&0u16.to_le_bytes());
    
    // ===== Program Header (56 bytes) =====
    // p_type: PT_LOAD
    elf.extend_from_slice(&1u32.to_le_bytes());
    // p_flags: PF_R | PF_X (readable, executable)
    elf.extend_from_slice(&5u32.to_le_bytes());
    // p_offset: 0
    elf.extend_from_slice(&0u64.to_le_bytes());
    // p_vaddr: 0x400000
    elf.extend_from_slice(&0x400000u64.to_le_bytes());
    // p_paddr: 0x400000
    elf.extend_from_slice(&0x400000u64.to_le_bytes());
    // p_filesz: will be set later
    let filesz_offset = elf.len();
    elf.extend_from_slice(&0u64.to_le_bytes());
    // p_memsz: will be set later
    let memsz_offset = elf.len();
    elf.extend_from_slice(&0u64.to_le_bytes());
    // p_align: 0x1000
    elf.extend_from_slice(&0x1000u64.to_le_bytes());
    
    // ===== Code starts at offset 0x78 (120 bytes) =====
    // Pad to offset 0x78
    while elf.len() < 0x78 {
        elf.push(0);
    }
    
    // Code: sys_exit(0)
    // mov rax, 60     ; syscall number for exit
    elf.extend_from_slice(&[0x48, 0xc7, 0xc0, 0x3c, 0x00, 0x00, 0x00]);
    // xor rdi, rdi    ; exit code 0
    elf.extend_from_slice(&[0x48, 0x31, 0xff]);
    // syscall
    elf.extend_from_slice(&[0x0f, 0x05]);
    // hlt (fallback, should never reach)
    elf.push(0xf4);
    
    // Update sizes
    let total_size = elf.len() as u64;
    elf[filesz_offset..filesz_offset + 8].copy_from_slice(&total_size.to_le_bytes());
    elf[memsz_offset..memsz_offset + 8].copy_from_slice(&total_size.to_le_bytes());
    
    log::debug!("Created test ELF: {} bytes", elf.len());
    
    elf
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_spawn_params_default() {
        let params = SpawnParams::default();
        assert_eq!(params.stack_size, DEFAULT_STACK_SIZE);
        assert!(params.args.is_empty());
        assert!(params.env.is_empty());
    }
}
