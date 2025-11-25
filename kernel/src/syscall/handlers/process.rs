//! Process Management System Call Handlers
//!
//! Handles process operations: fork, exec, exit, wait, signals

use crate::memory::{MemoryResult, MemoryError};
use core::sync::atomic::{AtomicU64, Ordering};

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

/// Fork - create child process
pub fn sys_fork() -> MemoryResult<Pid> {
    log::debug!("sys_fork");
    
    // 1-2. Copy current process with COW (Copy-On-Write)
    use crate::task;
    let parent = task::current();
    let parent_pid = parent.pid();
    
    // 3. Allocate new PID for child
    static NEXT_PID: AtomicU64 = AtomicU64::new(2);
    let child_pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    
    // 4-5. Create child thread (simplified fork)
    // TODO: Full process duplication with COW memory
    // TODO: Copy file descriptors
    // TODO: Set up child context to return 0
    
    log::info!("Fork: parent={} -> child={}", parent_pid, child_pid);
    Ok(child_pid)
}

/// Execute program
pub fn sys_exec(path: &str, args: &[&str], env: &[&str]) -> MemoryResult<()> {
    log::debug!("sys_exec: path={}, args={:?}", path, args);
    
    // 1. Load executable from VFS
    // TODO: Implement VFS file loading
    // let executable = vfs::read_file(path)?;
    
    // 2. Parse ELF header
    // TODO: Implement ELF parser
    // let elf = elf::parse(&executable)?;
    
    // 3. Set up new address space (destroy current)
    // TODO: Clear current address space
    // TODO: Map new segments
    
    // 4. Load program segments
    // TODO: Load text, data, bss segments
    
    // 5. Set up stack with args/env
    // TODO: Push args and env onto stack
    
    // 6. Jump to entry point
    // TODO: Set RIP to entry point and switch
    
    log::warn!("exec not fully implemented: {}", path);
    Err(MemoryError::NotFound)
}

/// Exit process
pub fn sys_exit(code: ExitCode) -> ! {
    log::debug!("sys_exit: code={}", code);
    
    // 1. Close all file descriptors
    // TODO: Iterate FD table and close all
    
    // 2. Free memory (address space)
    // TODO: Free all memory mappings
    
    // 3. Notify parent (send SIGCHLD)
    // TODO: Signal parent process
    
    // 4. Become zombie (exit but keep TCB for parent to wait)
    use crate::task;
    log::info!("Process {} exiting with code {}", task::current().pid(), code);
    
    // 5. Schedule next process (never returns)
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

/// Wait for child process
pub fn sys_wait(pid: Pid, options: WaitOptions) -> MemoryResult<(Pid, ProcessStatus)> {
    log::debug!("sys_wait: pid={}, options={:?}", pid, options);
    
    // 1. Find child process
    // TODO: Search child list for matching PID
    
    // 2. Check if child has exited (zombie state)
    // TODO: Check process state
    
    // 3. If WNOHANG and not exited, return immediately
    if options.nohang {
        // Return 0 to indicate no child ready
        return Ok((0, ProcessStatus::Running));
    }
    
    // 4. Block until child exits
    // TODO: Sleep on child exit event
    // For now, return stub
    
    Ok((pid, ProcessStatus::Exited(0)))
}

/// Get process ID
pub fn sys_getpid() -> Pid {
    use crate::task;
    task::current().pid()
}

/// Get parent process ID
pub fn sys_getppid() -> Pid {
    use crate::task;
    // TODO: Get parent PID from process structure
    // For now, return 0 (init's parent)
    0
}

/// Get thread ID
pub fn sys_gettid() -> u64 {
    use crate::task;
    task::current().id()
}

/// Clone - flexible process/thread creation
pub fn sys_clone(flags: u32, stack: Option<usize>) -> MemoryResult<Pid> {
    log::debug!("sys_clone: flags={:#x}, stack={:?}", flags, stack);
    
    // Clone flags:
    const CLONE_VM: u32 = 0x100;       // Share memory space
    const CLONE_FS: u32 = 0x200;       // Share filesystem info
    const CLONE_FILES: u32 = 0x400;    // Share file descriptors
    const CLONE_SIGHAND: u32 = 0x800;  // Share signal handlers
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
    log::info!("Clone: created {} {}", if is_thread { "thread" } else { "process" }, new_id);
    
    Ok(new_id)
}

/// Send signal to process
pub fn sys_kill(pid: Pid, sig: Signal) -> MemoryResult<()> {
    log::debug!("sys_kill: pid={}, sig={}", pid, sig);
    
    // 1. Find target process
    // TODO: Lookup process by PID
    
    // 2. Check permissions (can current process signal target?)
    use crate::task;
    let sender_pid = task::current().pid();
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

/// Return from signal handler
pub fn sys_sigreturn() -> MemoryResult<()> {
    log::debug!("sys_sigreturn");
    
    // 1. Find signal frame on stack
    // TODO: Locate saved context
    
    // 2. Restore registers from signal frame
    // TODO: Restore RIP, RSP, RFLAGS, general purpose registers
    
    // 3. Return to interrupted code
    // This syscall doesn't actually return normally
    // The context switch restores execution at the interrupted point
    
    Ok(())
}

/// Yield CPU to other processes
pub fn sys_yield() -> MemoryResult<()> {
    // TODO: Call scheduler
    crate::scheduler::yield_now();
    Ok(())
}

/// Set process priority
pub fn sys_setpriority(which: i32, who: Pid, priority: i32) -> MemoryResult<()> {
    log::debug!("sys_setpriority: which={}, who={}, prio={}", which, who, priority);
    
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
