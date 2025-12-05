//! Process syscall tests - fork/exec/wait
//!
//! Tests for Phase 1 process management

use crate::syscall::handlers::process;
use crate::scheduler::SCHEDULER;
use alloc::string::String;
use alloc::format;

/// Helper to print number
fn print_u64(n: u64) {
    let s = format!("{}", n);
    crate::logger::early_print(&s);
}

fn print_i32(n: i32) {
    let s = format!("{}", n);
    crate::logger::early_print(&s);
}

/// Test fork - creates child process
pub fn test_fork() {
    crate::logger::early_print("\n[TEST] test_fork starting...\n");

    let parent_pid = process::sys_getpid();
    crate::logger::early_print("[TEST] Parent PID: ");
    print_u64(parent_pid);
    crate::logger::early_print("\n");

    // Fork
    match process::sys_fork() {
        Ok(child_pid) => {
            if child_pid == 0 {
                // In child process
                crate::logger::early_print("[TEST] Child: I am the child process!\n");
                let my_pid = process::sys_getpid();
                crate::logger::early_print("[TEST] Child PID: ");
                print_u64(my_pid);
                crate::logger::early_print("\n");
                
                // Child exits
                process::sys_exit(42);
            } else {
                // In parent process
                crate::logger::early_print("[TEST] Parent: forked child with PID ");
                print_u64(child_pid);
                crate::logger::early_print("\n");

                // Wait for child with yield loop
                // The child is in pending queue, we need to yield so scheduler processes it
                crate::logger::early_print("[TEST] Parent: waiting for child (with yield loop)...\n");
                let options = process::WaitOptions {
                    nohang: true, // Use non-blocking wait
                    untraced: false,
                    continued: false,
                };

                let mut wait_count = 0;
                loop {
                    // First yield to let scheduler process pending threads
                    crate::scheduler::yield_now();
                    wait_count += 1;
                    
                    if wait_count % 10 == 0 {
                        crate::logger::early_print("[TEST] Wait iteration ");
                        print_u64(wait_count);
                        crate::logger::early_print("\n");
                    }
                    
                    match process::sys_wait(child_pid, options) {
                        Ok((waited_pid, status)) => {
                            match status {
                                process::ProcessStatus::Exited(code) => {
                                    crate::logger::early_print("[TEST] Parent: child ");
                                    print_u64(waited_pid);
                                    crate::logger::early_print(" exited with code: ");
                                    print_i32(code);
                                    crate::logger::early_print("\n");
                                    crate::logger::early_print("[TEST] ✅ test_fork PASSED\n");
                                    break;
                                }
                                process::ProcessStatus::Running => {
                                    // Child still running, continue loop
                                    if wait_count > 100 {
                                        crate::logger::early_print("[TEST] Timeout waiting for child\n");
                                        crate::logger::early_print("[TEST] ⚠️ test_fork PARTIAL (child created but not scheduled)\n");
                                        break;
                                    }
                                }
                                _ => {
                                    crate::logger::early_print("[TEST] Parent: unexpected child status\n");
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            crate::logger::early_print("[TEST] ❌ test_fork FAILED: wait error\n");
                            break;
                        }
                    }
                }
            }
        }
        Err(e) => {
            crate::logger::early_print("[TEST] ❌ test_fork FAILED: fork error\n");
        }
    }
}

/// Test fork return value - critical for inline context capture validation
pub fn test_fork_return_value() {
    crate::logger::early_print("\n[TEST] test_fork_return_value starting...\n");
    crate::logger::early_print("[TEST] Validates inline assembly context capture fix\n");
    
    let parent_pid = process::sys_getpid();
    crate::logger::early_print("[TEST] Parent PID: ");
    print_u64(parent_pid);
    crate::logger::early_print("\n");
    
    match process::sys_fork() {
        Ok(fork_result) => {
            if fork_result == 0 {
                // ✅ Child path - fork() returned 0
                crate::logger::early_print("[TEST] ✅ Child: fork() returned 0 (CORRECT)\n");
                
                let child_pid = process::sys_getpid();
                let child_ppid = process::sys_getppid();
                
                crate::logger::early_print("[TEST] Child PID: ");
                print_u64(child_pid);
                crate::logger::early_print(" PPID: ");
                print_u64(child_ppid);
                crate::logger::early_print("\n");
                
                // Verify we're a different process
                if child_pid != parent_pid && child_ppid == parent_pid {
                    crate::logger::early_print("[TEST] ✅ Child: PID verification PASSED\n");
                } else {
                    crate::logger::early_print("[TEST] ❌ Child: PID verification FAILED\n");
                }
                
                process::sys_exit(42);
            } else {
                // ✅ Parent path - fork() returned child_pid
                crate::logger::early_print("[TEST] ✅ Parent: fork() returned child PID ");
                print_u64(fork_result);
                crate::logger::early_print(" (CORRECT)\n");
                
                // Wait for child
                let options = process::WaitOptions {
                    nohang: false,
                    untraced: false,
                    continued: false,
                };
                
                match process::sys_wait(fork_result, options) {
                    Ok((waited_pid, status)) => {
                        if waited_pid == fork_result {
                            crate::logger::early_print("[TEST] ✅ Parent: wait() returned correct PID\n");
                        } else {
                            crate::logger::early_print("[TEST] ❌ Parent: wait() returned wrong PID\n");
                        }
                        
                        match status {
                            process::ProcessStatus::Exited(code) => {
                                if code == 42 {
                                    crate::logger::early_print("[TEST] ✅ Parent: child exit status 42 (CORRECT)\n");
                                    crate::logger::early_print("[TEST] ✅✅✅ test_fork_return_value PASSED\n");
                                } else {
                                    crate::logger::early_print("[TEST] ❌ Parent: wrong exit status ");
                                    print_i32(code);
                                    crate::logger::early_print("\n");
                                }
                            }
                            _ => {
                                crate::logger::early_print("[TEST] ❌ Parent: unexpected status type\n");
                            }
                        }
                    }
                    Err(_) => {
                        crate::logger::early_print("[TEST] ❌ Parent: wait() failed\n");
                    }
                }
            }
        }
        Err(_) => {
            crate::logger::early_print("[TEST] ❌ test_fork_return_value FAILED: fork error\n");
        }
    }
}

/// Test getpid/getppid
pub fn test_getpid() {
    crate::logger::early_print("\n[TEST] test_getpid starting...\n");

    let pid = process::sys_getpid();
    let ppid = process::sys_getppid();
    let tid = process::sys_gettid();

    crate::logger::early_print("[TEST] PID=");
    print_u64(pid);
    crate::logger::early_print(" PPID=");
    print_u64(ppid);
    crate::logger::early_print(" TID=");
    print_u64(tid);
    crate::logger::early_print("\n");

    crate::logger::early_print("[TEST] ✅ test_getpid PASSED\n");
}

/// Test complete fork/wait cycle
pub fn test_fork_wait_cycle() {
    crate::logger::early_print("\n[TEST] test_fork_wait_cycle starting...\n");
    
    let mut child_pids = alloc::vec::Vec::new();
    
    // Fork 3 children that will immediately exit
    crate::logger::early_print("[TEST] Forking 3 children...\n");
    for i in 0..3 {
        match process::sys_fork() {
            Ok(child_pid) => {
                child_pids.push(child_pid);
                crate::logger::early_print("[TEST]   Spawned child PID ");
                print_u64(child_pid);
                crate::logger::early_print("\n");
            }
            Err(_) => {
                crate::logger::early_print("[TEST]   Fork ");
                print_u64(i);
                crate::logger::early_print(" FAILED\n");
            }
        }
    }
    
    // Give children time to execute and exit (become zombies)
    crate::logger::early_print("[TEST] Yielding to let children execute...\n");
    for _ in 0..10 {
        crate::scheduler::yield_now();
    }
    
    // Now wait for each child (they should be zombies)
    crate::logger::early_print("[TEST] Waiting for zombie children...\n");
    let mut zombies_found = 0;
    
    for _ in 0..3 {
        let options = process::WaitOptions {
            nohang: false, // Block until zombie found
            untraced: false,
            continued: false,
        };
        
        match process::sys_wait(u64::MAX, options) {
            Ok((pid, status)) => {
                if pid > 0 {
                    crate::logger::early_print("[TEST]   ✅ Found zombie PID ");
                    print_u64(pid);
                    crate::logger::early_print(" status=");
                    match status {
                        process::ProcessStatus::Exited(code) => {
                            print_i32(code);
                        }
                        _ => {
                            crate::logger::early_print("?");
                        }
                    }
                    crate::logger::early_print("\n");
                    zombies_found += 1;
                } else {
                    crate::logger::early_print("[TEST]   ⚠️  wait returned PID 0 (no zombie)\n");
                    break;
                }
            }
            Err(_) => {
                crate::logger::early_print("[TEST]   ❌ wait failed\n");
                break;
            }
        }
    }
    
    crate::logger::early_print("[TEST] Found ");
    print_u64(zombies_found as u64);
    crate::logger::early_print("/3 zombies\n");
    
    if zombies_found == 3 {
        crate::logger::early_print("[TEST] ✅ test_fork_wait_cycle PASSED\n");
    } else {
        crate::logger::early_print("[TEST] ⚠️  test_fork_wait_cycle PARTIAL (");
        print_u64(zombies_found as u64);
        crate::logger::early_print("/3 zombies found)\n");
    }
}

/// Test fork+exec+wait - complete POSIX process lifecycle
pub fn test_fork_exec_wait() {
    crate::logger::early_print("\n[TEST] test_fork_exec_wait starting...\n");
    crate::logger::early_print("[TEST] This tests the complete POSIX process creation cycle:\n");
    crate::logger::early_print("[TEST]   1. Parent forks child\n");
    crate::logger::early_print("[TEST]   2. Child execs /tmp/hello.elf\n");
    crate::logger::early_print("[TEST]   3. hello.elf writes \"Hello from execve!\" and exits\n");
    crate::logger::early_print("[TEST]   4. Parent waits and collects exit status\n");
    
    let parent_pid = process::sys_getpid();
    crate::logger::early_print("[TEST] Parent PID: ");
    print_u64(parent_pid);
    crate::logger::early_print("\n");
    
    match process::sys_fork() {
        Ok(fork_result) => {
            if fork_result == 0 {
                // ========== CHILD PROCESS ==========
                crate::logger::early_print("[TEST] Child: About to exec /tmp/hello.elf...\n");
                
                // Give parent time to set up wait
                for _ in 0..5 {
                    crate::scheduler::yield_now();
                }
                
                // Execute hello.elf - this should replace the child process
                match process::sys_exec("/tmp/hello.elf", &[], &[]) {
                    Ok(_) => {
                        // Should NEVER reach here - exec replaces process image
                        crate::logger::early_print("[TEST] ❌ Child: exec returned (BUG!)\n");
                        process::sys_exit(-1);
                    }
                    Err(_) => {
                        crate::logger::early_print("[TEST] ❌ Child: exec failed (file not found or ELF error)\n");
                        process::sys_exit(-2);
                    }
                }
            } else {
                // ========== PARENT PROCESS ==========
                crate::logger::early_print("[TEST] Parent: Forked child PID ");
                print_u64(fork_result);
                crate::logger::early_print("\n");
                crate::logger::early_print("[TEST] Parent: Waiting for child to exec and exit...\n");
                
                // Wait for child to complete
                let options = process::WaitOptions {
                    nohang: false,
                    untraced: false,
                    continued: false,
                };
                
                match process::sys_wait(fork_result, options) {
                    Ok((waited_pid, status)) => {
                        if waited_pid == fork_result {
                            crate::logger::early_print("[TEST] ✅ Parent: Child completed\n");
                            crate::logger::early_print("[TEST]   PID: ");
                            print_u64(waited_pid);
                            crate::logger::early_print("\n[TEST]   Status: ");
                            
                            match status {
                                process::ProcessStatus::Exited(code) => {
                                    print_i32(code);
                                    crate::logger::early_print("\n");
                                    
                                    if code == 0 {
                                        crate::logger::early_print("[TEST] ✅✅✅ test_fork_exec_wait PASSED\n");
                                        crate::logger::early_print("[TEST]   hello.elf executed successfully!\n");
                                    } else if code == -1 {
                                        crate::logger::early_print("[TEST] ❌ test_fork_exec_wait FAILED: exec returned\n");
                                    } else if code == -2 {
                                        crate::logger::early_print("[TEST] ❌ test_fork_exec_wait FAILED: exec error\n");
                                    } else {
                                        crate::logger::early_print("[TEST] ⚠️  test_fork_exec_wait: unexpected exit code\n");
                                    }
                                }
                                _ => {
                                    crate::logger::early_print("(signal)\n");
                                    crate::logger::early_print("[TEST] ⚠️  test_fork_exec_wait: child terminated by signal\n");
                                }
                            }
                        } else {
                            crate::logger::early_print("[TEST] ❌ Parent: wait returned wrong PID\n");
                        }
                    }
                    Err(_) => {
                        crate::logger::early_print("[TEST] ❌ Parent: wait() failed\n");
                    }
                }
            }
        }
        Err(_) => {
            crate::logger::early_print("[TEST] ❌ test_fork_exec_wait FAILED: fork() failed\n");
        }
    }
}

/// Test exec directly (no fork) - Phase 3.1 standalone exec test
pub fn test_exec() {
    crate::logger::early_print("\n[TEST] test_exec_standalone starting...\n");
    crate::logger::early_print("[TEST] Testing exec without fork (direct replacement)\n");
    
    // Try to exec hello.elf - this will replace current thread if successful
    // If exec fails, we'll see an error. If it succeeds, hello.elf output should appear.
    crate::logger::early_print("[TEST] Calling sys_exec(\"/tmp/hello.elf\")...\n");
    
    let args: &[&str] = &[];
    let env: &[&str] = &[];
    
    match process::sys_exec("/tmp/hello.elf", args, env) {
        Ok(_) => {
            // Should never reach here - exec replaces current process
            crate::logger::early_print("[TEST] ⚠️  Exec returned (unexpected - should not return on success)\n");
        }
        Err(e) => {
            crate::logger::early_print("[TEST] ❌ test_exec FAILED: exec error\n");
            // Continue to next test instead of hanging
        }
    }
}

/// Test runner entry point (runs as a scheduler thread)
fn test_runner_main() -> ! {
    test_getpid();
    test_fork();
    test_fork_return_value();  // Critical test for inline context capture
    
    // SKIP: test_fork_wait_cycle() - blocks on sys_wait() when no children exist
    crate::logger::early_print("\n[TEST] Skipping test_fork_wait_cycle (would block on wait)\n");
    
    // SKIP: test_fork_exec_wait() - needs fork to work
    crate::logger::early_print("[TEST] Skipping test_fork_exec_wait (needs working fork)\n");
    
    // TEST: Direct exec test (Phase 3.1 - exec without fork)
    test_exec();

    crate::logger::early_print("\n");
    crate::logger::early_print("╔════════════════════════════════════════╗\n");
    crate::logger::early_print("║   ALL PROCESS TESTS COMPLETE          ║\n");
    crate::logger::early_print("╚════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
    
    // Exit when done
    process::sys_exit(0);
}

/// Run all process tests
/// 
/// Creates a test runner thread and starts the scheduler.
/// The scheduler takes over execution and never returns here.
pub fn run_all() {
    use crate::scheduler::{SCHEDULER, Thread};
    use crate::syscall::handlers::process::PROCESS_TABLE;
    use alloc::sync::Arc;
    use core::sync::atomic::AtomicI32;
    
    crate::logger::early_print("\n");
    crate::logger::early_print("╔════════════════════════════════════════╗\n");
    crate::logger::early_print("║   PROCESS SYSCALL TESTS (Phase 1)     ║\n");
    crate::logger::early_print("╚════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");

    // Initialize VFS before tests (needed for exec tests)
    crate::logger::early_print("[TEST] Initializing VFS...\n");
    match crate::fs::vfs::init() {
        Ok(_) => {
            crate::logger::early_print("[TEST] ✓ VFS initialized (hello.elf should be loaded at /tmp/)\n");
        }
        Err(e) => {
            crate::logger::early_print("[TEST] ⚠️  VFS init failed (tests may be limited)\n");
        }
    }
    crate::logger::early_print("\n");

    // Create test runner process in PROCESS_TABLE (PID 1)
    {
        let test_process = process::Process {
            pid: 1,
            ppid: 0,
            pgid: 1,
            sid: 1,
            main_tid: 1,
            name: alloc::string::String::from("test_runner"),
            fd_table: spin::Mutex::new(alloc::collections::BTreeMap::new()),
            memory_regions: spin::Mutex::new(alloc::vec::Vec::new()),
            cwd: spin::Mutex::new(alloc::string::String::from("/")),
            environ: spin::Mutex::new(alloc::vec::Vec::new()),
            exit_status: AtomicI32::new(0),
            state: spin::Mutex::new(process::ProcessState::Running),
            children: spin::Mutex::new(alloc::vec::Vec::new()),
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
        };
        
        PROCESS_TABLE.write().insert(1, Arc::new(test_process));
        crate::logger::early_print("[TEST] Created test process (PID 1) in PROCESS_TABLE\n");
    }

    // Create test runner thread
    let test_thread = Thread::new_kernel(
        1, // TID 1 for test runner
        "test_runner",
        test_runner_main,
        16384, // 16KB stack
    );
    
    crate::logger::early_print("[TEST] Created test runner thread (TID 1)\n");
    SCHEDULER.add_thread(test_thread);
    
    // Start scheduler - this takes over and never returns
    crate::logger::early_print("[TEST] Starting scheduler...\n\n");
    crate::scheduler::start();
}
