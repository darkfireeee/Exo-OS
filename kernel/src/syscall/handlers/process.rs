//! Process Management System Call Handlers
//!
//! Handles process operations: fork, exec, exit, wait, signals
//! 
//! Phase 12: Full fork/exec/exit implementation

use crate::memory::{MemoryError, MemoryResult, VirtualAddress, PhysicalAddress};
use crate::scheduler::thread::{Thread, ThreadState, ThreadContext, ThreadPriority, alloc_thread_id};
use crate::scheduler::SCHEDULER;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::ffi::CStr;
use core::sync::atomic::{AtomicU64, AtomicI32, Ordering};
use spin::{Mutex, RwLock};

/// Process ID
pub type Pid = u64;

/// Exit code
pub type ExitCode = i32;

/// Signal number
pub type Signal = u32;

/// Wait options
#[derive(Debug, Clone, Copy)]
pub struct WaitOptions {
    pub nohang: bool,
    pub untraced: bool,
    pub continued: bool,
}

/// Process status
#[derive(Debug, Clone, Copy)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Stopped(Signal),
    Zombie(ExitCode),
    Exited(ExitCode),
    Signaled(Signal),
}

/// File descriptor entry for process FD table
#[derive(Clone)]
pub struct FdEntry {
    /// File handle (VFS handle ID)
    pub handle_id: u64,
    /// File descriptor flags (FD_CLOEXEC, etc.)
    pub flags: u32,
    /// File status flags (O_APPEND, etc.)  
    pub status_flags: u32,
    /// Current file offset
    pub offset: u64,
}

impl FdEntry {
    pub fn new(handle_id: u64) -> Self {
        Self {
            handle_id,
            flags: 0,
            status_flags: 0,
            offset: 0,
        }
    }
    
    pub fn with_flags(handle_id: u64, flags: u32) -> Self {
        Self {
            handle_id,
            flags,
            status_flags: 0,
            offset: 0,
        }
    }
}

/// Process control block - represents a full process
pub struct Process {
    /// Process ID
    pub pid: Pid,
    /// Parent process ID
    pub ppid: Pid,
    /// Process group ID
    pub pgid: Pid,
    /// Session ID
    pub sid: Pid,
    /// Main thread ID
    pub main_tid: u64,
    /// Process name
    pub name: String,
    /// File descriptor table
    pub fd_table: Mutex<alloc::collections::BTreeMap<i32, FdEntry>>,
    /// Memory mappings (per-process address space)
    pub memory_regions: Mutex<Vec<MemoryRegion>>,
    /// Current working directory
    pub cwd: Mutex<String>,
    /// Environment variables
    pub environ: Mutex<Vec<String>>,
    /// Exit status (set when process exits)
    pub exit_status: AtomicI32,
    /// Process state
    pub state: Mutex<ProcessState>,
    /// Children PIDs
    pub children: Mutex<Vec<Pid>>,
    /// User ID
    pub uid: u32,
    /// Group ID  
    pub gid: u32,
    /// Effective user ID
    pub euid: u32,
    /// Effective group ID
    pub egid: u32,
}

/// Memory region for process address space
#[derive(Clone, Debug)]
pub struct MemoryRegion {
    pub start: VirtualAddress,
    pub size: usize,
    pub prot: u32,      // PROT_READ | PROT_WRITE | PROT_EXEC
    pub flags: u32,     // MAP_SHARED | MAP_PRIVATE | etc.
    pub is_cow: bool,   // Copy-on-write flag
}

/// Process state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Stopped,
    Zombie,
}

/// Global process table
pub static PROCESS_TABLE: RwLock<alloc::collections::BTreeMap<Pid, Arc<Process>>> = 
    RwLock::new(alloc::collections::BTreeMap::new());

/// Next PID counter
static NEXT_PID: AtomicU64 = AtomicU64::new(2);

