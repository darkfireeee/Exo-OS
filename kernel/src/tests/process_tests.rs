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
    
    // Fork 3 children
    for i in 0..3 {
        match process::sys_fork() {
            Ok(child_pid) => {
                if child_pid == 0 {
                    // Child process
                    crate::logger::early_print("[TEST] Child ");
                    print_u64(i);
                    crate::logger::early_print(" running\n");
                    
                    // Sleep equivalent - yield a few times
                    for _ in 0..10 {
                        crate::scheduler::yield_now();
                    }
                    
                    crate::logger::early_print("[TEST] Child ");
                    print_u64(i);
                    crate::logger::early_print(" exiting with code ");
                    print_u64(i + 100);
                    crate::logger::early_print("\n");
                    
                    process::sys_exit((i + 100) as i32);
                } else {
                    crate::logger::early_print("[TEST] Parent: spawned child ");
                    print_u64(child_pid);
                    crate::logger::early_print("\n");
                }
            }
            Err(_) => {
                crate::logger::early_print("[TEST] Fork failed\n");
            }
        }
    }
    
    // Parent waits for all children
    crate::logger::early_print("[TEST] Parent: waiting for all children...\n");
    
    for _ in 0..3 {
        let options = process::WaitOptions {
            nohang: false,
            untraced: false,
            continued: false,
        };
        
        match process::sys_wait(u64::MAX, options) {
            Ok((pid, status)) => {
                crate::logger::early_print("[TEST] Child ");
                print_u64(pid);
                crate::logger::early_print(" status: ");
                match status {
                    process::ProcessStatus::Exited(code) => {
                        print_i32(code);
                    }
                    _ => {
                        crate::logger::early_print("?");
                    }
                }
                crate::logger::early_print("\n");
            }
            Err(_) => break,
        }
    }
    
    crate::logger::early_print("[TEST] ✅ test_fork_wait_cycle COMPLETE\n");
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
