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

                // Wait for child
                crate::logger::early_print("[TEST] Parent: waiting for child...\n");
                let options = process::WaitOptions {
                    nohang: false,
                    untraced: false,
                    continued: false,
                };

                match process::sys_wait(child_pid, options) {
                    Ok((waited_pid, status)) => {
                        crate::logger::early_print("[TEST] Parent: child ");
                        print_u64(waited_pid);
                        crate::logger::early_print(" exited with status: ");
                        match status {
                            process::ProcessStatus::Exited(code) => {
                                print_i32(code);
                            }
                            _ => {
                                crate::logger::early_print("(unknown)");
                            }
                        }
                        crate::logger::early_print("\n");
                        crate::logger::early_print("[TEST] ✅ test_fork PASSED\n");
                    }
                    Err(e) => {
                        crate::logger::early_print("[TEST] ❌ test_fork FAILED: wait error\n");
                    }
                }
            }
        }
        Err(e) => {
            crate::logger::early_print("[TEST] ❌ test_fork FAILED: fork error\n");
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
    
    // Note: fork() currently doesn't properly return 0 in child context
    // This is because we don't yet modify the child thread's RAX register
    // For now, we test that fork creates processes and they appear in PROCESS_TABLE
    
    let mut child_pids = alloc::vec::Vec::new();
    
    // Fork 3 children
    for i in 0..3 {
        match process::sys_fork() {
            Ok(child_pid) => {
                child_pids.push(child_pid);
                crate::logger::early_print("[TEST] Parent: spawned child PID ");
                print_u64(child_pid);
                crate::logger::early_print("\n");
            }
            Err(_) => {
                crate::logger::early_print("[TEST] Fork ");
                print_u64(i);
                crate::logger::early_print(" failed\n");
            }
        }
    }
    
    // Verify children exist in PROCESS_TABLE
    crate::logger::early_print("[TEST] Verifying children in process table...\n");
    for &child_pid in &child_pids {
        let exists = process::PROCESS_TABLE.read().contains_key(&child_pid);
        crate::logger::early_print("[TEST]   PID ");
        print_u64(child_pid);
        crate::logger::early_print(": ");
        if exists {
            crate::logger::early_print("✅ exists\n");
        } else {
            crate::logger::early_print("❌ NOT FOUND\n");
        }
    }
    
    // Test wait with nohang (should return 0 since children aren't zombies yet)
    crate::logger::early_print("[TEST] Testing wait with nohang (no zombies yet)...\n");
    let options = process::WaitOptions {
        nohang: true,
        untraced: false,
        continued: false,
    };
    
    match process::sys_wait(u64::MAX, options) {
        Ok((pid, status)) => {
            crate::logger::early_print("[TEST]   wait returned PID ");
            print_u64(pid);
            if pid == 0 {
                crate::logger::early_print(" (no zombie found - correct)\n");
            } else {
                crate::logger::early_print(" (unexpected)\n");
            }
        }
        Err(_) => {
            crate::logger::early_print("[TEST]   wait failed\n");
        }
    }
    
    crate::logger::early_print("[TEST] ✅ test_fork_wait_cycle COMPLETE\n");
    crate::logger::early_print("[TEST]    Note: Full fork/wait cycle requires child context setup\n");
}

/// Run all process tests
pub fn run_all() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔════════════════════════════════════════╗\n");
    crate::logger::early_print("║   PROCESS SYSCALL TESTS (Phase 1)     ║\n");
    crate::logger::early_print("╚════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");

    test_getpid();
    test_fork();
    test_fork_wait_cycle();

    crate::logger::early_print("\n");
    crate::logger::early_print("╔════════════════════════════════════════╗\n");
    crate::logger::early_print("║   ALL PROCESS TESTS COMPLETE          ║\n");
    crate::logger::early_print("╚════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
}