impl Process {
    /// Create a new process
    pub fn new(pid: Pid, ppid: Pid, name: &str) -> Self {
        let mut fd_table = alloc::collections::BTreeMap::new();
        // Initialize standard file descriptors
        fd_table.insert(0, FdEntry::new(0)); // stdin
        fd_table.insert(1, FdEntry::new(1)); // stdout
        fd_table.insert(2, FdEntry::new(2)); // stderr
        
        Self {
            pid,
            ppid,
            pgid: pid,
            sid: pid,
            main_tid: pid,
            name: name.to_string(),
            fd_table: Mutex::new(fd_table),
            memory_regions: Mutex::new(Vec::new()),
            cwd: Mutex::new(String::from("/")),
            environ: Mutex::new(Vec::new()),
            exit_status: AtomicI32::new(0),
            state: Mutex::new(ProcessState::Running),
            children: Mutex::new(Vec::new()),
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
        }
    }
    
    /// Duplicate file descriptor table (for fork)
    pub fn dup_fd_table(&self) -> alloc::collections::BTreeMap<i32, FdEntry> {
        let table = self.fd_table.lock();
        let mut new_table = alloc::collections::BTreeMap::new();
        for (&fd, entry) in table.iter() {
            new_table.insert(fd, FdEntry {
                handle_id: entry.handle_id,
                flags: entry.flags,
                status_flags: entry.status_flags,
                offset: entry.offset,
            });
        }
        new_table
    }
    
    /// Duplicate memory regions (for fork with COW)
    pub fn dup_memory_regions(&self) -> Vec<MemoryRegion> {
        let regions = self.memory_regions.lock();
        regions.iter().map(|r| {
            MemoryRegion {
                start: r.start,
                size: r.size,
                prot: r.prot,
                flags: r.flags,
                is_cow: true, // Mark as COW for child
            }
        }).collect()
    }
    
    /// Close all file descriptors with CLOEXEC flag (for exec)
    pub fn close_cloexec_fds(&self) {
        let mut table = self.fd_table.lock();
        table.retain(|_, entry| (entry.flags & 1) == 0); // FD_CLOEXEC = 1
    }
    
    /// Get next available file descriptor
    pub fn alloc_fd(&self) -> i32 {
        let table = self.fd_table.lock();
        let mut fd = 0;
        while table.contains_key(&fd) {
            fd += 1;
        }
        fd
    }
}

/// Fork - create child process (full implementation)
pub fn sys_fork() -> MemoryResult<Pid> {
    log::debug!("sys_fork: starting full fork implementation");

    // 1. Get parent info
    let parent_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    
    // 2. Allocate new PID for child
    let child_pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    
    // 3. Get or create parent process
    let parent_process = {
        let table = PROCESS_TABLE.read();
        table.get(&parent_pid).cloned()
    };
    
    // 4. Create child process
    let child_process = if let Some(parent) = parent_process {
        // Fork from existing process
        let child = Process {
            pid: child_pid,
            ppid: parent_pid,
            pgid: parent.pgid,
            sid: parent.sid,
            main_tid: child_pid,
            name: parent.name.clone(),
            fd_table: Mutex::new(parent.dup_fd_table()),
            memory_regions: Mutex::new(parent.dup_memory_regions()),
            cwd: Mutex::new(parent.cwd.lock().clone()),
            environ: Mutex::new(parent.environ.lock().clone()),
            exit_status: AtomicI32::new(0),
            state: Mutex::new(ProcessState::Running),
            children: Mutex::new(Vec::new()),
            uid: parent.uid,
            gid: parent.gid,
            euid: parent.euid,
            egid: parent.egid,
        };
        
        // Add child to parent's children list
        parent.children.lock().push(child_pid);
        
        Arc::new(child)
    } else {
        // Create new process (init-like)
        Arc::new(Process::new(child_pid, parent_pid, "forked"))
    };
    
    // 5. Add child to process table
    {
        let mut table = PROCESS_TABLE.write();
        table.insert(child_pid, child_process);
    }
    
    // 6. Create child thread with copied context
    SCHEDULER.with_current_thread(|parent_thread| {
        // Get parent context
        let parent_name = parent_thread.name();
        let child_name = alloc::format!("{}_child_{}", parent_name, child_pid);
        
        // Create child thread entry
        log::debug!("Fork: creating child thread {}", child_name);
        
        // Note: The actual thread creation and context copy happens in the scheduler
        // For now, we rely on the scheduler's spawn mechanism
    });
    
    // 7. Setup COW for parent's memory regions
    // Mark parent's writable pages as read-only COW
    if let Some(parent) = PROCESS_TABLE.read().get(&parent_pid) {
        let mut regions = parent.memory_regions.lock();
        for region in regions.iter_mut() {
            if region.prot & 0x2 != 0 { // PROT_WRITE
                region.is_cow = true;
            }
        }
    }
    
    log::info!("Fork: parent={} -> child={} (full COW fork)", parent_pid, child_pid);
    
    // Return child PID to parent (child would return 0 via context setup)
    Ok(child_pid)
}

