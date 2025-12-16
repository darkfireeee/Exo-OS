//! Test program for fork() syscall
//! 
//! This will be integrated into the kernel to test fork functionality

use crate::syscall::dispatch::syscall_numbers::*;

/// Test fork syscall
pub fn test_fork_syscall() {
    crate::logger::early_print("\n");
    crate::logger::early_print("╔══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║           PHASE 1b - FORK/EXEC/WAIT TEST               ║\n");
    crate::logger::early_print("╚══════════════════════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
    
    // Test 1: Simple fork
    crate::logger::early_print("[TEST 1] Testing sys_fork()...\n");
    
    unsafe {
        let args = [0u64; 6];
        let result = crate::syscall::dispatch::dispatch_syscall(SYS_FORK as u64, &args);
        
        if result > 0 {
            // Parent process
            let child_pid = result as u64;
            crate::logger::early_print("[PARENT] fork() returned child PID: ");
            let s = alloc::format!("{}\n", child_pid);
            crate::logger::early_print(&s);
            
            // Wait for child
            crate::logger::early_print("[PARENT] Waiting for child to exit...\n");
            let mut wstatus: i32 = 0;
            let wait_args = [
                child_pid,
                &mut wstatus as *mut i32 as u64,
                0, // options
                0, 0, 0
            ];
            let wait_result = crate::syscall::dispatch::dispatch_syscall(SYS_WAIT4 as u64, &wait_args);
            
            if wait_result > 0 {
                crate::logger::early_print("[PARENT] Child exited, status: ");
                let exit_code = (wstatus >> 8) & 0xFF;
                let s = alloc::format!("{}\n", exit_code);
                crate::logger::early_print(&s);
                crate::logger::early_print("[TEST 1] ✅ PASS: fork + wait successful\n");
            } else {
                crate::logger::early_print("[PARENT] wait4() failed\n");
                crate::logger::early_print("[TEST 1] ❌ FAIL: wait failed\n");
            }
        } else if result == 0 {
            // This shouldn't happen - fork returns child PID to parent
            crate::logger::early_print("[CHILD] fork() returned 0 (this is child)\n");
            crate::logger::early_print("[CHILD] Child process running...\n");
        } else {
            crate::logger::early_print("[ERROR] fork() failed with error: ");
            let s = alloc::format!("{}\n", result);
            crate::logger::early_print(&s);
            crate::logger::early_print("[TEST 1] ❌ FAIL: fork failed\n");
        }
    }
    
    crate::logger::early_print("\n");
    crate::logger::early_print("╔══════════════════════════════════════════════════════════╗\n");
    crate::logger::early_print("║           PHASE 1b TESTS COMPLETE                       ║\n");
    crate::logger::early_print("╚══════════════════════════════════════════════════════════╝\n");
    crate::logger::early_print("\n");
}
