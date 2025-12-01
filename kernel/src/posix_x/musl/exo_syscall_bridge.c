/* exo_syscall_bridge.c - C to Rust Syscall Bridge for Exo-OS
 *
 * This file provides the syscall interface that musl libc will call.
 * All syscalls are redirected to the Rust kernel via exo_kernel_syscall().
 */

#include <stdint.h>
#include <stddef.h>
#include "exo_syscall_numbers.h"

/* External Rust function (defined in kernel/src/posix_x/libc_impl/bridge.rs) */
extern int64_t exo_kernel_syscall(
    int64_t num,
    uint64_t a1, uint64_t a2, uint64_t a3,
    uint64_t a4, uint64_t a5, uint64_t a6
);

/* ============================================================================
 * Syscall Wrappers (called by musl libc)
 * ============================================================================ */

long __syscall0(long n) {
    return exo_kernel_syscall(n, 0, 0, 0, 0, 0, 0);
}

long __syscall1(long n, long a1) {
    return exo_kernel_syscall(n, (uint64_t)a1, 0, 0, 0, 0, 0);
}

long __syscall2(long n, long a1, long a2) {
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, 0, 0, 0, 0);
}

long __syscall3(long n, long a1, long a2, long a3) {
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, (uint64_t)a3, 0, 0, 0);
}

long __syscall4(long n, long a1, long a2, long a3, long a4) {
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, (uint64_t)a3, (uint64_t)a4, 0, 0);
}

long __syscall5(long n, long a1, long a2, long a3, long a4, long a5) {
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, (uint64_t)a3, (uint64_t)a4, (uint64_t)a5, 0);
}

long __syscall6(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, (uint64_t)a3, (uint64_t)a4, (uint64_t)a5, (uint64_t)a6);
}

/* ============================================================================
 * Cancellation Point Wrappers (for pthread support)
 * ============================================================================ */

long __syscall_cp(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    /* For now, just forward to regular syscall */
    /* TODO: Add cancellation point handling when pthread is implemented */
    return exo_kernel_syscall(n, (uint64_t)a1, (uint64_t)a2, (uint64_t)a3, (uint64_t)a4, (uint64_t)a5, (uint64_t)a6);
}