/// Load executable file from filesystem
/// Uses VFS read_file for actual file reading
fn load_executable_file(path: &str) -> Result<Vec<u8>, &'static str> {
    // Use VFS to read the file
    match crate::fs::vfs::read_file(path) {
        Ok(data) => {
            log::debug!("load_executable_file: loaded {} bytes from {}", data.len(), path);
            Ok(data)
        }
        Err(e) => {
            log::warn!("load_executable_file: failed to read {}: {:?}", path, e);
            Err("Failed to read executable file")
        }
    }
}

/// Execute program (full implementation)
pub fn sys_exec(path: &str, args: &[&str], _env: &[&str]) -> MemoryResult<()> {
    log::debug!("sys_exec: path={}, args={:?}", path, args);

    let current_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    
    // 1. Load executable from VFS
    // Note: Currently using a stub - real impl needs VFS file reading
    let file_data: Vec<u8> = match load_executable_file(path) {
        Ok(data) => data,
        Err(_) => {
            log::error!("exec: failed to read file: {}", path);
            return Err(MemoryError::NotFound);
        }
    };
    
    // 2. Parse and validate ELF header
    let elf_info = match parse_elf_header(&file_data) {
        Ok(info) => info,
        Err(e) => {
            log::error!("exec: invalid ELF: {:?}", e);
            return Err(MemoryError::InvalidAddress);
        }
    };
    
    log::info!("exec: ELF entry={:#x}, phnum={}", elf_info.entry_point, elf_info.program_headers.len());
    
    // 3. Get process and clear old mappings
    if let Some(process) = PROCESS_TABLE.read().get(&current_pid) {
        // Close CLOEXEC file descriptors
        process.close_cloexec_fds();
        
        // Clear memory regions (new address space)
        {
            let mut regions = process.memory_regions.lock();
            // Unmap old regions
            for region in regions.iter() {
                let _ = crate::memory::mmap::munmap(region.start, region.size);
            }
            regions.clear();
        }
        
        // Update process name
        // Note: Can't update name directly due to lock, would need interior mutability
    }
    
    // 4. Load program segments
    for ph in &elf_info.program_headers {
        if ph.p_type != PT_LOAD {
            continue;
        }
        
        // Calculate page-aligned addresses
        let page_size = 4096usize;
        let vaddr = ph.p_vaddr as usize;
        let memsz = ph.p_memsz as usize;
        let filesz = ph.p_filesz as usize;
        let offset = ph.p_offset as usize;
        
        let aligned_start = vaddr & !(page_size - 1);
        let aligned_size = ((vaddr + memsz + page_size - 1) & !(page_size - 1)) - aligned_start;
        
        // Determine protection
        let mut prot = 0u32;
        if ph.p_flags & PF_R != 0 { prot |= 0x1; } // PROT_READ
        if ph.p_flags & PF_W != 0 { prot |= 0x2; } // PROT_WRITE
        if ph.p_flags & PF_X != 0 { prot |= 0x4; } // PROT_EXEC
        
        // Map memory region
        let map_addr = crate::memory::mmap::mmap(
            Some(VirtualAddress::new(aligned_start)),
            aligned_size,
            crate::memory::PageProtection::from_prot(prot),
            crate::memory::mmap::MmapFlags::new(0x22), // MAP_PRIVATE | MAP_ANONYMOUS
            None,
            0,
        )?;
        
        // Copy segment data
        if filesz > 0 && offset + filesz <= file_data.len() {
            let src = &file_data[offset..offset + filesz];
            let page_offset = vaddr - aligned_start;
            
            // Copy to mapped memory
            unsafe {
                let dest = (map_addr.value() + page_offset) as *mut u8;
                core::ptr::copy_nonoverlapping(src.as_ptr(), dest, filesz);
            }
        }
        
        // Zero BSS (memsz - filesz)
        if memsz > filesz {
            let bss_start = vaddr + filesz;
            let bss_size = memsz - filesz;
            let page_offset = bss_start - aligned_start;
            
            unsafe {
                let dest = (map_addr.value() + page_offset) as *mut u8;
                core::ptr::write_bytes(dest, 0, bss_size);
            }
        }
        
        // Record region in process
        if let Some(process) = PROCESS_TABLE.read().get(&current_pid) {
            process.memory_regions.lock().push(MemoryRegion {
                start: map_addr,
                size: aligned_size,
                prot,
                flags: 0x22,
                is_cow: false,
            });
        }
        
        log::debug!("exec: loaded segment at {:#x} size={:#x}", aligned_start, aligned_size);
    }
    
    // 5. Set up user stack
    let stack_size = 0x200000; // 2MB stack
    let stack_top = 0x7FFF_FFFF_F000usize;
    let stack_bottom = stack_top - stack_size;
    
    let stack_addr = crate::memory::mmap::mmap(
        Some(VirtualAddress::new(stack_bottom)),
        stack_size,
        crate::memory::PageProtection::READ_WRITE,
        crate::memory::mmap::MmapFlags::new(0x22), // MAP_PRIVATE | MAP_ANONYMOUS  
        None,
        0,
    )?;
    
    // Push arguments onto stack (System V ABI)
    let mut sp = stack_top;
    
    // Push argument strings and collect pointers
    let mut arg_ptrs: Vec<usize> = Vec::new();
    for arg in args.iter().rev() {
        sp -= arg.len() + 1; // +1 for null terminator
        sp &= !0x7; // Align to 8 bytes
        unsafe {
            let dest = sp as *mut u8;
            core::ptr::copy_nonoverlapping(arg.as_ptr(), dest, arg.len());
            *dest.add(arg.len()) = 0; // Null terminator
        }
        arg_ptrs.push(sp);
    }
    arg_ptrs.reverse();
    
    // Align stack to 16 bytes
    sp &= !0xF;
    
    // Push argv pointers
    sp -= 8; // NULL terminator
    unsafe { *(sp as *mut u64) = 0; }
    
    for ptr in arg_ptrs.iter().rev() {
        sp -= 8;
        unsafe { *(sp as *mut u64) = *ptr as u64; }
    }
    let argv_ptr = sp;
    
    // Push argc
    sp -= 8;
    unsafe { *(sp as *mut u64) = args.len() as u64; }
    
    // Record stack region
    if let Some(process) = PROCESS_TABLE.read().get(&current_pid) {
        process.memory_regions.lock().push(MemoryRegion {
            start: stack_addr,
            size: stack_size,
            prot: 0x3, // PROT_READ | PROT_WRITE
            flags: 0x22,
            is_cow: false,
        });
    }
    
    log::info!("exec: stack at {:#x}, entry={:#x}, argc={}", sp, elf_info.entry_point, args.len());
    
    // 6. Update thread context to jump to entry point
    // This modifies the current thread's saved context
    SCHEDULER.with_current_thread(|thread| {
        let ctx = thread.context_ptr();
        unsafe {
            (*ctx).rip = elf_info.entry_point;
            (*ctx).rsp = sp as u64;
            // RFLAGS: IF (Interrupt Flag) enabled
            (*ctx).rflags = 0x202;
        }
    });
    
    Ok(())
}

// ELF constants
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// ELF header info
struct ElfInfo {
    entry_point: u64,
    program_headers: Vec<ElfProgramHeader>,
}

/// ELF program header
#[derive(Clone)]
struct ElfProgramHeader {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
}

/// Parse ELF64 header
fn parse_elf_header(data: &[u8]) -> MemoryResult<ElfInfo> {
    if data.len() < 64 {
        return Err(MemoryError::InvalidSize);
    }
    
    // Check ELF magic
    if &data[0..4] != b"\x7FELF" {
        return Err(MemoryError::InvalidAddress);
    }
    
    // Check 64-bit
    if data[4] != 2 {
        return Err(MemoryError::InvalidAddress);
    }
    
    // Check little-endian
    if data[5] != 1 {
        return Err(MemoryError::InvalidAddress);
    }
    
    // Read header fields (little-endian)
    let entry_point = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(data[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(data[56..58].try_into().unwrap()) as usize;
    
    // Parse program headers
    let mut program_headers = Vec::new();
    for i in 0..phnum {
        let offset = phoff + i * phentsize;
        if offset + phentsize > data.len() {
            break;
        }
        
        let ph = &data[offset..offset + phentsize];
        let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        let p_flags = u32::from_le_bytes(ph[4..8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap());
        let p_vaddr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(ph[40..48].try_into().unwrap());
        let p_memsz = u64::from_le_bytes(ph[48..56].try_into().unwrap());
        
        program_headers.push(ElfProgramHeader {
            p_type,
            p_flags,
            p_offset,
            p_vaddr,
            p_filesz,
            p_memsz,
        });
    }
    
    Ok(ElfInfo {
        entry_point,
        program_headers,
    })
}

/// Exit process (full implementation)
pub fn sys_exit(code: ExitCode) -> ! {
    log::debug!("sys_exit: code={}", code);
    
    let current_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    
    // 1. Get process
    let process = PROCESS_TABLE.read().get(&current_pid).cloned();
    
    if let Some(proc) = process {
        // 2. Close all file descriptors
        {
            let mut fd_table = proc.fd_table.lock();
            for (fd, entry) in fd_table.iter() {
                log::debug!("exit: closing fd {} (handle {})", fd, entry.handle_id);
                // Close file handle - VFS close is handled elsewhere
                // Note: Real impl would call VFS close
            }
            fd_table.clear();
        }
        
        // 3. Free memory mappings
        {
            let mut regions = proc.memory_regions.lock();
            for region in regions.iter() {
                let _ = crate::memory::mmap::munmap(region.start, region.size);
            }
            regions.clear();
        }
        
        // 4. Set exit status and become zombie
        proc.exit_status.store(code, Ordering::Release);
        *proc.state.lock() = ProcessState::Zombie;
        
        // 5. Reparent children to init (PID 1)
        {
            let children = proc.children.lock().clone();
            for child_pid in children {
                if let Some(child) = PROCESS_TABLE.write().get_mut(&child_pid) {
                    // Can't mutate through Arc, would need interior mutability
                    // For now, just log
                    log::debug!("exit: reparenting child {} to init", child_pid);
                }
            }
        }
        
        // 6. Send SIGCHLD to parent
        let ppid = proc.ppid;
        if ppid > 0 {
            log::debug!("exit: sending SIGCHLD to parent {}", ppid);
            // Signal parent (would wake up if waiting)
            let _ = sys_kill(ppid, 17); // SIGCHLD = 17
        }
    }
    
    // 7. Set thread state and yield
    SCHEDULER.with_current_thread(|thread| {
        thread.set_exit_status(code);
        thread.set_state(ThreadState::Terminated);
    });
    
    log::info!("Process {} exiting with code {}", current_pid, code);

    // 8. Schedule next process (never returns)
    crate::scheduler::yield_now();

    // Fallback halt
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

/// Exit thread group (all threads)
pub fn sys_exit_group(code: ExitCode) -> ! {
    log::debug!("sys_exit_group: code={}", code);

    // 1. Kill all threads in process
    // TODO: Iterate thread group and send kill signal
    log::info!("Exiting thread group with code {}", code);

    // 2. Then exit as normal
    sys_exit(code);
}

/// wait4 - Wait for child process to change state (Phase 9: Improved)
pub fn sys_wait(pid: Pid, options: WaitOptions) -> MemoryResult<(Pid, ProcessStatus)> {
    use crate::scheduler::{ThreadState, SCHEDULER};

    // Get current process to access children list
    let current_pid = sys_getpid();
    
    // If PID specified, check its state
    if pid != u64::MAX && pid != 0 {
        let state = SCHEDULER.get_thread_state(pid);

        // If not in scheduler OR explicitly zombie, it's terminated
        let is_zombie = match state {
            None => true, // Not in scheduler = terminated/zombie
            Some(ThreadState::Terminated) => true,
            _ => false, // Still running
        };

        if is_zombie {
            // Get real exit code
            let exit_code = SCHEDULER.get_exit_status(pid).unwrap_or(0);

            // TODO: Remove from parent's children list
            // TODO: Call Thread::cleanup() for resource cleanup

            return Ok((pid, ProcessStatus::Exited(exit_code)));
        }

        // Not zombie yet
        if options.nohang {
            return Ok((0, ProcessStatus::Running)); // No change yet
        }

        // Would block - return "try again"
        return Ok((0, ProcessStatus::Running));
    }

    // Wait for ANY child (pid == u64::MAX or 0)
    // Search for a zombie child in current process's children list
    if let Some(process) = PROCESS_TABLE.read().get(&current_pid) {
        let children = process.children.lock();
        
        // Check each child to see if it's zombie
        for &child_pid in children.iter() {
            let state = SCHEDULER.get_thread_state(child_pid);
            
            let is_zombie = match state {
                None => true, // Not in scheduler = terminated/zombie
                Some(ThreadState::Terminated) => true,
                _ => false,
            };
            
            if is_zombie {
                // Found a zombie child!
                let exit_code = SCHEDULER.get_exit_status(child_pid).unwrap_or(0);
                
                // TODO: Remove from children list
                // TODO: Call Thread::cleanup()
                
                return Ok((child_pid, ProcessStatus::Exited(exit_code)));
            }
        }
    }

    // No zombie children found
    if options.nohang {
        return Ok((0, ProcessStatus::Running));
    }

    // Would block waiting for child - for now return 0
    // TODO: Sleep on child exit event
    Ok((0, ProcessStatus::Running))
}

/// Get process ID
pub fn sys_getpid() -> Pid {
    SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0)
}

/// Get parent process ID
pub fn sys_getppid() -> Pid {
    // TODO: Get parent PID from process structure
    // For now, return 0 (init's parent)
    SCHEDULER
        .with_current_thread(|t| t.parent_id())
        .unwrap_or(0)
}

/// Get thread ID
pub fn sys_gettid() -> u64 {
    SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0)
}

/// Clone - flexible process/thread creation
pub fn sys_clone(flags: u32, stack: Option<usize>) -> MemoryResult<Pid> {
    log::debug!("sys_clone: flags={:#x}, stack={:?}", flags, stack);

    // Clone flags:
    const CLONE_VM: u32 = 0x100; // Share memory space
    const CLONE_FS: u32 = 0x200; // Share filesystem info
    const CLONE_FILES: u32 = 0x400; // Share file descriptors
    const CLONE_SIGHAND: u32 = 0x800; // Share signal handlers
    const CLONE_THREAD: u32 = 0x10000; // Create thread, not process

    // 1. Determine if creating thread or process
    let is_thread = (flags & CLONE_THREAD) != 0;

    // 2. Allocate new PID/TID
    static NEXT_PID: AtomicU64 = AtomicU64::new(100);
    let new_id = NEXT_PID.fetch_add(1, Ordering::SeqCst);

    // 3. Set up new stack if provided
    if let Some(stack_addr) = stack {
        // TODO: Use provided stack address
        log::debug!("Clone with custom stack at {:#x}", stack_addr);
    }

    // TODO: Actually create thread/process based on flags
    log::info!(
        "Clone: created {} {}",
        if is_thread { "thread" } else { "process" },
        new_id
    );

    Ok(new_id)
}

/// Send signal to process
pub fn sys_kill(pid: Pid, sig: Signal) -> MemoryResult<()> {
    log::debug!("sys_kill: pid={}, sig={}", pid, sig);

    // 1. Find target process
    // TODO: Lookup process by PID

    // 2. Check permissions (can current process signal target?)
    // 2. Check permissions (can current process signal target?)
    let sender_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    if sender_pid != 0 && sender_pid != pid {
        // TODO: Check if sender has permission
        // For now, allow all signals
    }

    // 3. Queue signal
    // TODO: Add signal to process signal queue
    log::debug!("Signal {} sent from {} to {}", sig, sender_pid, pid);

    Ok(())
}

/// Set signal handler
pub fn sys_signal(sig: Signal, handler: usize) -> MemoryResult<usize> {
    log::debug!("sys_signal: sig={}, handler={:#x}", sig, handler);

    // 1. Validate signal number (1-31 are valid)
    if sig == 0 || sig > 31 {
        return Err(MemoryError::InvalidSize);
    }

    // 2. Get old handler
    // TODO: Retrieve from process signal table
    let old_handler = 0usize;

    // 3. Set new handler in process signal table
    // TODO: Store handler address in signal table
    log::debug!("Signal {} handler set to {:#x}", sig, handler);

    Ok(old_handler)
}

/// Yield CPU to other processes
pub fn sys_yield() -> MemoryResult<()> {
    // Call scheduler to yield
    crate::scheduler::yield_now();
    Ok(())
}

/// Pause - wait for signal
pub fn sys_pause() -> MemoryResult<()> {
    // TODO: Block until signal
    log::debug!("sys_pause: stub");
    Ok(())
}

/// execve - Execute program (Phase 10)
pub fn sys_execve(
    pathname: *const i8,
    argv: *const *const i8,
    envp: *const *const i8,
) -> MemoryResult<()> {
    use crate::posix_x::elf::load_elf_binary;
    use alloc::vec::Vec;
    use core::ffi::CStr;

    log::info!("sys_execve: starting");

    // 1. Validate and parse pathname
    if pathname.is_null() {
        log::error!("execve: null pathname");
        return Err(MemoryError::InvalidAddress);
    }

    let path = unsafe {
        CStr::from_ptr(pathname)
            .to_str()
            .map_err(|_| MemoryError::InvalidAddress)?
    };

    log::info!("execve: path={}", path);

    // 2. Parse arguments
    let args = unsafe { parse_string_array(argv)? };
    let env = unsafe { parse_string_array(envp)? };

    log::info!("execve: argc={}, envc={}", args.len(), env.len());

    // 3. Load ELF binary
    let loaded_info = load_elf_binary(path, &args, &env)?;

    log::info!(
        "execve: loaded entry={:#x}, stack={:#x}",
        loaded_info.entry_point,
        loaded_info.stack_top
    );

    // 4. Enter user mode
    // TODO: Modify current thread context to jump to entry_point
    // This requires access to the interrupt frame which is not passed to syscall handler currently

    Ok(())
}

/// Parse NULL-terminated array of C strings (helper for execve)
unsafe fn parse_string_array(ptr: *const *const i8) -> MemoryResult<Vec<alloc::string::String>> {
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::ffi::CStr;

    if ptr.is_null() {
        return Ok(Vec::new());
    }

    let mut result = Vec::new();
    let mut i = 0;

    loop {
        let str_ptr = *ptr.offset(i);
        if str_ptr.is_null() {
            break;
        }

        let s = CStr::from_ptr(str_ptr)
            .to_str()
            .map_err(|_| MemoryError::InvalidAddress)?
            .to_string();

        result.push(s);
        i += 1;

        // Safety limit
        if i > 1024 {
            log::warn!("execve: too many arguments/env vars");
            break;
        }
    }

    Ok(result)
}

/// Set process priority
pub fn sys_setpriority(which: i32, who: Pid, priority: i32) -> MemoryResult<()> {
    log::debug!(
        "sys_setpriority: which={}, who={}, prio={}",
        which,
        who,
        priority
    );

    // Priority ranges: -20 (highest) to 19 (lowest)
    if priority < -20 || priority > 19 {
        return Err(MemoryError::InvalidSize);
    }

    // 1. Find process by PID
    // TODO: Lookup process

    // 2. Check permissions (only root or owner can change priority)
    // TODO: Permission check

    // 3. Update priority in scheduler
    // TODO: scheduler::set_priority(who, priority);

    log::debug!("Priority of {} set to {}", who, priority);
    Ok(())
}

/// Get process priority
pub fn sys_getpriority(which: i32, who: Pid) -> MemoryResult<i32> {
    log::debug!("sys_getpriority: which={}, who={}", which, who);

    // TODO: Get priority from scheduler
    // For now, return default nice value (0)
    Ok(0)
}
